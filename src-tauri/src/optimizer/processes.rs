//! Process enumeration + EcoQoS via SetProcessInformation.
//! WinClean never terminates processes; EcoQoS is fully reversible.

use serde::{Deserialize, Serialize};

use super::policy::{classify_process_for_ecoqos, EcoQosDecision};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessInfo {
    pub pid: u32,
    pub name: String,
    pub working_set_mb: f64,
    pub cpu_percent_recent: f32,
    pub is_foreground: bool,
    pub eco_decision: EcoQosDecision,
    pub eco_qos_applied: bool,
}

#[cfg(windows)]
mod win {
    use super::*;
    use std::collections::HashSet;
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, HANDLE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
    };
    use windows_sys::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, GetProcessTimes, OpenProcess, QueryFullProcessImageNameW,
        SetProcessInformation, PROCESS_ACCESS_RIGHTS, PROCESS_POWER_THROTTLING_CURRENT_VERSION,
        PROCESS_POWER_THROTTLING_EXECUTION_SPEED, PROCESS_POWER_THROTTLING_STATE,
        ProcessPowerThrottling,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

    /// Return the foreground window’s process ID, or zero when no foreground window exists.
    fn foreground_pid() -> u32 {
        unsafe {
            let hwnd = GetForegroundWindow();
            if hwnd.is_null() { return 0; }
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, &mut pid);
            pid
        }
    }

    /// Combine a Windows FILETIME into a 64-bit count of 100-nanosecond ticks.
    fn filetime_ticks(ft: &FILETIME) -> u64 {
        ((ft.dwHighDateTime as u64) << 32) | ft.dwLowDateTime as u64
    }

    /// Sample Windows processes for memory and recent CPU usage, then classify EcoQoS eligibility.
    /// Return at most 300 entries ordered by working-set size; inaccessible metrics remain zero.
    pub fn list_processes_impl() -> Result<Vec<ProcessInfo>, String> {
        let fg = foreground_pid();
        let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
        if snap.is_null() { return Err(format!("CreateToolhelp32Snapshot failed: {}", std::io::Error::last_os_error())); }
        let mut out = Vec::new();
        let mut entry = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, cntUsage: 0, th32ProcessID: 0, th32DefaultHeapID: 0, th32ModuleID: 0, cntThreads: 0, th32ParentProcessID: 0, pcPriClassBase: 0, dwFlags: 0, szExeFile: [0; 260] };
        // Two samples 250 ms apart for a cheap, bounded CPU estimate.
        let mut first: std::collections::HashMap<u32, u64> = std::collections::HashMap::new();
        let sample = |map: &mut std::collections::HashMap<u32, u64>| {
            let s = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
            if s.is_null() { return; }
            let mut e = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, cntUsage: 0, th32ProcessID: 0, th32DefaultHeapID: 0, th32ModuleID: 0, cntThreads: 0, th32ParentProcessID: 0, pcPriClassBase: 0, dwFlags: 0, szExeFile: [0; 260] };
            if unsafe { Process32FirstW(s, &mut e) } != 0 {
                loop {
                    let h = unsafe { OpenProcess(0x1000 /* PROCESS_QUERY_LIMITED_INFORMATION */, 0, e.th32ProcessID) };
                    if !h.is_null() {
                        let (mut c, mut u, mut k, mut x) = (FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 }, FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 }, FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 }, FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 });
                        if unsafe { GetProcessTimes(h, &mut c, &mut x, &mut k, &mut u) } != 0 {
                            map.insert(e.th32ProcessID, filetime_ticks(&k) + filetime_ticks(&u));
                        }
                        unsafe { CloseHandle(h); }
                    }
                    if unsafe { Process32NextW(s, &mut e) } == 0 { break; }
                }
            }
            unsafe { CloseHandle(s); }
        };
        sample(&mut first);
        std::thread::sleep(std::time::Duration::from_millis(250));

        if unsafe { Process32FirstW(snap, &mut entry) } != 0 {
            loop {
                let pid = entry.th32ProcessID;
                let name = {
                    let s = String::from_utf16_lossy(&entry.szExeFile);
                    s.trim_end_matches('\0').to_string()
                };
                let mut ws_mb = 0.0f64;
                let mut cpu = 0.0f32;
                let h = unsafe { OpenProcess(0x1000 | 0x0400 /* QUERY_INFORMATION|VM_READ-ish limited */, 0, pid) };
                if !h.is_null() {
                    let mut counters: PROCESS_MEMORY_COUNTERS = unsafe { std::mem::zeroed() };
                    counters.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
                    if unsafe { K32GetProcessMemoryInfo(h, &mut counters, counters.cb) } != 0 {
                        ws_mb = counters.WorkingSetSize as f64 / (1024.0 * 1024.0);
                    }
                    let (mut c, mut u, mut k, mut x) = (FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 }, FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 }, FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 }, FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 });
                    if unsafe { GetProcessTimes(h, &mut c, &mut x, &mut k, &mut u) } != 0 {
                        if let Some(prev) = first.get(&pid) {
                            let delta = (filetime_ticks(&k) + filetime_ticks(&u)).saturating_sub(*prev);
                            // 250 ms window, 100 ns ticks; % of one core.
                            cpu = (delta as f32 / 2_500_000.0) * 100.0;
                        }
                    }
                    unsafe { CloseHandle(h); }
                }
                let is_fg = pid == fg && pid != 0;
                out.push(ProcessInfo {
                    eco_decision: classify_process_for_ecoqos(&name, is_fg, cpu),
                    pid, name, working_set_mb: (ws_mb * 10.0).round() / 10.0,
                    cpu_percent_recent: (cpu * 10.0).round() / 10.0,
                    is_foreground: is_fg, eco_qos_applied: false,
                });
                if unsafe { Process32NextW(snap, &mut entry) } == 0 { break; }
            }
        }
        unsafe { CloseHandle(snap); }
        let _ = QueryFullProcessImageNameW;
        let _ = GetCurrentProcess;
        let _seen: HashSet<u32> = HashSet::new();
        out.sort_by(|a, b| b.working_set_mb.partial_cmp(&a.working_set_mb).unwrap_or(std::cmp::Ordering::Equal));
        out.truncate(300);
        Ok(out)
    }

    /// Set or release process execution-speed throttling through SetProcessInformation.
    /// Return access or Windows API failures; policy checks belong to the command layer.
    pub fn set_ecoqos_impl(pid: u32, enable: bool) -> Result<(), String> {
        // PROCESS_SET_INFORMATION = 0x0200, PROCESS_QUERY_LIMITED_INFORMATION = 0x1000
        let access: PROCESS_ACCESS_RIGHTS = 0x0200 | 0x1000;
        let h: HANDLE = unsafe { OpenProcess(access, 0, pid) };
        if h.is_null() {
            return Err(format!("OpenProcess({pid}) failed: {}. EcoQoS on system/other-user processes needs elevation; WinClean never forces it.", std::io::Error::last_os_error()));
        }
        let mut state = PROCESS_POWER_THROTTLING_STATE {
            Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
            ControlMask: if enable { PROCESS_POWER_THROTTLING_EXECUTION_SPEED } else { 0 },
            StateMask: if enable { PROCESS_POWER_THROTTLING_EXECUTION_SPEED } else { 0 },
        };
        let ok = unsafe { SetProcessInformation(h, ProcessPowerThrottling, &mut state as *mut _ as *mut std::ffi::c_void, std::mem::size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32) };
        unsafe { CloseHandle(h); }
        if ok == 0 {
            return Err(format!("SetProcessInformation(EcoQoS, pid={pid}) failed: {}", std::io::Error::last_os_error()));
        }
        Ok(())
    }
}

/// Return sampled Windows processes with resource usage and EcoQoS classification.
#[cfg(windows)]
pub fn list_processes() -> Result<Vec<ProcessInfo>, String> { win::list_processes_impl() }
/// Report that process monitoring is unavailable outside Windows.
#[cfg(not(windows))]
pub fn list_processes() -> Result<Vec<ProcessInfo>, String> {
    Err("Process monitoring is only available on Windows.".into())
}

/// Set or release EcoQoS for a Windows PID; the caller must enforce eligibility policy.
#[cfg(windows)]
pub fn set_process_ecoqos(pid: u32, enable: bool) -> Result<(), String> { win::set_ecoqos_impl(pid, enable) }
/// Report that EcoQoS is unavailable outside Windows.
#[cfg(not(windows))]
pub fn set_process_ecoqos(_pid: u32, _enable: bool) -> Result<(), String> {
    Err("EcoQoS is only available on Windows.".into())
}
