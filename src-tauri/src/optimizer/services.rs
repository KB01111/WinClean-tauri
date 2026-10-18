//! Windows Service Control Manager access (windows-sys, no shell-outs).

use serde::{Deserialize, Serialize};

use super::policy::{classify_service, ServiceClassification};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceInfo {
    pub name: String,
    pub display_name: String,
    pub state: String,
    pub start_type: String,
    pub delayed_auto_start: bool,
    pub pid: Option<u32>,
    pub dependencies: Vec<String>,
    pub classification: ServiceClassification,
    pub can_change: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StartType {
    Automatic,
    AutomaticDelayed,
    Manual,
    Disabled,
}

impl StartType {
    pub fn as_str(&self) -> &'static str {
        match self {
            StartType::Automatic => "automatic",
            StartType::AutomaticDelayed => "automaticDelayed",
            StartType::Manual => "manual",
            StartType::Disabled => "disabled",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "automatic" => Some(StartType::Automatic),
            "automaticDelayed" => Some(StartType::AutomaticDelayed),
            "manual" => Some(StartType::Manual),
            "disabled" => Some(StartType::Disabled),
            _ => None,
        }
    }
    #[cfg(windows)]
    fn scm_value(&self) -> u32 {
        // SERVICE_AUTO_START=2, SERVICE_DEMAND_START=3, SERVICE_DISABLED=4
        match self {
            StartType::Automatic | StartType::AutomaticDelayed => 2,
            StartType::Manual => 3,
            StartType::Disabled => 4,
        }
    }
}

#[cfg(windows)]
mod win {
    use super::*;
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_MORE_DATA, HANDLE};
    use windows_sys::Win32::System::Services::*;

    /// Reads a NUL-terminated UTF-16 string starting at `ptr`, never
    /// reading at or past `buf_end`. Pointers from the SCM enumeration
    /// reference into `buf`; without the bound a short tail string would
    /// make a fixed-length slice read out of bounds (UB).
    unsafe fn wide_bounded(ptr: *const u16, buf_end: *const u16) -> String {
        if ptr.is_null() || ptr >= buf_end { return String::new(); }
        let max = (buf_end as usize - ptr as usize) / std::mem::size_of::<u16>();
        let mut len = 0usize;
        while len < max && unsafe { *ptr.add(len) } != 0 { len += 1; }
        wide(unsafe { std::slice::from_raw_parts(ptr, len) })
    }

    fn wide(bytes: &[u16]) -> String {
        let end = bytes.iter().position(|&c| c == 0).unwrap_or(bytes.len());
        OsString::from_wide(&bytes[..end]).to_string_lossy().into_owned()
    }
    fn state_str(v: u32) -> String {
        match v { 1 => "stopped", 2 => "startPending", 3 => "stopPending", 4 => "running", 5 => "continuePending", 6 => "pausePending", 7 => "paused", _ => "unknown" }.to_string()
    }
    fn start_str(v: u32) -> &'static str {
        match v { 0 => "boot", 1 => "system", 2 => "automatic", 3 => "manual", 4 => "disabled", _ => "unknown" }
    }

    struct Scm(HANDLE);
    impl Drop for Scm { fn drop(&mut self) { unsafe { CloseServiceHandle(self.0); } } }

    fn open_scm(access: u32) -> Result<Scm, String> {
        let h = unsafe { OpenSCManagerW(std::ptr::null(), std::ptr::null(), access) };
        if h.is_null() { return Err(format!("OpenSCManager failed ({}). Reading services needs no elevation; changing them needs an elevated WinClean.", std::io::Error::last_os_error())); }
        Ok(Scm(h))
    }
    fn open_service(scm: &Scm, name: &str, access: u32) -> Result<Scm, String> {
        let w: Vec<u16> = name.encode_utf16().chain([0]).collect();
        let h = unsafe { OpenServiceW(scm.0, w.as_ptr(), access) };
        if h.is_null() { return Err(format!("OpenService({name}) failed: {}", std::io::Error::last_os_error())); }
        Ok(Scm(h))
    }

    fn query_config(svc: &Scm) -> (String, bool, Vec<String>) {
        let mut needed = 0u32;
        unsafe { QueryServiceConfigW(svc.0, std::ptr::null_mut(), 0, &mut needed); }
        if needed == 0 { return ("unknown".into(), false, Vec::new()); }
        let mut buf = vec![0u8; needed as usize];
        let ok = unsafe { QueryServiceConfigW(svc.0, buf.as_mut_ptr() as *mut QUERY_SERVICE_CONFIGW, needed, &mut needed) };
        if ok == 0 { return ("unknown".into(), false, Vec::new()); }
        let cfg = unsafe { &*(buf.as_ptr() as *const QUERY_SERVICE_CONFIGW) };
        let mut deps = Vec::new();
        if !cfg.lpDependencies.is_null() {
            // Double-null terminated multi-string.
            let mut p = cfg.lpDependencies;
            loop {
                let mut len = 0usize;
                while unsafe { *p.add(len) } != 0 { len += 1; }
                if len == 0 { break; }
                deps.push(wide(unsafe { std::slice::from_raw_parts(p, len) }));
                p = unsafe { p.add(len + 1) };
            }
        }
        // Delayed auto-start flag (SERVICE_CONFIG_DELAYED_AUTO_START_INFO = 3).
        let mut delayed = false;
        let mut info = SERVICE_DELAYED_AUTO_START_INFO { fDelayedAutostart: 0 };
        let mut n2 = 0u32;
        let ok2 = unsafe { QueryServiceConfig2W(svc.0, 3, &mut info as *mut _ as *mut u8, std::mem::size_of::<SERVICE_DELAYED_AUTO_START_INFO>() as u32, &mut n2) };
        if ok2 != 0 { delayed = info.fDelayedAutostart != 0; }
        (start_str(cfg.dwStartType).to_string(), delayed, deps)
    }

    pub fn list_services_impl() -> Result<Vec<ServiceInfo>, String> {
        let scm = open_scm(SC_MANAGER_ENUMERATE_SERVICE | SC_MANAGER_CONNECT)?;
        let mut bytes_needed = 0u32;
        let mut count = 0u32;
        let mut resume: u32 = 0;
        unsafe { EnumServicesStatusExW(scm.0, SC_ENUM_PROCESS_INFO, SERVICE_WIN32, SERVICE_STATE_ALL, std::ptr::null_mut(), 0, &mut bytes_needed, &mut count, &mut resume, std::ptr::null()) };
        if bytes_needed == 0 { return Ok(Vec::new()); }
        let mut buf = vec![0u8; bytes_needed as usize];
        let ok = unsafe { EnumServicesStatusExW(scm.0, SC_ENUM_PROCESS_INFO, SERVICE_WIN32, SERVICE_STATE_ALL, buf.as_mut_ptr(), bytes_needed, &mut bytes_needed, &mut count, &mut resume, std::ptr::null()) };
        if ok == 0 && std::io::Error::last_os_error().raw_os_error() != Some(ERROR_MORE_DATA as i32) {
            return Err(format!("EnumServicesStatusEx failed: {}", std::io::Error::last_os_error()));
        }
        let mut out = Vec::with_capacity(count as usize);
        let base = buf.as_ptr() as *const ENUM_SERVICE_STATUS_PROCESSW;
        for i in 0..count as usize {
            let e = unsafe { &*base.add(i) };
            let buf_end = unsafe { buf.as_ptr().add(buf.len()) } as *const u16;
            let name = if e.lpServiceName.is_null() { String::new() } else { unsafe { wide_bounded(e.lpServiceName, buf_end) } };
            let display = if e.lpDisplayName.is_null() { name.clone() } else { unsafe { wide_bounded(e.lpDisplayName, buf_end) } };
            // Per-service config needs a handle; failures degrade to unknown.
            let (mut start_type, delayed, deps) = match open_service(&scm, &name, SERVICE_QUERY_CONFIG) {
                Ok(svc) => query_config(&svc),
                Err(_) => ("unknown".into(), false, Vec::new()),
            };
            if start_type == "automatic" && delayed { start_type = "automaticDelayed".into(); }
            let classification = classify_service(&name, &display);
            let can_change = classification.risk != super::super::policy::ServiceRisk::Protected;
            out.push(ServiceInfo {
                can_change,
                classification,
                name,
                display_name: display,
                state: state_str(e.ServiceStatusProcess.dwCurrentState),
                start_type,
                delayed_auto_start: delayed,
                pid: if e.ServiceStatusProcess.dwProcessId != 0 { Some(e.ServiceStatusProcess.dwProcessId) } else { None },
                dependencies: deps,
            });
            let _ = CloseHandle; // silence unused import on some targets
        }
        out.sort_by(|a, b| a.display_name.to_lowercase().cmp(&b.display_name.to_lowercase()));
        Ok(out)
    }

    pub fn set_start_type_impl(name: &str, target: StartType) -> Result<ServiceInfo, String> {
        let scm = open_scm(SC_MANAGER_CONNECT)?;
        let svc = open_service(&scm, name, SERVICE_CHANGE_CONFIG | SERVICE_QUERY_CONFIG | SERVICE_QUERY_STATUS)?;
        let ok = unsafe { ChangeServiceConfigW(svc.0, SERVICE_NO_CHANGE, target.scm_value(), SERVICE_NO_CHANGE, std::ptr::null(), std::ptr::null(), std::ptr::null_mut(), std::ptr::null(), std::ptr::null(), std::ptr::null(), std::ptr::null()) };
        if ok == 0 {
            return Err(format!("ChangeServiceConfig({name}) failed: {}. Changing a service requires running WinClean elevated (UAC) for this action.", std::io::Error::last_os_error()));
        }
        if target == StartType::AutomaticDelayed || target == StartType::Automatic {
            let mut info = SERVICE_DELAYED_AUTO_START_INFO { fDelayedAutostart: if target == StartType::AutomaticDelayed { 1 } else { 0 } };
            let ok2 = unsafe { ChangeServiceConfig2W(svc.0, 3, &mut info as *mut _ as *const std::ffi::c_void) };
            if ok2 == 0 {
                return Err(format!("ChangeServiceConfig2W(delayedStart, {name}) failed: {}. The start type was changed but the delayed flag was not; re-read the service before retrying.", std::io::Error::last_os_error()));
            }
        }
        // Verify by re-reading.
        let (mut start_type, delayed, deps) = query_config(&svc);
        if start_type == "automatic" && delayed { start_type = "automaticDelayed".into(); }
        let mut status = SERVICE_STATUS { dwServiceType: 0, dwCurrentState: 0, dwControlsAccepted: 0, dwWin32ExitCode: 0, dwServiceSpecificExitCode: 0, dwCheckPoint: 0, dwWaitHint: 0 };
        unsafe { QueryServiceStatus(svc.0, &mut status); }
        // Display name: reuse name; caller re-lists for full row.
        Ok(ServiceInfo {
            name: name.to_string(),
            display_name: name.to_string(),
            state: state_str(status.dwCurrentState),
            start_type,
            delayed_auto_start: delayed,
            pid: None,
            dependencies: deps,
            classification: classify_service(name, name),
            can_change: true,
        })
    }
}

#[cfg(windows)]
pub fn list_services() -> Result<Vec<ServiceInfo>, String> { win::list_services_impl() }
#[cfg(not(windows))]
pub fn list_services() -> Result<Vec<ServiceInfo>, String> {
    Err("Windows Service optimization is only available on Windows.".into())
}

#[cfg(windows)]
pub fn set_service_start_type(name: &str, target: StartType) -> Result<ServiceInfo, String> {
    win::set_start_type_impl(name, target)
}
#[cfg(not(windows))]
pub fn set_service_start_type(_name: &str, _target: StartType) -> Result<ServiceInfo, String> {
    Err("Windows Service optimization is only available on Windows.".into())
}

/// Snapshot payload stored before every mutation (for rollback).
pub fn snapshot_payload(s: &ServiceInfo) -> serde_json::Value {
    serde_json::json!({
        "name": s.name, "displayName": s.display_name,
        "startType": s.start_type, "delayedAutoStart": s.delayed_auto_start,
        "state": s.state,
    })
}

pub fn start_type_from_snapshot(v: &serde_json::Value) -> Option<StartType> {
    let delayed = v.get("delayedAutoStart").and_then(|x| x.as_bool()).unwrap_or(false);
    match v.get("startType").and_then(|x| x.as_str())? {
        "automatic" if delayed => Some(StartType::AutomaticDelayed),
        "automaticDelayed" => Some(StartType::AutomaticDelayed),
        "automatic" => Some(StartType::Automatic),
        "manual" => Some(StartType::Manual),
        "disabled" => Some(StartType::Disabled),
        _ => None,
    }
}
