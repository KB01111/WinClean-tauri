//! Capability report: Windows build, Copilot+ status, Windows ML execution
//! providers (assessed layer by layer) and hardware-aware inference routing.
//! Discovery only — nothing is installed, registered or executed here, and no
//! GPU API is touched (see `hardware` and `winml`).

use serde::{Deserialize, Serialize};

use super::hardware::{DgpuPower, HardwareInfo, NpuInfo};
use super::onnx_model::{inspect_file, ModelProfile};
use super::winml::{assess_model, assess_providers, select_route, EpAssessment, ModelFit, RouteContext, RouteEntry, WinMlInputs};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CopilotPlusKind {
    /// Model and NPU generation match Microsoft's published Copilot+ list. Throughput is not measured.
    BySpec,
    /// The hardware looks eligible but Windows is older than 24H2.
    NeedsOsUpdate,
    Unverified,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CopilotPlusStatus {
    pub status: CopilotPlusKind,
    pub note: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelReport {
    pub path: String,
    pub profile: Option<ModelProfile>,
    pub fits: Vec<ModelFit>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityReport {
    pub windows_build: Option<u32>,
    pub copilot_plus: CopilotPlusStatus,
    pub providers: Vec<EpAssessment>,
    pub route_on_battery: Vec<RouteEntry>,
    pub route_on_ac: Vec<RouteEntry>,
    pub dgpu_power: Option<DgpuPower>,
    /// Always false: WinClean does not run inference sessions.
    pub npu_verified_for_inference: bool,
    pub model: Option<ModelReport>,
    pub notes: Vec<String>,
}

/// Microsoft's Copilot+ PC list names the AMD Ryzen AI 300 series, whose XDNA 2
/// NPU is specified above the required 40 TOPS. WinClean cannot measure
/// throughput, so a match is reported "by specification", never as measured.
pub fn copilot_plus_status(cpu_name: &str, npu: Option<&NpuInfo>, build: Option<u32>) -> CopilotPlusStatus {
    let cpu = cpu_name.to_ascii_lowercase();
    let ryzen_ai_300 = cpu.contains("ryzen ai") && ["hx 370", " 365", " 350", " 340"].iter().any(|m| cpu.contains(m));
    let xdna2 = npu.map(|n| n.present && n.generation.as_deref() == Some("AMD XDNA 2")).unwrap_or(false);
    if ryzen_ai_300 && xdna2 {
        return match build {
            Some(b) if b >= super::winml::MIN_CATALOG_BUILD => CopilotPlusStatus {
                status: CopilotPlusKind::BySpec,
                note: "AMD Ryzen AI 300 series with an XDNA 2 NPU is on Microsoft's Copilot+ PC list (NPU specified above the required 40 TOPS). WinClean cannot measure NPU throughput, so this is by specification, not measured.".into(),
            },
            _ => CopilotPlusStatus { status: CopilotPlusKind::NeedsOsUpdate, note: "The hardware matches Copilot+, but Windows 11 24H2 (build 26100) or later is required for Copilot+ and Windows ML execution providers.".into() },
        };
    }
    CopilotPlusStatus { status: CopilotPlusKind::Unverified, note: "Copilot+ eligibility (an NPU with 40+ TOPS) cannot be measured from Windows' public data, and this device does not match the known Copilot+ AMD list.".into() }
}

pub fn build_report(inputs: &WinMlInputs, cpu_name: &str, model: Option<ModelReport>) -> CapabilityReport {
    let providers = assess_providers(inputs);
    let genai = model.as_ref().and_then(|m| m.profile.as_ref()).map(|p| p.genai_like).unwrap_or(false);
    let fits: Vec<ModelFit> = model.as_ref().map(|m| m.fits.clone()).unwrap_or_default();
    let ctx = |on_battery| RouteContext { on_battery, allow_wake_dgpu: false, model_is_genai: genai };
    let mut notes = vec![
        "Execution providers are assessed from the OS build, hardware, driver, package, runtime and (optionally) the model. No inference session is run, so every provider is at most a candidate.".to_string(),
        "Microsoft Learn MCP is a development-time documentation server (see .mcp.json); it is not part of the shipped app.".to_string(),
    ];
    if inputs.dgpu_power.map(|p| p != DgpuPower::Awake).unwrap_or(true) {
        notes.push("The RTX 4070 is not awake, so it is excluded from routing. WinClean never wakes it.".to_string());
    }
    CapabilityReport {
        windows_build: inputs.windows_build,
        copilot_plus: copilot_plus_status(cpu_name, inputs.npu.as_ref(), inputs.windows_build),
        route_on_battery: select_route(&providers, &fits, &ctx(true)),
        route_on_ac: select_route(&providers, &fits, &ctx(false)),
        providers,
        dgpu_power: inputs.dgpu_power,
        npu_verified_for_inference: false,
        model,
        notes,
    }
}

/// Inspects a model file and scores it against every provider.
pub fn model_report(path: &str) -> ModelReport {
    match inspect_file(std::path::Path::new(path)) {
        Ok(profile) => {
            let fits = ["VitisAIExecutionProvider", "MIGraphXExecutionProvider", "NvTensorRtRtxExecutionProvider", "DirectMLExecutionProvider", "CPUExecutionProvider"].iter().map(|ep| assess_model(&profile, ep)).collect();
            ModelReport { path: path.to_string(), profile: Some(profile), fits, error: None }
        }
        Err(e) => ModelReport { path: path.to_string(), profile: None, fits: vec![], error: Some(e.to_string()) },
    }
}

/// Builds the report from the live machine (hardware is optional, so a
/// partial report still comes back on a PC without an NPU or off Windows).
pub fn capability_report(model_path: Option<&str>) -> Result<CapabilityReport, String> {
    let hw: Option<HardwareInfo> = super::hardware::hardware_info().ok();
    let inputs = WinMlInputs {
        windows_build: windows_build(),
        adapters: hw.as_ref().map(|h| h.adapters.clone()).unwrap_or_default(),
        dgpu_power: hw.as_ref().map(|h| h.dgpu.power),
        npu: hw.as_ref().map(|h| h.npu.clone()),
        packages: user_packages(),
    };
    let cpu = hw.as_ref().map(|h| h.cpu_name.clone()).unwrap_or_default();
    Ok(build_report(&inputs, &cpu, model_path.filter(|p| !p.trim().is_empty()).map(model_report)))
}

#[cfg(windows)]
pub fn windows_build() -> Option<u32> {
    // The registry CurrentBuildNumber is the documented, read-only equivalent of RtlGetVersion.
    use windows_sys::Win32::System::Registry::{RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY_LOCAL_MACHINE, KEY_QUERY_VALUE};
    let sub: Vec<u16> = "SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion".encode_utf16().chain([0]).collect();
    let val: Vec<u16> = "CurrentBuildNumber".encode_utf16().chain([0]).collect();
    let mut hkey = std::ptr::null_mut();
    if unsafe { RegOpenKeyExW(HKEY_LOCAL_MACHINE, sub.as_ptr(), 0, KEY_QUERY_VALUE, &mut hkey) } != 0 {
        return None;
    }
    let mut ty = 0u32;
    let mut size = 64u32;
    let mut buf = [0u16; 32];
    let rc = unsafe { RegQueryValueExW(hkey, val.as_ptr(), std::ptr::null_mut(), &mut ty, buf.as_mut_ptr() as *mut u8, &mut size) };
    unsafe { RegCloseKey(hkey) };
    if rc != 0 {
        return None;
    }
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end]).trim().parse().ok()
}

#[cfg(not(windows))]
pub fn windows_build() -> Option<u32> {
    None
}

/// Package full names registered for the current user, from the per-user
/// package repository key. Read-only; used only to notice Windows ML
/// execution-provider and runtime packages by name.
#[cfg(windows)]
fn user_packages() -> Vec<String> {
    use windows_sys::Win32::System::Registry::{RegCloseKey, RegEnumKeyExW, RegOpenKeyExW, HKEY_CURRENT_USER, KEY_READ};
    let sub: Vec<u16> = "Software\\Classes\\Local Settings\\Software\\Microsoft\\Windows\\CurrentVersion\\AppModel\\Repository\\Packages".encode_utf16().chain([0]).collect();
    let mut hkey = std::ptr::null_mut();
    if unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, sub.as_ptr(), 0, KEY_READ, &mut hkey) } != 0 {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut idx = 0u32;
    loop {
        let mut name = [0u16; 256];
        let mut len = name.len() as u32;
        let rc = unsafe { RegEnumKeyExW(hkey, idx, name.as_mut_ptr(), &mut len, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut()) };
        if rc != 0 {
            break;
        }
        let full = String::from_utf16_lossy(&name[..len as usize]);
        let lower = full.to_ascii_lowercase();
        if ["winml", "windowsml", "windowsappruntime", "vitisai", "migraphx", "tensorrt", "onnx"].iter().any(|t| lower.contains(t)) {
            out.push(full);
        }
        idx += 1;
    }
    unsafe { RegCloseKey(hkey) };
    out
}

#[cfg(not(windows))]
fn user_packages() -> Vec<String> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::optimizer::hardware::DisplayAdapter;
    use crate::optimizer::onnx_model::testing::*;
    use crate::optimizer::winml::{Fit, RouteStatus};

    const CPU: &str = "AMD Ryzen AI 9 HX 370 w/ Radeon 890M";

    fn npu(gen: Option<&str>) -> NpuInfo {
        NpuInfo { present: true, name: Some("NPU Compute Accelerator Device".into()), generation: gen.map(|g| g.into()), driver_version: Some("32.0.203.257".into()), driver_date: None, hardware_id: None, inference_verified: false, note: String::new() }
    }

    fn g16(dgpu: DgpuPower) -> WinMlInputs {
        WinMlInputs {
            windows_build: Some(26200),
            adapters: vec![
                DisplayAdapter { name: "AMD Radeon(TM) 890M Graphics".into(), driver_version: Some("32.0.13031.3015".into()), driver_date: None, hardware_id: None, kind: "igpu".into() },
                DisplayAdapter { name: "NVIDIA GeForce RTX 4070 Laptop GPU".into(), driver_version: Some("32.0.15.6094".into()), driver_date: None, hardware_id: None, kind: "dgpu".into() },
            ],
            dgpu_power: Some(dgpu),
            npu: Some(npu(Some("AMD XDNA 2"))),
            packages: vec![],
        }
    }

    #[test]
    fn copilot_plus_is_reported_by_specification_only_and_never_as_measured() {
        let ok = copilot_plus_status(CPU, Some(&npu(Some("AMD XDNA 2"))), Some(26200));
        assert_eq!(ok.status, CopilotPlusKind::BySpec);
        assert!(ok.note.contains("not measured"));
        assert_eq!(copilot_plus_status(CPU, Some(&npu(Some("AMD XDNA 2"))), Some(22631)).status, CopilotPlusKind::NeedsOsUpdate);
        assert_eq!(copilot_plus_status(CPU, Some(&npu(Some("AMD XDNA (1st generation)"))), Some(26200)).status, CopilotPlusKind::Unverified);
        assert_eq!(copilot_plus_status("AMD Ryzen 9 7940HS", Some(&npu(Some("AMD XDNA 2"))), Some(26200)).status, CopilotPlusKind::Unverified);
        assert_eq!(copilot_plus_status(CPU, None, Some(26200)).status, CopilotPlusKind::Unverified);
    }

    #[test]
    fn the_report_never_claims_a_verified_npu() {
        let r = build_report(&g16(DgpuPower::PoweredDown), CPU, None);
        assert!(!r.npu_verified_for_inference);
        assert!(r.notes.iter().any(|n| n.contains("RTX 4070") && n.contains("never wakes")));
        assert!(r.providers.iter().all(|p| p.checks.iter().all(|c| !(c.id == "session" && c.status == crate::optimizer::winml::CheckStatus::Pass))));
    }

    #[test]
    fn battery_and_ac_routes_differ_but_neither_includes_a_sleeping_dgpu() {
        let r = build_report(&g16(DgpuPower::PoweredDown), CPU, None);
        let first = |route: &[RouteEntry]| route.iter().find(|e| e.status == RouteStatus::Candidate).unwrap().ep.clone();
        assert_eq!(first(&r.route_on_battery), "VitisAIExecutionProvider");
        assert_eq!(first(&r.route_on_ac), "MIGraphXExecutionProvider");
        for route in [&r.route_on_battery, &r.route_on_ac] {
            assert!(!route.iter().any(|e| e.ep.contains("Nv") && e.status == RouteStatus::Candidate));
        }
    }

    #[test]
    fn an_llm_changes_the_route_and_a_quantized_vision_model_keeps_the_npu() {
        let dir = tempfile::tempdir().unwrap();
        let llm = dir.path().join("llm.onnx");
        std::fs::write(&llm, model(17, &[node("GroupQueryAttention"), node("MatMul")], &[], &[input("past_key_values.0.key", &[Some(1), None])])).unwrap();
        let rep = model_report(llm.to_str().unwrap());
        assert!(rep.profile.as_ref().unwrap().genai_like);
        let r = build_report(&g16(DgpuPower::PoweredDown), CPU, Some(rep));
        let battery_candidates: Vec<&str> = r.route_on_battery.iter().filter(|e| e.status == RouteStatus::Candidate).map(|e| e.ep.as_str()).collect();
        assert!(!battery_candidates.contains(&"VitisAIExecutionProvider"), "{battery_candidates:?}");
        assert!(!battery_candidates.contains(&"MIGraphXExecutionProvider"), "{battery_candidates:?}");

        let cnn = dir.path().join("cnn.onnx");
        std::fs::write(&cnn, model(17, &[node("QuantizeLinear"), node("Conv"), node("DequantizeLinear")], &[tensor(3, &[0; 8])], &[input("images", &[Some(1), Some(3), Some(224), Some(224)])])).unwrap();
        let rep = model_report(cnn.to_str().unwrap());
        assert_eq!(rep.fits.iter().find(|f| f.ep == "VitisAIExecutionProvider").unwrap().fit, Fit::Good);
        let r = build_report(&g16(DgpuPower::PoweredDown), CPU, Some(rep));
        assert_eq!(r.route_on_battery.iter().find(|e| e.status == RouteStatus::Candidate).unwrap().ep, "VitisAIExecutionProvider");
    }

    #[test]
    fn a_bad_model_path_is_an_error_in_the_report_not_a_failure() {
        let r = model_report("C:\\definitely\\missing\\model.onnx");
        assert!(r.profile.is_none());
        assert!(r.error.is_some());
        let r = model_report("notes.txt");
        assert!(r.error.unwrap().contains("expected a .onnx file"));
    }

    #[test]
    fn partial_hardware_still_yields_a_cpu_route() {
        let inputs = WinMlInputs::default();
        let r = build_report(&inputs, "", None);
        assert!(r.route_on_battery.iter().any(|e| e.ep == "CPUExecutionProvider" && e.status == RouteStatus::Fallback));
        assert!(r.route_on_battery.iter().all(|e| e.status != RouteStatus::Candidate));
    }
}
