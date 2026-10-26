//! Windows ML execution-provider assessment and hardware-aware routing.
//!
//! What this does: checks, layer by layer, whether each execution provider
//! *could* work on this machine (OS build, hardware, driver, package,
//! runtime, model) and ranks the candidates for a workload.
//!
//! What it does not do: run an inference session. Windows ML itself needs the
//! Windows App SDK runtime, which WinClean does not bundle, so the final
//! `session` layer is always reported as `notRun`. A provider is therefore at
//! most a *candidate* — never "verified" — and the UI says so.
//!
//! Hard rule: nothing routes to the RTX 4070 unless Windows already reports it
//! awake (or the user explicitly allows waking it). Using it while it sleeps
//! would power it up, which is exactly what WinClean must never cause.
//!
//! Sources: Microsoft Learn "Windows ML execution providers" (EP names,
//! requirements, GenAI limitation of MIGraphX) and "Accelerate AI models with
//! Windows ML" (silicon-to-EP mapping). See docs/windows-api-research.md.

use serde::{Deserialize, Serialize};

use super::hardware::{DgpuPower, DisplayAdapter, NpuInfo};
use super::onnx_model::ModelProfile;

/// Windows 11 24H2: first build where catalog EPs are available.
pub const MIN_CATALOG_BUILD: u32 = 26100;
/// Minimum NVIDIA driver (Windows format) recommended for NvTensorRtRtx.
pub const NVIDIA_MIN_DRIVER: &str = "32.0.15.5585";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CheckStatus {
    Pass,
    Fail,
    /// Cannot be determined from what Windows exposes without the runtime.
    Unknown,
    NotRun,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Check {
    pub id: String,
    pub label: String,
    pub status: CheckStatus,
    pub detail: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Readiness {
    Unavailable,
    /// Static checks pass; a real session test is still required.
    Candidate,
    /// Ships with Windows ML (CPU, DirectML).
    Included,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EpAssessment {
    pub name: String,
    pub device: String,
    pub vendor: String,
    pub readiness: Readiness,
    /// Using this provider would power up the discrete GPU.
    pub wakes_dgpu: bool,
    pub checks: Vec<Check>,
    pub summary: String,
}

#[derive(Clone, Debug, Default)]
pub struct WinMlInputs {
    pub windows_build: Option<u32>,
    pub adapters: Vec<DisplayAdapter>,
    pub dgpu_power: Option<DgpuPower>,
    pub npu: Option<NpuInfo>,
    /// Package full names visible in the current user's package repository.
    pub packages: Vec<String>,
}

fn check(id: &str, label: &str, status: CheckStatus, detail: impl Into<String>) -> Check {
    Check { id: id.into(), label: label.into(), status, detail: detail.into() }
}

/// Compares dotted numeric versions (`32.0.15.5585`). `None` if either side
/// is not purely numeric.
pub fn compare_dotted(a: &str, b: &str) -> Option<std::cmp::Ordering> {
    let parse = |s: &str| -> Option<Vec<u64>> { s.trim().split('.').map(|p| p.parse::<u64>().ok()).collect() };
    let (pa, pb) = (parse(a)?, parse(b)?);
    let n = pa.len().max(pb.len());
    for i in 0..n {
        let (x, y) = (pa.get(i).copied().unwrap_or(0), pb.get(i).copied().unwrap_or(0));
        if x != y {
            return Some(x.cmp(&y));
        }
    }
    Some(std::cmp::Ordering::Equal)
}

fn package_with(packages: &[String], token: &str) -> Option<String> {
    packages.iter().find(|p| p.to_ascii_lowercase().contains(token)).cloned()
}

/// Lower-cases a registry adapter name and drops trademark marks and extra
/// spaces ("AMD Radeon(TM) 890M Graphics" → "amd radeon 890m graphics").
fn normalize_gpu_name(name: &str) -> String {
    let lowered = name.to_ascii_lowercase().replace("(tm)", "").replace("(r)", "").replace('™', "").replace('®', "");
    lowered.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// AMD GPUs the MIGraphX provider supports: RDNA 3 or later.
pub fn is_amd_rdna3_or_later(name: &str) -> bool {
    let n = normalize_gpu_name(name);
    ["radeon 890m", "radeon 880m", "radeon 860m", "radeon 780m", "radeon 760m", "radeon 740m", "rx 7", "rx 9", "pro w7", "pro w9"].iter().any(|t| n.contains(t))
}

/// NVIDIA RTX 30-series or later.
pub fn is_nvidia_rtx_30_or_later(name: &str) -> bool {
    let n = normalize_gpu_name(name);
    n.contains("rtx") && ["rtx 30", "rtx 40", "rtx 50", "rtx a", "rtx pro"].iter().any(|t| n.contains(t))
}

fn os_check(build: Option<u32>) -> Check {
    match build {
        Some(b) if b >= MIN_CATALOG_BUILD => check("os", "Windows 11 24H2 or later", CheckStatus::Pass, format!("Build {b} (catalog execution providers need ≥ {MIN_CATALOG_BUILD}).")),
        Some(b) => check("os", "Windows 11 24H2 or later", CheckStatus::Fail, format!("Build {b} is older than {MIN_CATALOG_BUILD}; catalog execution providers are not available.")),
        None => check("os", "Windows 11 24H2 or later", CheckStatus::Unknown, "The Windows build could not be read."),
    }
}

fn package_check(packages: &[String], token: &str, label: &str) -> Check {
    match package_with(packages, token) {
        Some(p) => check("package", label, CheckStatus::Pass, format!("A matching package is registered for this user ({p}). Detection is by package name only.")),
        None => check("package", label, CheckStatus::Unknown, "Not detected for this user. Windows downloads certified providers on demand through ExecutionProviderCatalog when an app requests one; WinClean never downloads or registers anything."),
    }
}

fn runtime_check(packages: &[String]) -> Check {
    match package_with(packages, "windowsappruntime").or_else(|| package_with(packages, "windowsml")) {
        Some(p) => check("runtime", "Windows ML runtime", CheckStatus::Pass, format!("Windows App SDK / Windows ML framework registered ({p}).")),
        None => check("runtime", "Windows ML runtime", CheckStatus::Unknown, "No Windows App SDK / Windows ML framework found for this user. WinClean does not bundle it; an app that runs models brings or requires it."),
    }
}

fn session_check() -> Check {
    check("session", "Inference session test", CheckStatus::NotRun, "WinClean does not run inference sessions. Validate with your real model and driver before relying on this provider.")
}

fn finish(name: &str, device: &str, vendor: &str, wakes_dgpu: bool, checks: Vec<Check>, required: &[&str]) -> EpAssessment {
    let blocked: Vec<&Check> = checks.iter().filter(|c| required.contains(&c.id.as_str()) && c.status == CheckStatus::Fail).collect();
    let unknown_required = checks.iter().any(|c| required.contains(&c.id.as_str()) && c.status == CheckStatus::Unknown);
    let readiness = if !blocked.is_empty() { Readiness::Unavailable } else { Readiness::Candidate };
    let summary = if let Some(b) = blocked.first() {
        format!("Unavailable: {}", b.detail)
    } else if unknown_required {
        "Candidate: static checks pass, some layers cannot be verified from Windows alone. A real session test is still required.".to_string()
    } else {
        "Candidate: static checks pass. A real session test is still required.".to_string()
    };
    EpAssessment { name: name.into(), device: device.into(), vendor: vendor.into(), readiness, wakes_dgpu, checks, summary }
}

pub fn assess_providers(i: &WinMlInputs) -> Vec<EpAssessment> {
    let mut out = Vec::new();

    // --- VitisAI (AMD XDNA NPU) ---
    {
        let npu = i.npu.as_ref();
        let amd_npu = npu.map(|n| n.present && n.generation.as_deref().map(|g| g.starts_with("AMD")).unwrap_or(false)).unwrap_or(false);
        let hardware = match npu {
            Some(n) if amd_npu => check("hardware", "AMD NPU present", CheckStatus::Pass, format!("{} ({}).", n.name.clone().unwrap_or_default(), n.generation.clone().unwrap_or_default())),
            Some(n) if n.present => check("hardware", "AMD NPU present", CheckStatus::Fail, "An NPU is present but it is not an AMD XDNA device."),
            _ => check("hardware", "AMD NPU present", CheckStatus::Fail, "No NPU found. Install the AMD NPU driver from ASUS/AMD."),
        };
        let driver = match npu.and_then(|n| n.driver_version.clone()) {
            Some(v) => check("driver", "NPU driver installed", CheckStatus::Pass, format!("Driver {v}. No numeric minimum is documented for the provider, so none is enforced.")),
            None => check("driver", "NPU driver installed", CheckStatus::Fail, "The NPU has no driver version in the registry."),
        };
        out.push(finish(
            "VitisAIExecutionProvider",
            "npu",
            "AMD",
            false,
            vec![os_check(i.windows_build), hardware, driver, package_check(&i.packages, "vitisai", "Provider package"), runtime_check(&i.packages), session_check()],
            &["os", "hardware", "driver"],
        ));
    }

    // --- MIGraphX (AMD GPU, integrated) ---
    {
        let amd = i.adapters.iter().find(|a| a.name.to_ascii_lowercase().contains("amd") || a.name.to_ascii_lowercase().contains("radeon"));
        let hardware = match amd {
            Some(a) if is_amd_rdna3_or_later(&a.name) => check("hardware", "AMD RDNA 3+ GPU", CheckStatus::Pass, format!("{} (RDNA 3 or later).", a.name)),
            Some(a) => check("hardware", "AMD RDNA 3+ GPU", CheckStatus::Fail, format!("{} is not a supported RDNA 3+ GPU.", a.name)),
            None => check("hardware", "AMD RDNA 3+ GPU", CheckStatus::Fail, "No AMD GPU found."),
        };
        let driver = match amd.and_then(|a| a.driver_version.clone()) {
            Some(v) => check("driver", "AMD driver ≥ 25.10.13.09", CheckStatus::Unknown, format!("Installed driver {v}. Microsoft states the minimum in AMD's Adrenalin version format, which differs from the registry's version format, so WinClean cannot compare them. Check Adrenalin → About.")),
            None => check("driver", "AMD driver ≥ 25.10.13.09", CheckStatus::Unknown, "No driver version found."),
        };
        out.push(finish(
            "MIGraphXExecutionProvider",
            "gpu",
            "AMD",
            false,
            vec![
                os_check(i.windows_build),
                hardware,
                driver,
                package_check(&i.packages, "migraphx", "Provider package"),
                runtime_check(&i.packages),
                check("genai", "Generative AI models", CheckStatus::Fail, "Microsoft documents MIGraphX as not supported for GenAI scenarios today; it suits classic vision/audio/small-transformer ONNX models."),
                session_check(),
            ],
            &["os", "hardware"],
        ));
    }

    // --- NvTensorRtRtx (discrete GPU) ---
    {
        let nv = i.adapters.iter().find(|a| a.kind == "dgpu" || a.name.to_ascii_lowercase().contains("nvidia"));
        let power = i.dgpu_power.unwrap_or(DgpuPower::Unknown);
        let hardware = match nv {
            Some(a) if is_nvidia_rtx_30_or_later(&a.name) => check("hardware", "NVIDIA RTX 30-series or later", CheckStatus::Pass, format!("{}.", a.name)),
            Some(a) => check("hardware", "NVIDIA RTX 30-series or later", CheckStatus::Fail, format!("{} is not an RTX 30-series or later GPU.", a.name)),
            None => check("hardware", "NVIDIA RTX 30-series or later", CheckStatus::Fail, "No NVIDIA GPU found."),
        };
        let driver = match nv.and_then(|a| a.driver_version.clone()) {
            Some(v) => match compare_dotted(&v, NVIDIA_MIN_DRIVER) {
                Some(std::cmp::Ordering::Less) => check("driver", &format!("NVIDIA driver ≥ {NVIDIA_MIN_DRIVER}"), CheckStatus::Fail, format!("Installed {v} is older than the recommended {NVIDIA_MIN_DRIVER}.")),
                Some(_) => check("driver", &format!("NVIDIA driver ≥ {NVIDIA_MIN_DRIVER}"), CheckStatus::Pass, format!("Installed {v}.")),
                None => check("driver", &format!("NVIDIA driver ≥ {NVIDIA_MIN_DRIVER}"), CheckStatus::Unknown, format!("Installed {v} could not be compared.")),
            },
            None => check("driver", &format!("NVIDIA driver ≥ {NVIDIA_MIN_DRIVER}"), CheckStatus::Unknown, "No driver version found."),
        };
        let awake = match power {
            DgpuPower::Awake => check("dgpu_awake", "RTX 4070 already awake", CheckStatus::Pass, "Windows reports the GPU awake (D0); using it does not wake anything."),
            DgpuPower::PoweredDown => check("dgpu_awake", "RTX 4070 already awake", CheckStatus::Fail, "The GPU is powered down. Running this provider would wake it, so WinClean will not route inference here."),
            DgpuPower::Disabled => check("dgpu_awake", "RTX 4070 already awake", CheckStatus::Fail, "The GPU is disabled (for example G-Helper Eco mode)."),
            DgpuPower::Absent => check("dgpu_awake", "RTX 4070 already awake", CheckStatus::Fail, "No discrete GPU is enumerated."),
            DgpuPower::Unknown => check("dgpu_awake", "RTX 4070 already awake", CheckStatus::Unknown, "The GPU power state could not be read; treated as unavailable."),
        };
        out.push(finish(
            "NvTensorRtRtxExecutionProvider",
            "gpu",
            "NVIDIA",
            power != DgpuPower::Awake,
            vec![os_check(i.windows_build), hardware, driver, awake, package_check(&i.packages, "tensorrt", "Provider package"), runtime_check(&i.packages), session_check()],
            &["os", "hardware", "driver", "dgpu_awake"],
        ));
    }

    // --- DirectML (included, legacy) ---
    {
        let igpu = i.adapters.iter().find(|a| a.kind == "igpu");
        let hardware = match igpu {
            Some(a) => check("hardware", "Integrated GPU to pin", CheckStatus::Pass, format!("{}. DirectML must be pinned to this adapter; letting it choose could pick the RTX 4070.", a.name)),
            None => check("hardware", "Integrated GPU to pin", CheckStatus::Fail, "No integrated GPU found to pin DirectML to."),
        };
        let mut e = finish("DirectMLExecutionProvider", "gpu", "Microsoft", false, vec![hardware, check("maintenance", "Maintenance mode", CheckStatus::Pass, "Included with Windows ML; Microsoft lists DirectML as legacy and recommends vendor providers."), session_check()], &["hardware"]);
        if e.readiness == Readiness::Candidate {
            e.readiness = Readiness::Included;
            e.summary = "Included with Windows ML (legacy). Pin it to the integrated GPU.".to_string();
        }
        out.push(e);
    }

    // --- CPU ---
    out.push(EpAssessment {
        name: "CPUExecutionProvider".into(),
        device: "cpu".into(),
        vendor: "Microsoft".into(),
        readiness: Readiness::Included,
        wakes_dgpu: false,
        checks: vec![check("always", "Always available", CheckStatus::Pass, "Ships with Windows ML / ONNX Runtime. Universal fallback.")],
        summary: "Always available fallback.".into(),
    });
    out
}

// ---------------------------------------------------------------------------
// Model fit
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Fit {
    Good,
    Risky,
    Unsupported,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelFit {
    pub ep: String,
    pub fit: Fit,
    pub reasons: Vec<String>,
}

/// Guidance on how well `model` suits a provider. This is a heuristic from the
/// model's structure — not a compatibility guarantee, which only a session
/// test can give — and is labelled that way in the UI.
pub fn assess_model(model: &ModelProfile, ep: &str) -> ModelFit {
    let mut reasons = Vec::new();
    let fit = match ep {
        "VitisAIExecutionProvider" => {
            if model.genai_like {
                reasons.push("The model has LLM structure (KV cache / attention). Arbitrary language models are not expected to run through the generic NPU provider; LLMs need AMD's dedicated Ryzen AI/ONNX Runtime GenAI flow.".to_string());
                Fit::Unsupported
            } else if model.quantized {
                reasons.push("Quantized (QDQ/int8) operators are what the AMD NPU toolchain targets.".to_string());
                if model.dynamic_input_shapes {
                    reasons.push("Dynamic input shapes usually have to be fixed before NPU compilation.".to_string());
                    Fit::Risky
                } else {
                    Fit::Good
                }
            } else {
                reasons.push("The model is not quantized. NPU execution normally needs an int8/QDQ (or BF16) model produced with AMD's quantization tools; operators may fall back to the CPU.".to_string());
                Fit::Risky
            }
        }
        "MIGraphXExecutionProvider" => {
            if model.genai_like {
                reasons.push("Microsoft documents MIGraphX as not supported for GenAI scenarios.".to_string());
                Fit::Unsupported
            } else if model.quantized || model.block_quantized {
                reasons.push("Quantized graphs have only partial GPU support in MIGraphX; some operators may fall back.".to_string());
                Fit::Risky
            } else {
                reasons.push("A float model without LLM structure is the main target of this provider.".to_string());
                Fit::Good
            }
        }
        "NvTensorRtRtxExecutionProvider" => {
            if model.genai_like {
                reasons.push("LLM support through this provider depends on the model and runtime build; test it.".to_string());
                Fit::Unknown
            } else {
                Fit::Good
            }
        }
        "DirectMLExecutionProvider" => {
            reasons.push("DirectML runs most float ONNX models on any DirectX 12 GPU (legacy).".to_string());
            if model.block_quantized {
                reasons.push("Block-quantized (int4) weights may not be supported.".to_string());
                Fit::Risky
            } else {
                Fit::Good
            }
        }
        "CPUExecutionProvider" => {
            reasons.push("Runs everything, at CPU speed.".to_string());
            Fit::Good
        }
        _ => Fit::Unknown,
    };
    if model.uses_external_data {
        reasons.push("The model uses external weight files; keep them next to the .onnx file.".to_string());
    }
    ModelFit { ep: ep.to_string(), fit, reasons }
}

// ---------------------------------------------------------------------------
// Routing
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RouteStatus {
    /// Static checks pass; needs a session test.
    Candidate,
    Fallback,
    /// Deliberately not used; `reason` says why.
    Excluded,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteEntry {
    pub ep: String,
    pub device: String,
    pub status: RouteStatus,
    pub reason: String,
}

#[derive(Clone, Copy, Debug)]
pub struct RouteContext {
    pub on_battery: bool,
    /// The user explicitly allows waking the discrete GPU.
    pub allow_wake_dgpu: bool,
    pub model_is_genai: bool,
}

/// True when the only failing required check is "the dGPU is asleep".
fn only_blocked_by_dgpu_sleep(a: &EpAssessment) -> bool {
    let failing: Vec<&Check> = a.checks.iter().filter(|c| c.status == CheckStatus::Fail).collect();
    a.wakes_dgpu && !failing.is_empty() && failing.iter().all(|c| c.id == "dgpu_awake") && a.checks.iter().any(|c| c.id == "dgpu_awake" && c.detail.contains("powered down"))
}

/// Ranks execution providers for a workload. Order on battery: NPU → iGPU →
/// (awake) dGPU → CPU. On AC: iGPU → NPU → (awake) dGPU → CPU. Providers that
/// would wake the dGPU, or that cannot work for the model, are listed as
/// `excluded` with the reason, never silently dropped.
pub fn select_route(assessments: &[EpAssessment], fits: &[ModelFit], ctx: &RouteContext) -> Vec<RouteEntry> {
    let find = |name: &str| assessments.iter().find(|a| a.name == name);
    let fit_of = |name: &str| fits.iter().find(|f| f.ep == name);
    let mut candidates: Vec<RouteEntry> = Vec::new();
    let mut excluded: Vec<RouteEntry> = Vec::new();

    let consider = |name: &str, device: &str, candidates: &mut Vec<RouteEntry>, excluded: &mut Vec<RouteEntry>| {
        let Some(a) = find(name) else { return };
        // A provider blocked *only* because the dGPU sleeps may be used when the
        // user explicitly allows waking it; the reason says so plainly.
        if a.readiness == Readiness::Unavailable && ctx.allow_wake_dgpu && only_blocked_by_dgpu_sleep(a) {
            candidates.push(RouteEntry { ep: name.into(), device: device.into(), status: RouteStatus::Candidate, reason: "Will wake the RTX 4070 (you allowed it). Confirm with a real session test.".into() });
            return;
        }
        if a.readiness == Readiness::Unavailable {
            excluded.push(RouteEntry { ep: name.into(), device: device.into(), status: RouteStatus::Excluded, reason: a.summary.clone() });
            return;
        }
        if a.wakes_dgpu && !ctx.allow_wake_dgpu {
            excluded.push(RouteEntry { ep: name.into(), device: device.into(), status: RouteStatus::Excluded, reason: "Would wake the RTX 4070; WinClean does not route inference to a sleeping GPU.".into() });
            return;
        }
        if let Some(f) = fit_of(name) {
            if f.fit == Fit::Unsupported {
                excluded.push(RouteEntry { ep: name.into(), device: device.into(), status: RouteStatus::Excluded, reason: f.reasons.first().cloned().unwrap_or_else(|| "The model is not supported by this provider.".into()) });
                return;
            }
        }
        if ctx.model_is_genai && name == "MIGraphXExecutionProvider" {
            excluded.push(RouteEntry { ep: name.into(), device: device.into(), status: RouteStatus::Excluded, reason: "MIGraphX is not supported for GenAI scenarios.".into() });
            return;
        }
        let note = match fit_of(name) {
            Some(f) if f.fit == Fit::Risky => format!("Candidate with caveats: {}", f.reasons.join(" ")),
            _ if a.readiness == Readiness::Included => "Included with Windows ML.".to_string(),
            _ => "Static checks pass; confirm with a real session test.".to_string(),
        };
        candidates.push(RouteEntry { ep: name.into(), device: device.into(), status: RouteStatus::Candidate, reason: note });
    };

    let order: [(&str, &str); 4] = if ctx.on_battery {
        [("VitisAIExecutionProvider", "npu"), ("MIGraphXExecutionProvider", "gpu"), ("DirectMLExecutionProvider", "gpu"), ("NvTensorRtRtxExecutionProvider", "gpu")]
    } else {
        [("MIGraphXExecutionProvider", "gpu"), ("DirectMLExecutionProvider", "gpu"), ("VitisAIExecutionProvider", "npu"), ("NvTensorRtRtxExecutionProvider", "gpu")]
    };
    for (name, device) in order {
        consider(name, device, &mut candidates, &mut excluded);
    }
    // MIGraphX and DirectML both target the iGPU; keep the first as the iGPU
    // choice and the other as a backup only when the first is missing.
    let mut route = candidates;
    route.push(RouteEntry { ep: "CPUExecutionProvider".into(), device: "cpu".into(), status: RouteStatus::Fallback, reason: "Always available; used when nothing above is usable.".into() });
    route.extend(excluded);
    route
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::optimizer::hardware::NpuInfo;

    fn adapter(name: &str, kind: &str, driver: &str) -> DisplayAdapter {
        DisplayAdapter { name: name.into(), driver_version: Some(driver.into()), driver_date: None, hardware_id: None, kind: kind.into() }
    }

    fn npu(present: bool, generation: Option<&str>) -> NpuInfo {
        NpuInfo {
            present,
            name: present.then(|| "NPU Compute Accelerator Device".to_string()),
            generation: generation.map(|g| g.to_string()),
            driver_version: present.then(|| "32.0.203.257".to_string()),
            driver_date: None,
            hardware_id: None,
            inference_verified: false,
            note: String::new(),
        }
    }

    fn g16(dgpu: DgpuPower, build: u32) -> WinMlInputs {
        WinMlInputs {
            windows_build: Some(build),
            adapters: vec![adapter("AMD Radeon(TM) 890M Graphics", "igpu", "32.0.13031.3015"), adapter("NVIDIA GeForce RTX 4070 Laptop GPU", "dgpu", "32.0.15.6094")],
            dgpu_power: Some(dgpu),
            npu: Some(npu(true, Some("AMD XDNA 2"))),
            packages: vec![],
        }
    }

    fn get<'a>(v: &'a [EpAssessment], name: &str) -> &'a EpAssessment {
        v.iter().find(|a| a.name == name).unwrap()
    }
    fn status<'a>(a: &'a EpAssessment, id: &str) -> CheckStatus {
        a.checks.iter().find(|c| c.id == id).unwrap().status
    }

    #[test]
    fn dotted_versions_compare_numerically() {
        use std::cmp::Ordering::*;
        assert_eq!(compare_dotted("32.0.15.6094", NVIDIA_MIN_DRIVER), Some(Greater));
        assert_eq!(compare_dotted("32.0.15.5585", NVIDIA_MIN_DRIVER), Some(Equal));
        assert_eq!(compare_dotted("32.0.15.999", NVIDIA_MIN_DRIVER), Some(Less), "numeric, not lexicographic");
        assert_eq!(compare_dotted("32.0.15", "32.0.15.0"), Some(Equal));
        assert_eq!(compare_dotted("abc", "1.2"), None);
    }

    #[test]
    fn the_g16_with_a_sleeping_dgpu_has_npu_and_igpu_candidates_and_no_dgpu() {
        let a = assess_providers(&g16(DgpuPower::PoweredDown, 26200));
        assert_eq!(get(&a, "VitisAIExecutionProvider").readiness, Readiness::Candidate);
        assert_eq!(get(&a, "MIGraphXExecutionProvider").readiness, Readiness::Candidate);
        let nv = get(&a, "NvTensorRtRtxExecutionProvider");
        assert_eq!(nv.readiness, Readiness::Unavailable);
        assert!(nv.wakes_dgpu);
        assert_eq!(status(nv, "dgpu_awake"), CheckStatus::Fail);
        assert_eq!(get(&a, "CPUExecutionProvider").readiness, Readiness::Included);
    }

    #[test]
    fn session_tests_are_never_claimed() {
        for e in assess_providers(&g16(DgpuPower::Awake, 26200)).iter().filter(|e| e.name != "CPUExecutionProvider") {
            assert_eq!(status(e, "session"), CheckStatus::NotRun, "{}", e.name);
            assert!(e.checks.iter().all(|c| !(c.id == "session" && c.status == CheckStatus::Pass)), "WinClean cannot run a session, so it never reports one as passed");
        }
    }

    #[test]
    fn an_old_windows_build_blocks_the_catalog_providers_but_not_cpu() {
        let a = assess_providers(&g16(DgpuPower::Awake, 22631));
        for name in ["VitisAIExecutionProvider", "MIGraphXExecutionProvider", "NvTensorRtRtxExecutionProvider"] {
            assert_eq!(get(&a, name).readiness, Readiness::Unavailable, "{name}");
        }
        assert_eq!(get(&a, "CPUExecutionProvider").readiness, Readiness::Included);
    }

    #[test]
    fn missing_npu_or_driver_blocks_vitisai() {
        let mut i = g16(DgpuPower::Awake, 26200);
        i.npu = Some(npu(false, None));
        assert_eq!(get(&assess_providers(&i), "VitisAIExecutionProvider").readiness, Readiness::Unavailable);
        i.npu = Some(npu(true, Some("AMD XDNA 2")));
        i.npu.as_mut().unwrap().driver_version = None;
        assert_eq!(get(&assess_providers(&i), "VitisAIExecutionProvider").readiness, Readiness::Unavailable);
        let mut intel = g16(DgpuPower::Awake, 26200);
        intel.npu = Some(npu(true, None));
        assert_eq!(get(&assess_providers(&intel), "VitisAIExecutionProvider").readiness, Readiness::Unavailable, "a non-AMD NPU is not an XDNA target");
    }

    #[test]
    fn nvidia_driver_minimum_is_enforced_with_the_documented_value() {
        let mut i = g16(DgpuPower::Awake, 26200);
        i.adapters[1].driver_version = Some("31.0.15.4601".into());
        let nv = assess_providers(&i);
        assert_eq!(status(get(&nv, "NvTensorRtRtxExecutionProvider"), "driver"), CheckStatus::Fail);
        assert_eq!(get(&nv, "NvTensorRtRtxExecutionProvider").readiness, Readiness::Unavailable);
    }

    #[test]
    fn amd_driver_comparison_is_honestly_unknown() {
        let a = assess_providers(&g16(DgpuPower::PoweredDown, 26200));
        assert_eq!(status(get(&a, "MIGraphXExecutionProvider"), "driver"), CheckStatus::Unknown);
        assert_eq!(status(get(&a, "MIGraphXExecutionProvider"), "genai"), CheckStatus::Fail);
    }

    #[test]
    fn package_detection_is_name_based_and_otherwise_unknown() {
        let mut i = g16(DgpuPower::PoweredDown, 26200);
        i.packages = vec!["MicrosoftCorporationII.WinML.AMD.VitisAI_1.8.0.0_x64__8wekyb3d8bbwe".into(), "Microsoft.WindowsAppRuntime.1.8_8000.1.0.0_x64__8wekyb3d8bbwe".into()];
        let a = assess_providers(&i);
        assert_eq!(status(get(&a, "VitisAIExecutionProvider"), "package"), CheckStatus::Pass);
        assert_eq!(status(get(&a, "VitisAIExecutionProvider"), "runtime"), CheckStatus::Pass);
        assert_eq!(status(get(&a, "MIGraphXExecutionProvider"), "package"), CheckStatus::Unknown, "absence is never reported as a hard failure");
    }

    fn route(inputs: &WinMlInputs, ctx: RouteContext, fits: &[ModelFit]) -> Vec<RouteEntry> {
        select_route(&assess_providers(inputs), fits, &ctx)
    }
    fn names(r: &[RouteEntry], status: RouteStatus) -> Vec<String> {
        r.iter().filter(|e| e.status == status).map(|e| e.ep.clone()).collect()
    }
    const BATTERY: RouteContext = RouteContext { on_battery: true, allow_wake_dgpu: false, model_is_genai: false };
    const AC: RouteContext = RouteContext { on_battery: false, allow_wake_dgpu: false, model_is_genai: false };

    #[test]
    fn on_battery_the_npu_leads_then_igpu_then_cpu_and_the_sleeping_dgpu_is_excluded() {
        let r = route(&g16(DgpuPower::PoweredDown, 26200), BATTERY, &[]);
        let c = names(&r, RouteStatus::Candidate);
        assert_eq!(c[0], "VitisAIExecutionProvider");
        assert!(c.contains(&"MIGraphXExecutionProvider".to_string()));
        assert!(!c.iter().any(|n| n.contains("Nv")), "{c:?}");
        assert_eq!(names(&r, RouteStatus::Fallback), vec!["CPUExecutionProvider"]);
        let ex = r.iter().find(|e| e.ep == "NvTensorRtRtxExecutionProvider").unwrap();
        assert_eq!(ex.status, RouteStatus::Excluded);
        assert!(ex.reason.contains("RTX 4070") || ex.reason.contains("powered down"), "{}", ex.reason);
    }

    #[test]
    fn on_ac_the_igpu_leads() {
        let r = route(&g16(DgpuPower::PoweredDown, 26200), AC, &[]);
        assert_eq!(names(&r, RouteStatus::Candidate)[0], "MIGraphXExecutionProvider");
    }

    #[test]
    fn an_awake_dgpu_becomes_a_candidate_but_a_sleeping_one_needs_explicit_consent() {
        let awake = route(&g16(DgpuPower::Awake, 26200), AC, &[]);
        assert!(names(&awake, RouteStatus::Candidate).contains(&"NvTensorRtRtxExecutionProvider".to_string()));
        let denied = route(&g16(DgpuPower::PoweredDown, 26200), AC, &[]);
        assert_eq!(denied.iter().find(|e| e.ep == "NvTensorRtRtxExecutionProvider").unwrap().status, RouteStatus::Excluded);
        let allowed = route(&g16(DgpuPower::PoweredDown, 26200), RouteContext { allow_wake_dgpu: true, ..AC }, &[]);
        let nv = allowed.iter().find(|e| e.ep == "NvTensorRtRtxExecutionProvider").unwrap();
        assert_eq!(nv.status, RouteStatus::Candidate);
        assert!(nv.reason.contains("Will wake the RTX 4070"), "{}", nv.reason);
        // Consent never overrides other failures (a disabled GPU cannot be woken at all).
        let disabled = route(&g16(DgpuPower::Disabled, 26200), RouteContext { allow_wake_dgpu: true, ..AC }, &[]);
        assert_eq!(disabled.iter().find(|e| e.ep == "NvTensorRtRtxExecutionProvider").unwrap().status, RouteStatus::Excluded);
    }

    #[test]
    fn unknown_dgpu_state_is_never_routed_to() {
        let r = route(&g16(DgpuPower::Unknown, 26200), BATTERY, &[]);
        assert!(!names(&r, RouteStatus::Candidate).contains(&"NvTensorRtRtxExecutionProvider".to_string()));
    }

    #[test]
    fn with_no_accelerators_the_route_is_cpu_only() {
        let i = WinMlInputs { windows_build: Some(22631), adapters: vec![], dgpu_power: Some(DgpuPower::Absent), npu: None, packages: vec![] };
        let r = route(&i, BATTERY, &[]);
        assert!(names(&r, RouteStatus::Candidate).is_empty());
        assert_eq!(names(&r, RouteStatus::Fallback), vec!["CPUExecutionProvider"]);
    }

    #[test]
    fn language_models_are_kept_off_the_npu_and_migraphx() {
        let llm = ModelProfile { genai_like: true, ..Default::default() };
        let fits: Vec<ModelFit> = ["VitisAIExecutionProvider", "MIGraphXExecutionProvider", "DirectMLExecutionProvider"].iter().map(|e| assess_model(&llm, e)).collect();
        assert_eq!(fits[0].fit, Fit::Unsupported);
        assert_eq!(fits[1].fit, Fit::Unsupported);
        let r = route(&g16(DgpuPower::PoweredDown, 26200), RouteContext { model_is_genai: true, ..BATTERY }, &fits);
        let c = names(&r, RouteStatus::Candidate);
        assert!(!c.contains(&"VitisAIExecutionProvider".to_string()), "{c:?}");
        assert!(!c.contains(&"MIGraphXExecutionProvider".to_string()), "{c:?}");
        assert!(r.iter().any(|e| e.ep == "VitisAIExecutionProvider" && e.status == RouteStatus::Excluded));
    }

    #[test]
    fn model_fit_follows_quantization_and_shape() {
        let qdq = ModelProfile { quantized: true, ..Default::default() };
        assert_eq!(assess_model(&qdq, "VitisAIExecutionProvider").fit, Fit::Good);
        let qdq_dyn = ModelProfile { quantized: true, dynamic_input_shapes: true, ..Default::default() };
        assert_eq!(assess_model(&qdq_dyn, "VitisAIExecutionProvider").fit, Fit::Risky);
        let fp32 = ModelProfile::default();
        assert_eq!(assess_model(&fp32, "VitisAIExecutionProvider").fit, Fit::Risky, "an unquantized model is not assumed to suit the NPU");
        assert_eq!(assess_model(&fp32, "MIGraphXExecutionProvider").fit, Fit::Good);
        assert_eq!(assess_model(&qdq, "MIGraphXExecutionProvider").fit, Fit::Risky);
        assert_eq!(assess_model(&fp32, "CPUExecutionProvider").fit, Fit::Good);
        assert_eq!(assess_model(&fp32, "SomethingElse").fit, Fit::Unknown);
    }

    #[test]
    fn risky_fits_stay_candidates_with_their_caveats() {
        let fp32 = ModelProfile::default();
        let fits = vec![assess_model(&fp32, "VitisAIExecutionProvider")];
        let r = route(&g16(DgpuPower::PoweredDown, 26200), BATTERY, &fits);
        let npu = r.iter().find(|e| e.ep == "VitisAIExecutionProvider").unwrap();
        assert_eq!(npu.status, RouteStatus::Candidate);
        assert!(npu.reason.contains("not quantized"), "{}", npu.reason);
    }

    #[test]
    fn gpu_classifiers() {
        assert!(is_amd_rdna3_or_later("AMD Radeon(TM) 890M Graphics"), "the real registry name carries a (TM) mark");
        assert!(is_amd_rdna3_or_later("AMD Radeon 890M Graphics"));
        assert!(is_amd_rdna3_or_later("AMD  Radeon™  780M"));
        assert!(is_amd_rdna3_or_later("AMD Radeon RX 7900 XTX"));
        assert!(!is_amd_rdna3_or_later("AMD Radeon RX 580"));
        assert!(is_nvidia_rtx_30_or_later("NVIDIA GeForce RTX 4070 Laptop GPU"));
        assert!(!is_nvidia_rtx_30_or_later("NVIDIA GeForce GTX 1660"));
    }
}
