//! Windows Service Control Manager access (windows-sys, no shell-outs).
//!
//! Reads (enumeration, configuration, dependencies) need no elevation.
//! Start-type writes need `SERVICE_CHANGE_CONFIG`, which Microsoft documents
//! as "should be granted only to administrators", so writes are only ever
//! executed by the elevated helper (or an already-elevated process) — see
//! `helper_client` and `backend`.

use serde::{Deserialize, Serialize};

use super::policy::ServiceClassification;
#[cfg(windows)]
use super::policy::classify_service_with_path;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceInfo {
    pub name: String,
    pub display_name: String,
    pub state: String,
    pub start_type: String,
    pub delayed_auto_start: bool,
    pub pid: Option<u32>,
    /// Services this one needs (from `QueryServiceConfig`, groups removed).
    pub dependencies: Vec<String>,
    /// Services that need this one (derived from every service's dependencies).
    #[serde(default)]
    pub dependents: Vec<String>,
    pub classification: ServiceClassification,
    pub can_change: bool,
    /// Runs in its own process, so `memory_mb` is attributable to it.
    #[serde(default)]
    pub own_process: bool,
    /// Has SCM start triggers (device arrival, network, ETW…): `Manual` plus a
    /// trigger is the idiomatic "do not run at boot" state.
    #[serde(default)]
    pub trigger_start: bool,
    /// Working set of the service process; only reported for own-process services.
    #[serde(default)]
    pub memory_mb: Option<f64>,
    #[serde(default)]
    pub binary_path: Option<String>,
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
    /// How much work the start type causes at boot: 0 disabled, 1 manual,
    /// 2 delayed, 3 automatic. Recommendations only ever lower this.
    pub fn startup_cost(&self) -> u8 {
        match self {
            StartType::Disabled => 0,
            StartType::Manual => 1,
            StartType::AutomaticDelayed => 2,
            StartType::Automatic => 3,
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

impl ServiceInfo {
    /// The current start type as an enum, when it is one WinClean can restore.
    pub fn current_start_type(&self) -> Option<StartType> {
        StartType::parse(&self.start_type)
    }
}

#[cfg(windows)]
mod win {
    use super::*;
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_INSUFFICIENT_BUFFER, ERROR_MORE_DATA, HANDLE};
    use windows_sys::Win32::System::Services::*;

    const SERVICE_CONFIG_TRIGGER_INFO_LEVEL: u32 = 8;

    /// Reads a NUL-terminated UTF-16 string starting at `ptr`, never
    /// reading at or past `buf_end`. Pointers from the SCM enumeration
    /// reference into `buf`; without the bound a short tail string would
    /// make a fixed-length slice read out of bounds (UB).
    unsafe fn wide_bounded(ptr: *const u16, buf_end: *const u16) -> String {
        if ptr.is_null() || ptr >= buf_end {
            return String::new();
        }
        let max = (buf_end as usize - ptr as usize) / std::mem::size_of::<u16>();
        let mut len = 0usize;
        while len < max && unsafe { *ptr.add(len) } != 0 {
            len += 1;
        }
        wide(unsafe { std::slice::from_raw_parts(ptr, len) })
    }

    fn wide(bytes: &[u16]) -> String {
        let end = bytes.iter().position(|&c| c == 0).unwrap_or(bytes.len());
        OsString::from_wide(&bytes[..end]).to_string_lossy().into_owned()
    }
    fn state_str(v: u32) -> String {
        match v {
            1 => "stopped",
            2 => "startPending",
            3 => "stopPending",
            4 => "running",
            5 => "continuePending",
            6 => "pausePending",
            7 => "paused",
            _ => "unknown",
        }
        .to_string()
    }
    fn start_str(v: u32) -> &'static str {
        match v {
            0 => "boot",
            1 => "system",
            2 => "automatic",
            3 => "manual",
            4 => "disabled",
            _ => "unknown",
        }
    }

    struct Scm(SC_HANDLE);
    impl Drop for Scm {
        fn drop(&mut self) {
            unsafe {
                CloseServiceHandle(self.0);
            }
        }
    }

    fn open_scm(access: u32) -> Result<Scm, String> {
        let h = unsafe { OpenSCManagerW(std::ptr::null(), std::ptr::null(), access) };
        if h.is_null() {
            return Err(format!(
                "OpenSCManager failed ({}). Reading services needs no elevation; changing them needs the administrator helper.",
                std::io::Error::last_os_error()
            ));
        }
        Ok(Scm(h))
    }
    fn open_service(scm: &Scm, name: &str, access: u32) -> Result<Scm, String> {
        let w: Vec<u16> = name.encode_utf16().chain([0]).collect();
        let h = unsafe { OpenServiceW(scm.0, w.as_ptr(), access) };
        if h.is_null() {
            return Err(format!("OpenService({name}) failed: {}", std::io::Error::last_os_error()));
        }
        Ok(Scm(h))
    }

    struct Config {
        start_type: String,
        delayed: bool,
        deps: Vec<String>,
        binary_path: Option<String>,
        trigger_start: bool,
    }

    fn query_config(svc: &Scm) -> Config {
        let unknown = Config { start_type: "unknown".into(), delayed: false, deps: Vec::new(), binary_path: None, trigger_start: false };
        let mut needed = 0u32;
        unsafe {
            QueryServiceConfigW(svc.0, std::ptr::null_mut(), 0, &mut needed);
        }
        if needed == 0 {
            return unknown;
        }
        // u32-backed buffer keeps QUERY_SERVICE_CONFIGW aligned.
        let mut buf = vec![0u32; (needed as usize).div_ceil(4)];
        let ok = unsafe { QueryServiceConfigW(svc.0, buf.as_mut_ptr() as *mut QUERY_SERVICE_CONFIGW, needed, &mut needed) };
        if ok == 0 {
            return unknown;
        }
        let cfg = unsafe { &*(buf.as_ptr() as *const QUERY_SERVICE_CONFIGW) };
        let buf_end = unsafe { (buf.as_ptr() as *const u8).add(buf.len() * 4) } as *const u16;
        let mut deps = Vec::new();
        if !cfg.lpDependencies.is_null() {
            // Double-null terminated multi-string; entries starting with '+'
            // are load-order *groups*, not services.
            let mut p = cfg.lpDependencies as *const u16;
            loop {
                if p >= buf_end {
                    break;
                }
                let mut len = 0usize;
                while unsafe { p.add(len) } < buf_end && unsafe { *p.add(len) } != 0 {
                    len += 1;
                }
                if len == 0 {
                    break;
                }
                let dep = wide(unsafe { std::slice::from_raw_parts(p, len) });
                if !dep.starts_with('+') {
                    deps.push(dep);
                }
                p = unsafe { p.add(len + 1) };
            }
        }
        let binary_path = if cfg.lpBinaryPathName.is_null() {
            None
        } else {
            Some(unsafe { wide_bounded(cfg.lpBinaryPathName, buf_end) }).filter(|s| !s.is_empty())
        };
        // Delayed auto-start flag (SERVICE_CONFIG_DELAYED_AUTO_START_INFO = 3).
        let mut info = SERVICE_DELAYED_AUTO_START_INFO { fDelayedAutostart: 0 };
        let mut n2 = 0u32;
        let ok2 = unsafe {
            QueryServiceConfig2W(
                svc.0,
                SERVICE_CONFIG_DELAYED_AUTO_START_INFO,
                &mut info as *mut _ as *mut u8,
                std::mem::size_of::<SERVICE_DELAYED_AUTO_START_INFO>() as u32,
                &mut n2,
            )
        };
        let delayed = ok2 != 0 && info.fDelayedAutostart != 0;
        let mut start_type = start_str(cfg.dwStartType).to_string();
        if start_type == "automatic" && delayed {
            start_type = "automaticDelayed".into();
        }
        Config { start_type, delayed, deps, binary_path, trigger_start: has_triggers(svc) }
    }

    /// True when the service has at least one SCM start trigger.
    fn has_triggers(svc: &Scm) -> bool {
        let mut needed = 0u32;
        let ok = unsafe { QueryServiceConfig2W(svc.0, SERVICE_CONFIG_TRIGGER_INFO_LEVEL, std::ptr::null_mut(), 0, &mut needed) };
        let err = std::io::Error::last_os_error().raw_os_error().unwrap_or(0) as u32;
        if ok != 0 || err != ERROR_INSUFFICIENT_BUFFER || needed == 0 {
            return false;
        }
        let mut buf = vec![0u32; (needed as usize).div_ceil(4)];
        let ok = unsafe { QueryServiceConfig2W(svc.0, SERVICE_CONFIG_TRIGGER_INFO_LEVEL, buf.as_mut_ptr() as *mut u8, needed, &mut needed) };
        if ok == 0 {
            return false;
        }
        // SERVICE_TRIGGER_INFO begins with DWORD cTriggers.
        buf[0] > 0
    }

    fn to_info(name: String, display: String, state: u32, pid: u32, service_type: u32, cfg: Config) -> ServiceInfo {
        let classification = classify_service_with_path(&name, &display, cfg.binary_path.as_deref());
        let own_process = service_type & SERVICE_WIN32_OWN_PROCESS != 0;
        let can_change = classification.risk != super::super::policy::ServiceRisk::Protected;
        let memory_mb = if own_process && pid != 0 { super::super::processes::working_set_mb(pid) } else { None };
        ServiceInfo {
            can_change,
            classification,
            name,
            display_name: display,
            state: state_str(state),
            start_type: cfg.start_type,
            delayed_auto_start: cfg.delayed,
            pid: (pid != 0).then_some(pid),
            dependencies: cfg.deps,
            dependents: Vec::new(),
            own_process,
            trigger_start: cfg.trigger_start,
            memory_mb,
            binary_path: cfg.binary_path,
        }
    }

    pub fn list_services_impl() -> Result<Vec<ServiceInfo>, String> {
        let scm = open_scm(SC_MANAGER_ENUMERATE_SERVICE | SC_MANAGER_CONNECT)?;
        let mut bytes_needed = 0u32;
        let mut count = 0u32;
        let mut resume: u32 = 0;
        unsafe {
            EnumServicesStatusExW(scm.0, SC_ENUM_PROCESS_INFO, SERVICE_WIN32, SERVICE_STATE_ALL, std::ptr::null_mut(), 0, &mut bytes_needed, &mut count, &mut resume, std::ptr::null())
        };
        if bytes_needed == 0 {
            return Ok(Vec::new());
        }
        // Services can start between the sizing call and the real call; the
        // buffer gets some headroom and a short read is retried once.
        let mut capacity = bytes_needed as usize + 4096;
        let mut buf;
        let mut attempts = 0;
        loop {
            buf = vec![0u64; capacity.div_ceil(8)];
            resume = 0;
            let ok = unsafe {
                EnumServicesStatusExW(scm.0, SC_ENUM_PROCESS_INFO, SERVICE_WIN32, SERVICE_STATE_ALL, buf.as_mut_ptr() as *mut u8, (buf.len() * 8) as u32, &mut bytes_needed, &mut count, &mut resume, std::ptr::null())
            };
            let err = std::io::Error::last_os_error().raw_os_error();
            if ok != 0 {
                break;
            }
            if err == Some(ERROR_MORE_DATA as i32) && attempts < 2 {
                capacity = bytes_needed as usize + 8192;
                attempts += 1;
                continue;
            }
            return Err(format!("EnumServicesStatusEx failed: {}", std::io::Error::last_os_error()));
        }
        let mut out = Vec::with_capacity(count as usize);
        let base = buf.as_ptr() as *const ENUM_SERVICE_STATUS_PROCESSW;
        let buf_end = unsafe { (buf.as_ptr() as *const u8).add(buf.len() * 8) } as *const u16;
        for i in 0..count as usize {
            let e = unsafe { &*base.add(i) };
            let name = unsafe { wide_bounded(e.lpServiceName, buf_end) };
            let display = if e.lpDisplayName.is_null() { name.clone() } else { unsafe { wide_bounded(e.lpDisplayName, buf_end) } };
            // Per-service config needs a handle; failures degrade to unknown.
            let cfg = match open_service(&scm, &name, SERVICE_QUERY_CONFIG) {
                Ok(svc) => query_config(&svc),
                Err(_) => Config { start_type: "unknown".into(), delayed: false, deps: Vec::new(), binary_path: None, trigger_start: false },
            };
            out.push(to_info(
                name,
                display,
                e.ServiceStatusProcess.dwCurrentState,
                e.ServiceStatusProcess.dwProcessId,
                e.ServiceStatusProcess.dwServiceType,
                cfg,
            ));
        }
        out.sort_by(|a, b| a.display_name.to_lowercase().cmp(&b.display_name.to_lowercase()));
        Ok(out)
    }

    pub fn set_start_type_impl(name: &str, target: StartType) -> Result<(), String> {
        let scm = open_scm(SC_MANAGER_CONNECT)?;
        let svc = open_service(&scm, name, SERVICE_CHANGE_CONFIG | SERVICE_QUERY_CONFIG)?;
        let ok = unsafe {
            ChangeServiceConfigW(
                svc.0,
                SERVICE_NO_CHANGE,
                target.scm_value(),
                SERVICE_NO_CHANGE,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
            )
        };
        if ok == 0 {
            return Err(format!(
                "ChangeServiceConfig({name}) failed: {}. Changing a service requires the administrator helper.",
                std::io::Error::last_os_error()
            ));
        }
        // The delayed flag is only meaningful for auto-start services and is
        // always written explicitly so a Manual→Automatic switch never keeps a
        // stale delayed flag.
        if target == StartType::AutomaticDelayed || target == StartType::Automatic {
            let mut info = SERVICE_DELAYED_AUTO_START_INFO { fDelayedAutostart: if target == StartType::AutomaticDelayed { 1 } else { 0 } };
            let ok2 = unsafe { ChangeServiceConfig2W(svc.0, SERVICE_CONFIG_DELAYED_AUTO_START_INFO, &mut info as *mut _ as *const std::ffi::c_void) };
            if ok2 == 0 {
                return Err(format!(
                    "ChangeServiceConfig2W(delayedStart, {name}) failed: {}. The start type was changed but the delayed flag was not; re-read the service before retrying.",
                    std::io::Error::last_os_error()
                ));
            }
        }
        let _ = CloseHandle as unsafe extern "system" fn(HANDLE) -> i32;
        Ok(())
    }
}

#[cfg(windows)]
pub fn list_services() -> Result<Vec<ServiceInfo>, String> {
    let mut services = win::list_services_impl()?;
    super::service_graph::finalize(&mut services);
    Ok(services)
}
#[cfg(not(windows))]
pub fn list_services() -> Result<Vec<ServiceInfo>, String> {
    Err("Windows Service optimization is only available on Windows.".into())
}

/// Writes the start type. Requires administrator rights; production callers go
/// through the `ServiceBackend` so the write is routed to the elevated helper.
#[cfg(windows)]
pub fn set_service_start_type(name: &str, target: StartType) -> Result<(), String> {
    win::set_start_type_impl(name, target)
}
#[cfg(not(windows))]
pub fn set_service_start_type(_name: &str, _target: StartType) -> Result<(), String> {
    Err("Windows Service optimization is only available on Windows.".into())
}

/// Per-service payload stored in a snapshot (for rollback).
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_cost_orders_the_start_types() {
        assert!(StartType::Automatic.startup_cost() > StartType::AutomaticDelayed.startup_cost());
        assert!(StartType::AutomaticDelayed.startup_cost() > StartType::Manual.startup_cost());
        assert!(StartType::Manual.startup_cost() > StartType::Disabled.startup_cost());
    }

    #[test]
    fn snapshot_payload_round_trips_every_restorable_start_type() {
        for (start, delayed, expect) in [
            ("automatic", false, StartType::Automatic),
            ("automatic", true, StartType::AutomaticDelayed),
            ("automaticDelayed", true, StartType::AutomaticDelayed),
            ("manual", false, StartType::Manual),
            ("disabled", false, StartType::Disabled),
        ] {
            let v = serde_json::json!({ "startType": start, "delayedAutoStart": delayed });
            assert_eq!(start_type_from_snapshot(&v), Some(expect), "{start}/{delayed}");
        }
        // boot/system/unknown are not restorable, so a change is refused up front.
        for start in ["boot", "system", "unknown"] {
            assert_eq!(start_type_from_snapshot(&serde_json::json!({ "startType": start })), None);
        }
    }
}
