//! Event sources that wake the optimizer runtime instead of polling.
//!
//! * Power settings (`PowerSettingRegisterNotification`, callback flavor — no
//!   window needed): AC/DC source, battery percentage, Windows power-saving
//!   status. Windows calls back immediately with the current value and then
//!   only on change.
//! * Foreground window changes (`SetWinEventHook(EVENT_SYSTEM_FOREGROUND)`,
//!   out-of-context) on a dedicated message-loop thread, so workload-based
//!   profile rules react to a game or editor coming to the front without any
//!   timer.
//!
//! Neither touches a GPU API, so neither can wake the RTX 4070.

use std::sync::Arc;

use super::telemetry::Wake;

/// Keeps the registrations alive; dropping it unregisters everything.
pub struct PowerEvents {
    /// Held only to keep the registrations alive until drop.
    #[cfg(windows)]
    #[allow(dead_code)]
    inner: win::Registrations,
}

impl PowerEvents {
    /// Registers all event sources. Failures are reported but never fatal: the
    /// runtime still has its slow safety-net timer.
    pub fn start(wake: Arc<Wake>) -> (Self, Vec<String>) {
        #[cfg(windows)]
        {
            let (inner, notes) = win::Registrations::start(wake);
            (Self { inner }, notes)
        }
        #[cfg(not(windows))]
        {
            let _ = wake;
            (Self {}, vec!["Power events are only available on Windows.".to_string()])
        }
    }
}

#[cfg(windows)]
mod win {
    use super::*;
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicU32, Ordering};

    use windows_sys::Win32::System::Power::{
        PowerSettingRegisterNotification, PowerSettingUnregisterNotification, DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS,
    };
    use windows_sys::Win32::System::SystemServices::{GUID_ACDC_POWER_SOURCE, GUID_BATTERY_PERCENTAGE_REMAINING, GUID_POWER_SAVING_STATUS};
    use windows_sys::Win32::System::Threading::GetCurrentThreadId;
    use windows_sys::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetMessageW, PostThreadMessageW, DEVICE_NOTIFY_CALLBACK, EVENT_SYSTEM_FOREGROUND, MSG, WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS, WM_QUIT,
    };

    /// The `Wake` the callbacks signal. Callbacks are plain `extern "system"`
    /// functions, so the target is reached through a process-wide slot.
    static WAKE: std::sync::OnceLock<Arc<Wake>> = std::sync::OnceLock::new();

    unsafe extern "system" fn power_callback(_context: *const c_void, _kind: u32, _setting: *const c_void) -> u32 {
        // Must be quick and must never panic across the FFI boundary.
        if let Some(w) = WAKE.get() {
            w.notify();
        }
        0
    }

    unsafe extern "system" fn foreground_callback(_hook: HWINEVENTHOOK, _event: u32, _hwnd: *mut c_void, _id_object: i32, _id_child: i32, _thread: u32, _time: u32) {
        if let Some(w) = WAKE.get() {
            w.notify();
        }
    }

    pub struct Registrations {
        handles: Vec<*mut c_void>,
        // Boxed so the pointer handed to Windows stays valid.
        _params: Vec<Box<DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS>>,
        hook_thread_id: Arc<AtomicU32>,
        hook_thread: Option<std::thread::JoinHandle<()>>,
    }

    // SAFETY: the raw notification handles are only used to unregister in Drop.
    unsafe impl Send for Registrations {}
    unsafe impl Sync for Registrations {}

    impl Registrations {
        pub fn start(wake: Arc<Wake>) -> (Self, Vec<String>) {
            let _ = WAKE.set(wake);
            let mut notes = Vec::new();
            let mut handles = Vec::new();
            let mut params = Vec::new();
            for (name, guid) in [("AC/DC source", &GUID_ACDC_POWER_SOURCE), ("battery percentage", &GUID_BATTERY_PERCENTAGE_REMAINING), ("power saving status", &GUID_POWER_SAVING_STATUS)] {
                let mut p = Box::new(DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS { Callback: Some(power_callback), Context: std::ptr::null_mut() });
                let mut handle: *mut c_void = std::ptr::null_mut();
                let rc = unsafe { PowerSettingRegisterNotification(guid, DEVICE_NOTIFY_CALLBACK, &mut *p as *mut _ as *mut c_void as _, &mut handle) };
                if rc == 0 && !handle.is_null() {
                    handles.push(handle);
                    params.push(p);
                } else {
                    notes.push(format!("could not subscribe to {name} changes (Win32 {rc}); falling back to timed checks"));
                }
            }

            // Foreground hook on its own message-loop thread.
            let thread_id = Arc::new(AtomicU32::new(0));
            let tid = thread_id.clone();
            let (ready_tx, ready_rx) = std::sync::mpsc::channel::<bool>();
            let hook_thread = std::thread::Builder::new().name("winclean-foreground-hook".into()).spawn(move || unsafe {
                tid.store(GetCurrentThreadId(), Ordering::Release);
                let hook = SetWinEventHook(EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND, std::ptr::null_mut(), Some(foreground_callback), 0, 0, WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS);
                let ok = !hook.is_null();
                let _ = ready_tx.send(ok);
                if !ok {
                    return;
                }
                let mut msg: MSG = std::mem::zeroed();
                while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {}
                UnhookWinEvent(hook);
            });
            let hook_thread = match hook_thread {
                Ok(t) => {
                    if !ready_rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap_or(false) {
                        notes.push("could not hook foreground-window changes; workload rules fall back to timed checks".to_string());
                    }
                    Some(t)
                }
                Err(e) => {
                    notes.push(format!("could not start the foreground-event thread: {e}"));
                    None
                }
            };
            (Self { handles, _params: params, hook_thread_id: thread_id, hook_thread }, notes)
        }
    }

    impl Drop for Registrations {
        fn drop(&mut self) {
            for h in self.handles.drain(..) {
                unsafe { PowerSettingUnregisterNotification(h as isize) };
            }
            let tid = self.hook_thread_id.load(Ordering::Acquire);
            if tid != 0 {
                unsafe { PostThreadMessageW(tid, WM_QUIT, 0, 0) };
            }
            if let Some(t) = self.hook_thread.take() {
                let _ = t.join();
            }
        }
    }
}
