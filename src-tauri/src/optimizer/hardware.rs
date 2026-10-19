//! ASUS ROG Zephyrus G16 hardware discovery.
//!
//! Hard rule: nothing here may power up the RTX 4070. Discovery uses the
//! registry, SetupAPI-free PnP *property* reads and `GetLogicalProcessor…`
//! only. Deliberately absent: CUDA, NVAPI/NVML, DXGI adapter enumeration,
//! Direct3D devices, WMI `Win32_VideoController` and any periodic GPU
//! polling. The dGPU's power state comes from the PnP manager's cached
//! `DEVPKEY_Device_PowerData` (the same data Device Manager's "Power Data"
//! tab shows), which involves no device I/O.
//!
//! Everything is lazy: nothing runs until the Hardware view (or the AI
//! assistant's read-only tool) asks, the static part is cached, and nothing
//! polls in the background.

use serde::{Deserialize, Serialize};

use super::topology::CpuTopology;

/// True for a ComputeAccelerator driver description that looks like an NPU
/// (AMD XDNA, or a generic NPU/IPU such as Intel's).
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
fn is_npu_description(desc: &str) -> bool {
    let d = desc.to_ascii_lowercase();
    ["xdna", "x-dna", "npu", "ipu"].iter().any(|needle| d.contains(needle))
}

/// Vendor-accurate label: only an XDNA device is called "AMD XDNA NPU".
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
fn npu_label(desc: &str) -> &'static str {
    let d = desc.to_ascii_lowercase();
    if d.contains("xdna") || d.contains("x-dna") {
        "AMD XDNA NPU"
    } else {
        "NPU"
    }
}

/// AMD NPU generation from the PCI hardware id (`VEN_1022` + device id), using
/// the mapping of AMD's open-source `amdxdna` driver: `1502` is the first
/// generation (Phoenix/Hawk Point), `17F0` the XDNA 2 family (Strix Point and
/// later). Unknown ids are reported as unidentified rather than guessed.
pub fn npu_generation(hardware_id: &str) -> Option<&'static str> {
    let id = hardware_id.to_ascii_lowercase();
    if !id.contains("ven_1022") {
        return None;
    }
    if id.contains("dev_17f0") {
        Some("AMD XDNA 2")
    } else if id.contains("dev_1502") {
        Some("AMD XDNA (1st generation)")
    } else {
        Some("AMD NPU (generation not identified)")
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HardwareInfo {
    pub cpu_name: String,
    pub cpu_is_ryzen_ai_9_hx370: bool,
    pub cpu_architecture_note: String,
    pub logical_processors: u32,
    pub ram_total_mb: Option<u64>,
    pub topology: Option<CpuTopology>,
    pub topology_error: Option<String>,
    pub adapters: Vec<DisplayAdapter>,
    pub dgpu: DgpuState,
    /// The discrete GPU is powered down, disabled or absent: iGPU-only mode.
    pub igpu_only: bool,
    pub npu: NpuInfo,
    pub ghelper_detected: bool,
    pub ghelper: GHelperInfo,
    pub platform: PlatformInfo,
    pub dgpu_policy: String,
    /// What was used to gather this, and what was deliberately never used.
    pub sources: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DisplayAdapter {
    pub name: String,
    pub driver_version: Option<String>,
    pub driver_date: Option<String>,
    pub hardware_id: Option<String>,
    pub kind: String, // "igpu" | "dgpu" | "other"
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DgpuPower {
    /// D0: awake and in use.
    Awake,
    /// D1–D3: the PnP manager reports a low-power state.
    PoweredDown,
    /// Disabled in Device Manager (for example by G-Helper's Eco mode).
    Disabled,
    Absent,
    /// Could not be determined. Never treated as awake *or* asleep.
    Unknown,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DgpuState {
    pub present: bool,
    pub power: DgpuPower,
    pub note: String,
}

impl DgpuState {
    /// Only a confirmed D0 counts as awake for inference routing.
    pub fn awake(&self) -> bool {
        self.power == DgpuPower::Awake
    }
}

/// Maps the PnP manager's answers to a [`DgpuPower`].
/// `device_power_state`: 1 = D0 … 4 = D3 (`DEVICE_POWER_STATE`).
pub fn dgpu_power_from_pnp(located: bool, problem: Option<u32>, device_power_state: Option<u32>) -> DgpuPower {
    if !located {
        return DgpuPower::Absent;
    }
    if problem == Some(22) {
        // CM_PROB_DISABLED
        return DgpuPower::Disabled;
    }
    match device_power_state {
        Some(1) => DgpuPower::Awake,
        Some(2..=4) => DgpuPower::PoweredDown,
        _ => DgpuPower::Unknown,
    }
}

#[cfg_attr(not(any(windows, test)), allow(dead_code))]
fn dgpu_note(power: DgpuPower) -> &'static str {
    match power {
        DgpuPower::Awake => "The RTX 4070 is awake (D0). WinClean did not wake it; it only read the PnP power state.",
        DgpuPower::PoweredDown => "The RTX 4070 is powered down. WinClean works in iGPU-only mode and will not touch it.",
        DgpuPower::Disabled => "The RTX 4070 is disabled in Device Manager (for example G-Helper Eco mode). iGPU-only mode.",
        DgpuPower::Absent => "No discrete NVIDIA GPU is enumerated. iGPU-only mode.",
        DgpuPower::Unknown => "The RTX 4070 power state could not be read. WinClean treats it as unavailable for inference and never wakes it to find out.",
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NpuInfo {
    pub present: bool,
    pub name: Option<String>,
    pub generation: Option<String>,
    pub driver_version: Option<String>,
    pub driver_date: Option<String>,
    pub hardware_id: Option<String>,
    pub inference_verified: bool,
    pub note: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GHelperInfo {
    pub installed: bool,
    pub running: bool,
    /// What WinClean does about it.
    pub coexistence: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlatformInfo {
    pub manufacturer: Option<String>,
    pub product_name: Option<String>,
    pub bios_version: Option<String>,
    pub is_asus: bool,
    pub is_zephyrus_g16: bool,
}

pub fn platform_from_dmi(manufacturer: Option<String>, product: Option<String>, bios: Option<String>) -> PlatformInfo {
    let is_asus = manufacturer.as_deref().map(|m| m.to_ascii_lowercase().contains("asus")).unwrap_or(false);
    let is_zephyrus_g16 = is_asus && product.as_deref().map(|p| p.to_ascii_lowercase().contains("zephyrus g16")).unwrap_or(false);
    PlatformInfo { manufacturer, product_name: product, bios_version: bios, is_asus, is_zephyrus_g16 }
}

pub fn ghelper_coexistence(info: &GHelperInfo) -> String {
    if info.running {
        "G-Helper is running. WinClean never writes ASUS firmware, fan, GPU-mode or battery-limit settings, and automatic profile switching pauses so the two tools do not fight over the Windows power plan.".into()
    } else if info.installed {
        "G-Helper is installed but not running. WinClean still never writes ASUS firmware, fan, GPU-mode or battery-limit settings.".into()
    } else {
        "G-Helper was not detected. WinClean still never writes ASUS firmware, fan, GPU-mode or battery-limit settings.".into()
    }
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
        "AMD Ryzen AI 9 HX 370 (12 cores, 24 threads; Zen 5 + Zen 5c) — expected CPU for the ROG Zephyrus G16.".into()
    } else if n.contains("ryzen ai") {
        "AMD Ryzen AI (Zen 5 family) detected.".into()
    } else if n.contains("ryzen") {
        "AMD Ryzen CPU detected.".into()
    } else {
        "CPU model string reported by Windows; no ASUS-specific tuning applied.".into()
    }
}

pub const DGPU_POLICY: &str = "WinClean never initializes CUDA, NVAPI/NVML, DXGI, Direct3D or WMI GPU queries and never polls a GPU, so it cannot wake the RTX 4070. The dGPU is a routing target only while Windows already reports it awake.";

#[cfg(windows)]
mod win {
    use super::*;
    use std::sync::{Mutex, OnceLock};
    use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
        CM_Get_DevNode_PropertyW, CM_Get_DevNode_Status, CM_Get_Device_ID_ListW, CM_Get_Device_ID_List_SizeW, CM_Locate_DevNodeW, CM_GETIDLIST_FILTER_ENUMERATOR, CM_LOCATE_DEVNODE_NORMAL,
    };
    use windows_sys::Win32::Devices::Properties::DEVPKEY_Device_PowerData;
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegEnumKeyExW, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_LOCAL_MACHINE, KEY_QUERY_VALUE, KEY_READ,
    };
    use windows_sys::Win32::System::SystemInformation::{GetSystemInfo, GlobalMemoryStatusEx, MEMORYSTATUSEX, SYSTEM_INFO};

    const DISPLAY_CLASS: &str = "{4d36e968-e325-11ce-bfc1-08002be10318}";
    const COMPUTE_ACCELERATOR_CLASS: &str = "{f01a9d53-3ff6-48d2-9f97-c8a7004be10c}";

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain([0]).collect()
    }

    /// Read a registry value as a string (REG_SZ or REG_MULTI_SZ, joined by `|`).
    fn reg_get_string(root: HKEY, sub: &str, value: &str) -> Option<String> {
        let subw = wide(sub);
        let valw = wide(value);
        let mut hkey: HKEY = std::ptr::null_mut();
        if unsafe { RegOpenKeyExW(root, subw.as_ptr(), 0, KEY_QUERY_VALUE, &mut hkey) } != 0 {
            return None;
        }
        let mut ty = 0u32;
        let mut size = 0u32;
        let rc = unsafe { RegQueryValueExW(hkey, valw.as_ptr(), std::ptr::null_mut(), &mut ty, std::ptr::null_mut(), &mut size) };
        let mut out = None;
        if rc == 0 && size > 0 {
            let mut buf = vec![0u16; (size as usize).div_ceil(2).max(1)];
            let rc2 = unsafe { RegQueryValueExW(hkey, valw.as_ptr(), std::ptr::null_mut(), &mut ty, buf.as_mut_ptr() as *mut u8, &mut size) };
            if rc2 == 0 {
                let text = String::from_utf16_lossy(&buf[..(size as usize / 2).min(buf.len())]);
                let parts: Vec<&str> = text.split('\0').filter(|p| !p.is_empty()).collect();
                if !parts.is_empty() {
                    out = Some(parts.join("|"));
                }
            }
        }
        unsafe { RegCloseKey(hkey) };
        out
    }

    fn reg_subkeys(root: HKEY, sub: &str) -> Vec<String> {
        let subw = wide(sub);
        let mut hkey: HKEY = std::ptr::null_mut();
        if unsafe { RegOpenKeyExW(root, subw.as_ptr(), 0, KEY_READ, &mut hkey) } != 0 {
            return Vec::new();
        }
        let mut out = Vec::new();
        let mut idx = 0u32;
        loop {
            let mut name_buf = [0u16; 128];
            let mut name_len = name_buf.len() as u32;
            let rc = unsafe { RegEnumKeyExW(hkey, idx, name_buf.as_mut_ptr(), &mut name_len, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut()) };
            if rc != 0 {
                break;
            }
            out.push(String::from_utf16_lossy(&name_buf[..name_len as usize]));
            idx += 1;
        }
        unsafe { RegCloseKey(hkey) };
        out
    }

    fn class_entries(class_guid: &str) -> Vec<String> {
        let base = format!("SYSTEM\\CurrentControlSet\\Control\\Class\\{class_guid}");
        reg_subkeys(HKEY_LOCAL_MACHINE, &base).into_iter().filter(|s| s.len() == 4 && s.chars().all(|c| c.is_ascii_digit())).map(|s| format!("{base}\\{s}")).collect()
    }

    fn display_adapters() -> Vec<DisplayAdapter> {
        class_entries(DISPLAY_CLASS)
            .into_iter()
            .filter_map(|path| {
                let desc = reg_get_string(HKEY_LOCAL_MACHINE, &path, "DriverDesc").or_else(|| reg_get_string(HKEY_LOCAL_MACHINE, &path, "DeviceDesc"))?;
                Some(DisplayAdapter {
                    kind: classify_adapter(&desc).to_string(),
                    driver_version: reg_get_string(HKEY_LOCAL_MACHINE, &path, "DriverVersion"),
                    driver_date: reg_get_string(HKEY_LOCAL_MACHINE, &path, "DriverDate"),
                    hardware_id: reg_get_string(HKEY_LOCAL_MACHINE, &path, "MatchingDeviceId"),
                    name: desc,
                })
            })
            .collect()
    }

    fn npu_info() -> NpuInfo {
        let mut found: Option<(String, String)> = None;
        for path in class_entries(COMPUTE_ACCELERATOR_CLASS) {
            if let Some(desc) = reg_get_string(HKEY_LOCAL_MACHINE, &path, "DriverDesc") {
                let hw = reg_get_string(HKEY_LOCAL_MACHINE, &path, "MatchingDeviceId").unwrap_or_default();
                if is_npu_description(&desc) || npu_generation(&hw).is_some() {
                    found = Some((desc, path));
                    break;
                }
            }
        }
        match found {
            Some((name, path)) => {
                let hw = reg_get_string(HKEY_LOCAL_MACHINE, &path, "MatchingDeviceId");
                let generation = hw.as_deref().and_then(npu_generation).map(|s| s.to_string());
                NpuInfo {
                    present: true,
                    note: format!("{} enumerated from the registry. Inference is NOT enabled until a Windows ML model + execution-provider session test passes on this machine; CPU fallback remains active.", generation.as_deref().unwrap_or_else(|| npu_label(&name))),
                    generation,
                    driver_version: reg_get_string(HKEY_LOCAL_MACHINE, &path, "DriverVersion"),
                    driver_date: reg_get_string(HKEY_LOCAL_MACHINE, &path, "DriverDate"),
                    hardware_id: hw,
                    name: Some(name),
                    inference_verified: false,
                }
            }
            None => NpuInfo {
                present: false,
                name: None,
                generation: None,
                driver_version: None,
                driver_date: None,
                hardware_id: None,
                inference_verified: false,
                note: "No NPU device found in the registry ComputeAccelerator class. Install the AMD NPU (IPU/XDNA) driver from ASUS/AMD to enable Copilot+ NPU features.".into(),
            },
        }
    }

    /// Reads the discrete GPU's power state from the PnP manager. Never opens
    /// the device and never calls a GPU API.
    pub fn dgpu_state(adapters: &[DisplayAdapter]) -> DgpuState {
        let registry_says_present = adapters.iter().any(|a| a.kind == "dgpu");
        let make = |power: DgpuPower| DgpuState { present: power != DgpuPower::Absent && (registry_says_present || power != DgpuPower::Unknown), power, note: dgpu_note(power).to_string() };
        let Some(instance) = nvidia_display_instance_id() else {
            return make(if registry_says_present { DgpuPower::Unknown } else { DgpuPower::Absent });
        };
        let id_w = wide(&instance);
        let mut devinst = 0u32;
        let located = unsafe { CM_Locate_DevNodeW(&mut devinst, id_w.as_ptr(), CM_LOCATE_DEVNODE_NORMAL) } == 0;
        if !located {
            return make(DgpuPower::Absent);
        }
        let (mut status, mut problem) = (0u32, 0u32);
        let problem = (unsafe { CM_Get_DevNode_Status(&mut status, &mut problem, devinst, 0) } == 0).then_some(problem);
        let mut ty = 0u32;
        let mut buf = [0u8; 128];
        let mut len = buf.len() as u32;
        let ok = unsafe { CM_Get_DevNode_PropertyW(devinst, &DEVPKEY_Device_PowerData, &mut ty, buf.as_mut_ptr(), &mut len, 0) } == 0 && len >= 8;
        // CM_POWER_DATA: { ULONG PD_Size; DEVICE_POWER_STATE PD_MostRecentPowerState; … }
        let state = ok.then(|| u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]));
        make(dgpu_power_from_pnp(true, problem, state))
    }

    /// Instance id of the NVIDIA display-class PCI function, from the PnP
    /// device list plus the registry's `ClassGUID` (no device access).
    fn nvidia_display_instance_id() -> Option<String> {
        let filter = wide("PCI");
        let mut len = 0u32;
        if unsafe { CM_Get_Device_ID_List_SizeW(&mut len, filter.as_ptr(), CM_GETIDLIST_FILTER_ENUMERATOR) } != 0 || len == 0 {
            return None;
        }
        let mut buf = vec![0u16; len as usize];
        if unsafe { CM_Get_Device_ID_ListW(filter.as_ptr(), buf.as_mut_ptr(), len, CM_GETIDLIST_FILTER_ENUMERATOR) } != 0 {
            return None;
        }
        String::from_utf16_lossy(&buf)
            .split('\0')
            .filter(|id| id.to_ascii_uppercase().contains("VEN_10DE"))
            .find(|id| {
                let key = format!("SYSTEM\\CurrentControlSet\\Enum\\{id}");
                reg_get_string(HKEY_LOCAL_MACHINE, &key, "ClassGUID").map(|g| g.eq_ignore_ascii_case(DISPLAY_CLASS)).unwrap_or(false)
            })
            .map(|s| s.to_string())
    }

    fn ghelper_info() -> GHelperInfo {
        let appdata = std::env::var("APPDATA").ok().map(std::path::PathBuf::from);
        let installed = appdata.map(|a| a.join("GHelper").exists()).unwrap_or(false)
            || std::path::Path::new("C:\\Program Files\\GHelper").exists()
            || reg_get_string(HKEY_LOCAL_MACHINE, "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\GHelper", "DisplayName").is_some();
        let running = super::super::processes::light_snapshot().iter().any(|p| p.name.eq_ignore_ascii_case("ghelper.exe"));
        let mut info = GHelperInfo { installed: installed || running, running, coexistence: String::new() };
        info.coexistence = ghelper_coexistence(&info);
        info
    }

    fn platform() -> PlatformInfo {
        let key = "HARDWARE\\DESCRIPTION\\System\\BIOS";
        platform_from_dmi(
            reg_get_string(HKEY_LOCAL_MACHINE, key, "SystemManufacturer"),
            reg_get_string(HKEY_LOCAL_MACHINE, key, "SystemProductName"),
            reg_get_string(HKEY_LOCAL_MACHINE, key, "BIOSVersion"),
        )
    }

    struct Cached {
        info: HardwareInfo,
    }
    static CACHE: OnceLock<Mutex<Option<Cached>>> = OnceLock::new();

    fn gather_static() -> HardwareInfo {
        let cpu_name = reg_get_string(HKEY_LOCAL_MACHINE, "HARDWARE\\DESCRIPTION\\System\\CentralProcessor\\0", "ProcessorNameString")
            .or_else(|| std::env::var("PROCESSOR_IDENTIFIER").ok())
            .unwrap_or_else(|| "Unknown CPU".into())
            .trim()
            .to_string();
        let (logical, ram_mb) = unsafe {
            let mut si: SYSTEM_INFO = std::mem::zeroed();
            GetSystemInfo(&mut si);
            let mut ms = MEMORYSTATUSEX { dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32, dwMemoryLoad: 0, ullTotalPhys: 0, ullAvailPhys: 0, ullTotalPageFile: 0, ullAvailPageFile: 0, ullTotalVirtual: 0, ullAvailVirtual: 0, ullAvailExtendedVirtual: 0 };
            let ram = if GlobalMemoryStatusEx(&mut ms) != 0 { Some(ms.ullTotalPhys / (1024 * 1024)) } else { None };
            (si.dwNumberOfProcessors, ram)
        };
        let (topology, topology_error) = match super::super::topology::read_topology(&cpu_name) {
            Ok(t) => (Some(t), None),
            Err(e) => (None, Some(e)),
        };
        let adapters = display_adapters();
        let ghelper = ghelper_info();
        HardwareInfo {
            cpu_is_ryzen_ai_9_hx370: cpu_name.to_ascii_lowercase().contains("ryzen ai 9 hx 370"),
            cpu_architecture_note: cpu_note(&cpu_name),
            cpu_name,
            logical_processors: logical,
            ram_total_mb: ram_mb,
            topology,
            topology_error,
            dgpu: DgpuState { present: false, power: DgpuPower::Unknown, note: String::new() },
            igpu_only: false,
            adapters,
            npu: npu_info(),
            ghelper_detected: ghelper.installed,
            ghelper,
            platform: platform(),
            dgpu_policy: DGPU_POLICY.to_string(),
            sources: vec![
                "Registry: processor, display-class and compute-accelerator driver keys, BIOS strings".to_string(),
                "GetLogicalProcessorInformationEx (core, cache and efficiency-class topology)".to_string(),
                "PnP manager cached power data (no device I/O) for the discrete GPU".to_string(),
                "Never used: CUDA, NVAPI/NVML, DXGI, Direct3D, WMI GPU queries, GPU polling".to_string(),
            ],
        }
    }

    /// Hardware info; the static part is cached, the dGPU state is always fresh.
    pub fn hardware_impl(refresh: bool) -> Result<HardwareInfo, String> {
        let cache = CACHE.get_or_init(|| Mutex::new(None));
        let mut guard = cache.lock().unwrap_or_else(|p| p.into_inner());
        if refresh || guard.is_none() {
            *guard = Some(Cached { info: gather_static() });
        }
        let mut info = guard.as_ref().unwrap().info.clone();
        info.dgpu = dgpu_state(&info.adapters);
        info.igpu_only = matches!(info.dgpu.power, DgpuPower::PoweredDown | DgpuPower::Disabled | DgpuPower::Absent);
        // G-Helper can start or stop at any time: refresh its (cheap) state too.
        info.ghelper = ghelper_info();
        info.ghelper_detected = info.ghelper.installed;
        Ok(info)
    }
}

/// Return Windows hardware discovery without activating GPU inference APIs.
#[cfg(windows)]
pub fn hardware_info() -> Result<HardwareInfo, String> {
    win::hardware_impl(false)
}
/// Same, but re-reads the cached static facts (after a driver change).
#[cfg(windows)]
pub fn hardware_info_refresh() -> Result<HardwareInfo, String> {
    win::hardware_impl(true)
}
#[cfg(not(windows))]
pub fn hardware_info() -> Result<HardwareInfo, String> {
    Err("Hardware discovery is only available on Windows.".into())
}
#[cfg(not(windows))]
pub fn hardware_info_refresh() -> Result<HardwareInfo, String> {
    hardware_info()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adapter_classification() {
        assert_eq!(classify_adapter("AMD Radeon 890M Graphics"), "igpu");
        assert_eq!(classify_adapter("NVIDIA GeForce RTX 4070 Laptop GPU"), "dgpu");
        assert_eq!(classify_adapter("Microsoft Basic Display Adapter"), "other");
    }

    #[test]
    fn npu_detection_is_vendor_accurate() {
        assert!(is_npu_description("AMD IPU Device"));
        assert!(is_npu_description("NPU Compute Accelerator Device"));
        assert!(is_npu_description("AMD XDNA Neural Processing Unit"));
        assert!(!is_npu_description("Microsoft Basic Display Adapter"));
        assert_eq!(npu_label("AMD XDNA Neural Processing Unit"), "AMD XDNA NPU");
        assert_eq!(npu_label("Intel(R) AI Boost NPU"), "NPU");
    }

    #[test]
    fn npu_generation_comes_from_the_pci_id_and_never_guesses() {
        assert_eq!(npu_generation("PCI\\VEN_1022&DEV_17F0&REV_10"), Some("AMD XDNA 2"));
        assert_eq!(npu_generation("pci\\ven_1022&dev_1502"), Some("AMD XDNA (1st generation)"));
        assert_eq!(npu_generation("PCI\\VEN_1022&DEV_ABCD"), Some("AMD NPU (generation not identified)"));
        assert_eq!(npu_generation("PCI\\VEN_8086&DEV_643E"), None, "another vendor's NPU is not labelled AMD");
        assert_eq!(npu_generation(""), None);
    }

    #[test]
    fn hx370_note() {
        assert!(cpu_note("AMD Ryzen AI 9 HX 370 w/ Radeon 890M").contains("HX 370"));
        assert!(cpu_note("Intel Core Ultra 9").contains("no ASUS-specific"));
    }

    #[test]
    fn dgpu_power_state_is_only_awake_when_the_pnp_manager_says_d0() {
        assert_eq!(dgpu_power_from_pnp(false, None, None), DgpuPower::Absent);
        assert_eq!(dgpu_power_from_pnp(true, Some(22), Some(1)), DgpuPower::Disabled, "disabled wins over a stale power state");
        assert_eq!(dgpu_power_from_pnp(true, Some(0), Some(1)), DgpuPower::Awake);
        for d in [2, 3, 4] {
            assert_eq!(dgpu_power_from_pnp(true, Some(0), Some(d)), DgpuPower::PoweredDown, "D{}", d - 1);
        }
        for unknown in [None, Some(0), Some(9)] {
            assert_eq!(dgpu_power_from_pnp(true, Some(0), unknown), DgpuPower::Unknown);
        }
        // An unreadable status is never turned into "awake".
        assert!(!DgpuState { present: true, power: DgpuPower::Unknown, note: String::new() }.awake());
    }

    #[test]
    fn igpu_only_mode_covers_every_state_where_the_dgpu_is_not_in_use() {
        for (power, igpu_only) in [(DgpuPower::PoweredDown, true), (DgpuPower::Disabled, true), (DgpuPower::Absent, true), (DgpuPower::Awake, false), (DgpuPower::Unknown, false)] {
            let flag = matches!(power, DgpuPower::PoweredDown | DgpuPower::Disabled | DgpuPower::Absent);
            assert_eq!(flag, igpu_only, "{power:?}");
        }
        assert!(dgpu_note(DgpuPower::PoweredDown).contains("never touch") || dgpu_note(DgpuPower::PoweredDown).contains("will not touch"));
        assert!(dgpu_note(DgpuPower::Unknown).contains("never wakes"));
    }

    #[test]
    fn platform_detection_recognizes_the_zephyrus_g16() {
        let p = platform_from_dmi(Some("ASUSTeK COMPUTER INC.".into()), Some("ROG Zephyrus G16 GA605WI_GA605WI".into()), Some("GA605WI.312".into()));
        assert!(p.is_asus && p.is_zephyrus_g16);
        let other = platform_from_dmi(Some("ASUSTeK COMPUTER INC.".into()), Some("ROG Strix G16".into()), None);
        assert!(other.is_asus && !other.is_zephyrus_g16);
        assert!(!platform_from_dmi(Some("Dell Inc.".into()), Some("XPS 15".into()), None).is_asus);
    }

    #[test]
    fn ghelper_coexistence_text_matches_the_state() {
        let running = GHelperInfo { installed: true, running: true, coexistence: String::new() };
        assert!(ghelper_coexistence(&running).contains("pauses"));
        let installed = GHelperInfo { installed: true, running: false, coexistence: String::new() };
        assert!(ghelper_coexistence(&installed).contains("not running"));
        assert!(ghelper_coexistence(&GHelperInfo::default()).contains("not detected"));
        for info in [running, installed, GHelperInfo::default()] {
            assert!(ghelper_coexistence(&info).contains("never writes ASUS firmware"), "ASUS controls are never written in any state");
        }
    }

    /// Static guard for the RTX 4070 sleep requirement: no source file in the
    /// optimizer or the command layer may reference a GPU-waking API, and the
    /// manifest must not enable the Windows features that bind to them.
    #[test]
    fn no_gpu_waking_api_is_referenced_anywhere_in_the_codebase() {
        fn collect(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                let p = entry.path();
                if p.is_dir() {
                    collect(&p, out);
                } else if p.extension().map(|e| e == "rs").unwrap_or(false) {
                    out.push(p);
                }
            }
        }
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        collect(&src, &mut files);
        // Tokens that open, enumerate or query a GPU (comments excluded below).
        let forbidden = [
            "nvapi", "nvml", "nvcuda", "cuInit", "cudaGetDevice", "cudaSetDevice", "IDXGIFactory", "CreateDXGIFactory", "EnumAdapters", "D3D11CreateDevice", "D3D12CreateDevice",
            "Win32_Graphics_Dxgi", "Win32_Graphics_Direct3D", "Win32_VideoController", "Win32_PnPEntity", "wgpu", "ash::vk", "vulkan",
        ];
        let this_file = std::path::Path::new(file!()).file_name().unwrap().to_owned();
        for f in files {
            if f.file_name() == Some(&this_file) {
                continue; // the guard itself names the tokens
            }
            let text = std::fs::read_to_string(&f).unwrap();
            for (n, line) in text.lines().enumerate() {
                let code = line.split("//").next().unwrap_or("");
                for token in forbidden {
                    assert!(!code.to_ascii_lowercase().contains(&token.to_ascii_lowercase()), "{}:{} references GPU-waking API `{token}`", f.display(), n + 1);
                }
            }
        }
        let manifest = std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml")).unwrap();
        for feature in ["Win32_Graphics_Dxgi", "Win32_Graphics_Direct3D", "Win32_Graphics_Direct3D11", "Win32_Graphics_Direct3D12", "Win32_Graphics_Gdi"] {
            assert!(!manifest.contains(feature), "Cargo.toml enables {feature}");
        }
        for dep in ["wgpu", "ash", "nvml-wrapper", "cudarc", "ort"] {
            assert!(!manifest.lines().any(|l| l.trim_start().starts_with(&format!("{dep} ")) || l.trim_start().starts_with(&format!("{dep}="))), "Cargo.toml depends on {dep}");
        }
    }
}
