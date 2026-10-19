//! Process enumeration + EcoQoS via SetProcessInformation.
//! WinClean never terminates processes; EcoQoS is fully reversible.
//!
//! The setter revalidates the process that the PID *currently* identifies
//! (image name via QueryFullProcessImageNameW, foreground state, live
//! policy classification) before enabling throttling, so a reused PID or
//! a caller-supplied name can never throttle the wrong process.

use serde::{Deserialize, Serialize};

use super::policy::EcoQosDecision;
#[cfg(windows)]
use super::policy::classify_process_for_ecoqos;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessInfo {
    pub pid: u32,
    pub name: String,
    pub working_set_mb: f64,
    /// -1.0 means "no CPU sample available" (never treated as idle).
    pub cpu_percent_recent: f32,
    pub is_foreground: bool,
    pub eco_decision: EcoQosDecision,
    pub eco_qos_applied: bool,
}

/// Rows returned to the Processes view (largest working sets first).
#[cfg(windows)]
const UI_PROCESS_LIMIT: usize = 300;

/// What a PID currently identifies, used to tell a process WinClean
/// throttled from a different process that reused the same PID.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcessLiveness {
    /// No such process.
    Gone,
    /// Running; `started_ms` is the Unix-ms creation time.
    Alive { started_ms: i64 },
    /// Could not be determined (access denied, unsupported platform).
    Unknown,
}

/// Audit target for a process action: `"<image name> (<pid>)"`.
pub fn process_target(name: &str, pid: u32) -> String {
    format!("{name} ({pid})")
}

/// Inverse of [`process_target`].
pub fn parse_process_target(target: &str) -> Option<(String, u32)> {
    let body = target.strip_suffix(')')?;
    let (name, pid) = body.rsplit_once(" (")?;
    if name.is_empty() { return None; }
    Some((name.to_string(), pid.parse().ok()?))
}

#[cfg(windows)]
mod win {
    use super::*;
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
    };
    use windows_sys::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows_sys::Win32::System::Threading::{
        GetProcessInformation, GetProcessTimes, OpenProcess, QueryFullProcessImageNameW,
        SetProcessInformation, PROCESS_ACCESS_RIGHTS, PROCESS_POWER_THROTTLING_CURRENT_VERSION,
        PROCESS_POWER_THROTTLING_EXECUTION_SPEED, PROCESS_POWER_THROTTLING_STATE,
        ProcessPowerThrottling,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const PROCESS_SET_INFORMATION: u32 = 0x0200;
    const PROCESS_VM_READ: u32 = 0x0010;

    pub fn foreground_pid() -> u32 {
        unsafe {
            let hwnd = GetForegroundWindow();
            if hwnd.is_null() { return 0; }
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, &mut pid);
            pid
        }
    }

    fn filetime_ticks(ft: &FILETIME) -> u64 {
        ((ft.dwHighDateTime as u64) << 32) | ft.dwLowDateTime as u64
    }

    fn image_name(h: HANDLE) -> Option<String> {
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = unsafe { QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut len) };
        if ok == 0 || len == 0 { return None; }
        let path = String::from_utf16_lossy(&buf[..len as usize]);
        Some(path.rsplit(['\\', '/']).next().unwrap_or(&path).to_string())
    }

    /// Live EcoQoS bit; `None` when Windows could not report it.
    fn eco_qos_state(h: HANDLE) -> Option<bool> {
        let mut state = PROCESS_POWER_THROTTLING_STATE {
            Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
            ControlMask: 0,
            StateMask: 0,
        };
        let ok = unsafe {
            GetProcessInformation(h, ProcessPowerThrottling, &mut state as *mut _ as *mut std::ffi::c_void, std::mem::size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32)
        };
        if ok == 0 { return None; }
        Some((state.StateMask & PROCESS_POWER_THROTTLING_EXECUTION_SPEED) != 0)
    }

    /// Like [`eco_qos_state`] but a failed query reads as "off", for listing
    /// and post-change verification where unknown must not look throttled.
    fn eco_qos_applied(h: HANDLE) -> bool {
        eco_qos_state(h).unwrap_or(false)
    }

    fn sample_times() -> std::collections::HashMap<u32, u64> {
        let mut map = std::collections::HashMap::new();
        let s = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
        if s.is_null() || s == INVALID_HANDLE_VALUE { return map; }
        let mut e = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, cntUsage: 0, th32ProcessID: 0, th32DefaultHeapID: 0, th32ModuleID: 0, cntThreads: 0, th32ParentProcessID: 0, pcPriClassBase: 0, dwFlags: 0, szExeFile: [0; 260] };
        if unsafe { Process32FirstW(s, &mut e) } != 0 {
            loop {
                let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, e.th32ProcessID) };
                if !h.is_null() {
                    let (mut c, mut u, mut k, mut x) = (zero_ft(), zero_ft(), zero_ft(), zero_ft());
                    if unsafe { GetProcessTimes(h, &mut c, &mut x, &mut k, &mut u) } != 0 {
                        map.insert(e.th32ProcessID, filetime_ticks(&k) + filetime_ticks(&u));
                    }
                    unsafe { CloseHandle(h); }
                }
                if unsafe { Process32NextW(s, &mut e) } == 0 { break; }
            }
        }
        unsafe { CloseHandle(s); }
        map
    }

    fn zero_ft() -> FILETIME { FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 } }

    /// Kernel + user CPU time in 100 ns ticks, read through an open handle.
    fn cpu_ticks(h: HANDLE) -> Option<u64> {
        let (mut c, mut u, mut k, mut x) = (zero_ft(), zero_ft(), zero_ft(), zero_ft());
        if unsafe { GetProcessTimes(h, &mut c, &mut x, &mut k, &mut u) } == 0 { return None; }
        Some(filetime_ticks(&k) + filetime_ticks(&u))
    }

    /// Measures one process's CPU over a short window on its own handle.
    /// `None` when the times cannot be read.
    pub fn sample_cpu_percent(h: HANDLE) -> Option<f32> {
        let first = cpu_ticks(h)?;
        let started = std::time::Instant::now();
        std::thread::sleep(std::time::Duration::from_millis(150));
        let second = cpu_ticks(h)?;
        let window_ticks = (started.elapsed().as_secs_f64() * 10_000_000.0).max(1.0);
        Some((second.saturating_sub(first) as f64 / window_ticks * 100.0) as f32)
    }

    /// Live EcoQoS bit of a PID; `None` when it cannot be opened.
    pub fn process_ecoqos_state_impl(pid: u32) -> Option<bool> {
        let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if h.is_null() { return None; }
        let state = eco_qos_state(h);
        unsafe { CloseHandle(h); }
        state
    }

    /// Creation time (Unix ms) of the process behind an open handle.
    fn created_ms(h: HANDLE) -> Option<i64> {
        let (mut created, mut x, mut k, mut u) = (zero_ft(), zero_ft(), zero_ft(), zero_ft());
        if unsafe { GetProcessTimes(h, &mut created, &mut x, &mut k, &mut u) } == 0 { return None; }
        // FILETIME epoch (1601) -> Unix epoch (1970), 100 ns -> ms.
        const FILETIME_UNIX_DIFF_TICKS: u64 = 116_444_736_000_000_000;
        Some((filetime_ticks(&created).saturating_sub(FILETIME_UNIX_DIFF_TICKS) / 10_000) as i64)
    }

    /// Whether a PID is running and, if so, when its process was created.
    pub fn process_liveness_impl(pid: u32) -> ProcessLiveness {
        const ERROR_INVALID_PARAMETER: i32 = 87;
        let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if h.is_null() {
            // OpenProcess reports a PID that does not exist as invalid-parameter.
            return if std::io::Error::last_os_error().raw_os_error() == Some(ERROR_INVALID_PARAMETER) {
                ProcessLiveness::Gone
            } else {
                ProcessLiveness::Unknown
            };
        }
        let started = created_ms(h);
        unsafe { CloseHandle(h); }
        match started {
            Some(started_ms) => ProcessLiveness::Alive { started_ms },
            None => ProcessLiveness::Unknown,
        }
    }

    pub fn list_processes_impl(limit: Option<usize>) -> Result<Vec<ProcessInfo>, String> {
        let fg = foreground_pid();
        let first = sample_times();
        let started = std::time::Instant::now();
        std::thread::sleep(std::time::Duration::from_millis(250));
        // Actual elapsed window (100 ns ticks) — never assume 250 ms.
        let window_ticks = (started.elapsed().as_secs_f64() * 10_000_000.0).max(1.0);

        let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
        if snap.is_null() || snap == INVALID_HANDLE_VALUE { return Err(format!("CreateToolhelp32Snapshot failed: {}", std::io::Error::last_os_error())); }
        let mut out = Vec::new();
        let mut entry = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, cntUsage: 0, th32ProcessID: 0, th32DefaultHeapID: 0, th32ModuleID: 0, cntThreads: 0, th32ParentProcessID: 0, pcPriClassBase: 0, dwFlags: 0, szExeFile: [0; 260] };
        if unsafe { Process32FirstW(snap, &mut entry) } != 0 {
            loop {
                let pid = entry.th32ProcessID;
                let name = {
                    let s = String::from_utf16_lossy(&entry.szExeFile);
                    s.trim_end_matches('\0').to_string()
                };
                let mut ws_mb = 0.0f64;
                // No sample yet -> -1 sentinel, classified Unknown (never Eligible).
                let mut cpu = -1.0f32;
                let mut applied = false;
                // PROCESS_VM_READ is required by K32GetProcessMemoryInfo.
                let access = PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_VM_READ;
                let h = unsafe { OpenProcess(access, 0, pid) };
                if !h.is_null() {
                    let mut counters: PROCESS_MEMORY_COUNTERS = unsafe { std::mem::zeroed() };
                    counters.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
                    if unsafe { K32GetProcessMemoryInfo(h, &mut counters, counters.cb) } != 0 {
                        ws_mb = counters.WorkingSetSize as f64 / (1024.0 * 1024.0);
                    }
                    let (mut c, mut u, mut k, mut x) = (zero_ft(), zero_ft(), zero_ft(), zero_ft());
                    if unsafe { GetProcessTimes(h, &mut c, &mut x, &mut k, &mut u) } != 0 {
                        if let Some(prev) = first.get(&pid) {
                            let delta = (filetime_ticks(&k) + filetime_ticks(&u)).saturating_sub(*prev);
                            cpu = (delta as f64 / window_ticks * 100.0) as f32;
                        }
                    }
                    applied = eco_qos_applied(h);
                    unsafe { CloseHandle(h); }
                }
                let is_fg = pid != 0 && pid == fg;
                out.push(ProcessInfo {
                    eco_decision: classify_process_for_ecoqos(&name, is_fg, cpu),
                    pid, name, working_set_mb: (ws_mb * 10.0).round() / 10.0,
                    cpu_percent_recent: (cpu * 10.0).round() / 10.0,
                    is_foreground: is_fg, eco_qos_applied: applied,
                });
                if unsafe { Process32NextW(snap, &mut entry) } == 0 { break; }
            }
        }
        unsafe { CloseHandle(snap); }
        out.sort_by(|a, b| b.working_set_mb.partial_cmp(&a.working_set_mb).unwrap_or(std::cmp::Ordering::Equal));
        if let Some(limit) = limit { out.truncate(limit); }
        Ok(out)
    }

    /// Returns the process's real current image name after applying.
    pub fn set_ecoqos_impl(pid: u32, expected_name: &str, not_started_after_ms: Option<i64>, enable: bool) -> Result<String, String> {
        let access: PROCESS_ACCESS_RIGHTS = PROCESS_SET_INFORMATION | PROCESS_QUERY_LIMITED_INFORMATION;
        let h: HANDLE = unsafe { OpenProcess(access, 0, pid) };
        if h.is_null() {
            return Err(format!("OpenProcess({pid}) failed: {}. EcoQoS on system/other-user processes needs elevation; WinClean never forces it.", std::io::Error::last_os_error()));
        }
        let result = (|| {
            // Revalidate identity: the PID may have been reused since listing.
            let real = image_name(h).ok_or_else(|| format!("could not read image name for pid {pid}; refusing to guess"))?;
            if !expected_name.is_empty() && !real.eq_ignore_ascii_case(expected_name) {
                return Err(format!("pid {pid} now belongs to '{real}', not '{expected_name}' (stale listing). Refresh and retry."));
            }
            // Pin identity to the handle being mutated: a PID reused by a
            // same-named process since WinClean recorded the change has a
            // newer creation time and must not be touched.
            if let Some(limit) = not_started_after_ms {
                match created_ms(h) {
                    Some(started) if started <= limit => {}
                    Some(_) => return Err(format!("pid {pid} now belongs to a newer '{real}' process than the one WinClean changed; leaving it alone.")),
                    None => return Err(format!("could not read the start time of '{real}' (pid {pid}); refusing to change it blind.")),
                }
            }
            if enable {
                let is_fg = pid == foreground_pid();
                // Another actor (or an earlier run) already throttled it:
                // WinClean must not claim ownership of state it did not set.
                match eco_qos_state(h) {
                    Some(false) => {}
                    Some(true) => return Err(format!("'{real}' (pid {pid}) is already EcoQoS-throttled; WinClean will not claim it.")),
                    None => return Err(format!("could not read the EcoQoS state of '{real}' (pid {pid}); refusing to change it blind.")),
                }
                // Sample this process's CPU now: a busy updater/indexer must
                // be rejected here even if the listing showed it idle. Without
                // a sample the setter refuses rather than assuming idle.
                let cpu = sample_cpu_percent(h)
                    .ok_or_else(|| format!("could not sample CPU for '{real}' (pid {pid}); refusing to throttle without a measurement"))?;
                let decision = classify_process_for_ecoqos(&real, is_fg, cpu);
                if decision != EcoQosDecision::Eligible {
                    return Err(format!("'{real}' is {decision:?} and will not be throttled. Only clearly eligible, currently idle background work (updaters, indexers, telemetry) can be enabled."));
                }
            }
            let mut state = PROCESS_POWER_THROTTLING_STATE {
                Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
                ControlMask: if enable { PROCESS_POWER_THROTTLING_EXECUTION_SPEED } else { 0 },
                StateMask: if enable { PROCESS_POWER_THROTTLING_EXECUTION_SPEED } else { 0 },
            };
            let ok = unsafe { SetProcessInformation(h, ProcessPowerThrottling, &mut state as *mut _ as *mut std::ffi::c_void, std::mem::size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32) };
            if ok == 0 {
                return Err(format!("SetProcessInformation(EcoQoS, pid={pid}) failed: {}", std::io::Error::last_os_error()));
            }
            // Verify the requested state stuck.
            if eco_qos_applied(h) != enable {
                return Err(format!("EcoQoS verification failed for '{real}' (pid {pid}): state did not change."));
            }
            Ok(real)
        })();
        unsafe { CloseHandle(h); }
        result
    }
}

#[cfg(windows)]
pub fn list_processes() -> Result<Vec<ProcessInfo>, String> { win::list_processes_impl(Some(UI_PROCESS_LIMIT)) }
/// Every running process (no UI row cap), for profile application where a
/// small updater must not be missed just because it uses little memory.
#[cfg(windows)]
pub fn list_all_processes() -> Result<Vec<ProcessInfo>, String> { win::list_processes_impl(None) }
#[cfg(not(windows))]
pub fn list_processes() -> Result<Vec<ProcessInfo>, String> {
    Err("Process monitoring is only available on Windows.".into())
}
#[cfg(not(windows))]
pub fn list_all_processes() -> Result<Vec<ProcessInfo>, String> { list_processes() }

/// Applies or restores EcoQoS after revalidating the PID's real identity.
/// Returns the verified image name.
#[cfg(windows)]
pub fn set_process_ecoqos(pid: u32, expected_name: &str, not_started_after_ms: Option<i64>, enable: bool) -> Result<String, String> {
    win::set_ecoqos_impl(pid, expected_name, not_started_after_ms, enable)
}
#[cfg(not(windows))]
pub fn set_process_ecoqos(_pid: u32, _expected_name: &str, _not_started_after_ms: Option<i64>, _enable: bool) -> Result<String, String> {
    Err("EcoQoS is only available on Windows.".into())
}

/// Reports whether a PID is still the process WinClean may have throttled.
#[cfg(windows)]
pub fn process_liveness(pid: u32) -> ProcessLiveness { win::process_liveness_impl(pid) }
#[cfg(not(windows))]
pub fn process_liveness(_pid: u32) -> ProcessLiveness { ProcessLiveness::Unknown }

/// Reads the live EcoQoS bit, so callers only count a process as changed
/// when its state really was on (another app may have cleared it).
#[cfg(windows)]
pub fn process_ecoqos_state(pid: u32) -> Option<bool> { win::process_ecoqos_state_impl(pid) }
#[cfg(not(windows))]
pub fn process_ecoqos_state(_pid: u32) -> Option<bool> { None }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_target_round_trips() {
        assert_eq!(parse_process_target(&process_target("GoogleUpdate.exe", 4242)), Some(("GoogleUpdate.exe".into(), 4242)));
        // Names may themselves contain parentheses.
        assert_eq!(parse_process_target("Tool (x64).exe (17)"), Some(("Tool (x64).exe".into(), 17)));
        assert_eq!(parse_process_target("no pid"), None);
        assert_eq!(parse_process_target(" (5)"), None);
        assert_eq!(parse_process_target("a.exe (notanumber)"), None);
    }
}
