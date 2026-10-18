//! ASUS G16 hardware discovery — registry / CPUID strings ONLY.
//!
//! Deliberately absent: CUDA, NVAPI, DXGI adapter enumeration, WMI GPU
//! queries and any periodic GPU polling. All of those can power up the
//! RTX 4070. Display adapters are read from the registry Display class
//! key, which Windows keeps without waking the device.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HardwareInfo {
    pub cpu_name: String,
    pub cpu_is_ryzen_ai_9_hx370: bool,
    pub cpu_architecture_note: String,
    pub logical_processors: u32,
    pub ram_total_mb: Option<u64>,
    pub adapters: Vec<DisplayAdapter>,
    pub npu: NpuInfo,
    pub ghelper_detected: bool,
    pub dgpu_policy: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DisplayAdapter {
    pub name: String,
    pub driver_version: Option<String>,
    pub kind: String, // "igpu" | "dgpu" | "other"
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NpuInfo {
    pub present: bool,
    pub name: Option<String>,
    pub inference_verified: bool,
    pub note: String,
}

/// Classify a display-adapter name as integrated, discrete, or other using known name patterns.
pub fn classify_adapter(name: &str) -> &'static str {
    let n = name.to_ascii_lowercase();
    if n.contains("radeon 890m") || n.contains("radeon(tm) graphics") || (n.contains("amd radeon") && !n.contains("rx ")) {
        "igpu"
    } else if n.contains("nvidia") || n.contains("geforce") || n.contains("rtx") {
        "dgpu"
    } else {
        "other"
    }
}

/// Describe a CPU model string, recognizing Ryzen AI and the expected G16 processor.
pub fn cpu_note(name: &str) -> String {
    let n = name.to_ascii_lowercase();
    if n.contains("ryzen ai 9 hx 370") {
        "AMD Ryzen AI 9 HX 370 (Zen 5 + Zen 5c hybrid, 12 cores) — expected CPU for the ROG Zephyrus G16.".into()
    } else if n.contains("ryzen ai") {
        "AMD Ryzen AI (Zen 5 family) detected.".into()
    } else if n.contains("ryzen") {
        "AMD Ryzen CPU detected.".into()
    } else {
        "CPU model string reported by Windows; no ASUS-specific tuning applied.".into()
    }
}

#[cfg(windows)]
mod win {
    use super::*;
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegEnumKeyExW, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_LOCAL_MACHINE,
        KEY_ENUMERATE_SUB_KEYS, KEY_QUERY_VALUE, KEY_READ,
    };
    use windows_sys::Win32::System::SystemInformation::{
        GetSystemInfo, GlobalMemoryStatusEx, MEMORYSTATUSEX, SYSTEM_INFO,
    };

    /// Read a UTF-16 registry value, returning `None` if the key or value cannot be read.
    fn reg_get_string(root: HKEY, sub: &str, value: &str) -> Option<String> {
        let subw: Vec<u16> = sub.encode_utf16().chain([0]).collect();
        let valw: Vec<u16> = value.encode_utf16().chain([0]).collect();
        let mut hkey: HKEY = std::ptr::null_mut();
        if unsafe { RegOpenKeyExW(root, subw.as_ptr(), 0, KEY_QUERY_VALUE, &mut hkey) } != 0 { return None; }
        let mut ty = 0u32;
        let mut size = 0u32;
        let rc = unsafe { RegQueryValueExW(hkey, valw.as_ptr(), std::ptr::null_mut(), &mut ty, std::ptr::null_mut(), &mut size) };
        let mut out = None;
        if rc == 0 && size > 0 {
            let mut buf = vec![0u16; (size as usize / 2).max(1)];
            let rc2 = unsafe { RegQueryValueExW(hkey, valw.as_ptr(), std::ptr::null_mut(), &mut ty, buf.as_mut_ptr() as *mut u8, &mut size) };
            if rc2 == 0 {
                let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
                out = Some(String::from_utf16_lossy(&buf[..end]));
            }
        }
        unsafe { RegCloseKey(hkey); }
        out
    }

    /// Read adapter names and driver versions from the Display registry class without GPU polling.
    fn display_adapters() -> Vec<DisplayAdapter> {
        // HKLM\SYSTEM\CurrentControlSet\Control\Class\{4d36e968-e325-11ce-bfc1-08002be10318}\NNNN
        let base = "SYSTEM\\CurrentControlSet\\Control\\Class\\{4d36e968-e325-11ce-bfc1-08002be10318}";
        let basew: Vec<u16> = base.encode_utf16().chain([0]).collect();
        let mut hkey: HKEY = std::ptr::null_mut();
        if unsafe { RegOpenKeyExW(HKEY_LOCAL_MACHINE, basew.as_ptr(), 0, KEY_READ, &mut hkey) } != 0 {
            return Vec::new();
        }
        let mut out = Vec::new();
        let mut idx = 0u32;
        loop {
            let mut name_buf = [0u16; 64];
            let mut name_len = name_buf.len() as u32;
            let rc = unsafe { RegEnumKeyExW(hkey, idx, name_buf.as_mut_ptr(), &mut name_len, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut()) };
            if rc != 0 { break; }
            let sub = String::from_utf16_lossy(&name_buf[..name_len as usize]);
            if sub.chars().all(|c| c.is_ascii_digit()) && sub.len() == 4 {
                let path = format!("{base}\\{sub}");
                if let Some(desc) = reg_get_string(HKEY_LOCAL_MACHINE, &path, "DriverDesc")
                    .or_else(|| reg_get_string(HKEY_LOCAL_MACHINE, &path, "DeviceDesc"))
                {
                    out.push(DisplayAdapter {
                        kind: classify_adapter(&desc).to_string(),
                        driver_version: reg_get_string(HKEY_LOCAL_MACHINE, &path, "DriverVersion"),
                        name: desc,
                    });
                }
            }
            idx += 1;
            let _ = KEY_ENUMERATE_SUB_KEYS;
        }
        unsafe { RegCloseKey(hkey); }
        out
    }

    /// Discover a matching NPU description in the ComputeAccelerator registry class.
    /// Device presence does not mark inference as verified.
    fn npu_info(adapters_note: &[DisplayAdapter]) -> NpuInfo {
        // NPU appears under the ComputeAccelerator class; scan both class keys by name match.
        let mut found: Option<String> = None;
        for class_guid in ["{f01a9d53-3ff6-48d2-9f97-c8a7004be10c}"] {
            let base = format!("SYSTEM\\CurrentControlSet\\Control\\Class\\{class_guid}");
            let basew: Vec<u16> = base.encode_utf16().chain([0]).collect();
            let mut hkey: HKEY = std::ptr::null_mut();
            if unsafe { RegOpenKeyExW(HKEY_LOCAL_MACHINE, basew.as_ptr(), 0, KEY_READ, &mut hkey) } != 0 { continue; }
            let mut idx = 0u32;
            loop {
                let mut nb = [0u16; 64];
                let mut nl = nb.len() as u32;
                if unsafe { RegEnumKeyExW(hkey, idx, nb.as_mut_ptr(), &mut nl, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut()) } != 0 { break; }
                let sub = String::from_utf16_lossy(&nb[..nl as usize]);
                let path = format!("{base}\\{sub}");
                if let Some(desc) = reg_get_string(HKEY_LOCAL_MACHINE, &path, "DriverDesc") {
                    if desc.to_ascii_lowercase().contains("x-dna") || desc.to_ascii_lowercase().contains("xDNA") || desc.to_ascii_lowercase().contains("npu") || desc.to_ascii_lowercase().contains("ipu") {
                        found = Some(desc);
                        break;
                    }
                }
                idx += 1;
            }
            unsafe { RegCloseKey(hkey); }
        }
        let _ = adapters_note;
        match found {
            Some(name) => NpuInfo {
                present: true,
                name: Some(name),
                inference_verified: false,
                note: "AMD XDNA NPU enumerated from the registry. Inference is NOT enabled until a Windows ML model + VitisAI EP driver check passes on this machine; CPU fallback remains active.".into(),
            },
            None => NpuInfo {
                present: false,
                name: None,
                inference_verified: false,
                note: "No NPU device found in the registry ComputeAccelerator class. Install the AMD XDNA driver from ASUS/AMD to enable Copilot+ NPU features.".into(),
            },
        }
    }

    /// Collect Windows CPU, memory, adapter, NPU, and G-Helper presence information without GPU polling.
    pub fn hardware_impl() -> Result<HardwareInfo, String> {
        let cpu_name = reg_get_string(HKEY_LOCAL_MACHINE, "HARDWARE\\DESCRIPTION\\System\\CentralProcessor\\0", "ProcessorNameString")
            .or_else(|| std::env::var("PROCESSOR_IDENTIFIER").ok())
            .unwrap_or_else(|| "Unknown CPU".into());
        let (logical, ram_mb) = unsafe {
            let mut si: SYSTEM_INFO = std::mem::zeroed();
            GetSystemInfo(&mut si);
            let mut ms = MEMORYSTATUSEX { dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32, dwMemoryLoad: 0, ullTotalPhys: 0, ullAvailPhys: 0, ullTotalPageFile: 0, ullAvailPageFile: 0, ullTotalVirtual: 0, ullAvailVirtual: 0, ullAvailExtendedVirtual: 0 };
            let ram = if GlobalMemoryStatusEx(&mut ms) != 0 { Some(ms.ullTotalPhys / (1024 * 1024)) } else { None };
            (si.dwNumberOfProcessors, ram)
        };
        let adapters = display_adapters();
        let npu = npu_info(&adapters);
        // G-Helper coexistence: detect only; WinClean never writes ASUS firmware/GPU-mode/fan state.
        let ghelper = std::path::Path::new("C:\\Program Files\\GHelper").exists()
            || reg_get_string(HKEY_LOCAL_MACHINE, "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\GHelper", "DisplayName").is_some();
        let is_hx370 = cpu_name.to_ascii_lowercase().contains("ryzen ai 9 hx 370");
        Ok(HardwareInfo {
            cpu_is_ryzen_ai_9_hx370: is_hx370,
            cpu_architecture_note: cpu_note(&cpu_name),
            cpu_name,
            logical_processors: logical,
            ram_total_mb: ram_mb,
            adapters,
            npu,
            ghelper_detected: ghelper,
            dgpu_policy: "RTX 4070 is never woken by WinClean: no CUDA/NVAPI/DXGI polling. It is only listed as a routing target when Windows already reports it awake.".into(),
        })
    }
}

/// Return Windows hardware discovery without activating GPU inference APIs.
#[cfg(windows)]
pub fn hardware_info() -> Result<HardwareInfo, String> { win::hardware_impl() }
/// Report that hardware discovery is unavailable outside Windows.
#[cfg(not(windows))]
pub fn hardware_info() -> Result<HardwareInfo, String> {
    Err("Hardware discovery is only available on Windows.".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn adapter_classification() {
        assert_eq!(classify_adapter("AMD Radeon 890M Graphics"), "igpu");
        assert_eq!(classify_adapter("NVIDIA GeForce RTX 4070 Laptop GPU"), "dgpu");
    }
    #[test]
    fn hx370_note() {
        assert!(cpu_note("AMD Ryzen AI 9 HX 370 w/ Radeon 890M").contains("HX 370"));
    }
}
