//! Pure, cross-platform safety policy for WinClean 2.0.
//! No Windows calls here: every rule is unit-testable on any OS.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ServiceRisk {
    /// Never mutable from WinClean (security, update, drivers, networking core).
    Protected,
    /// Mutable only after explicit approval; snapshot + rollback required.
    Caution,
    /// Safe to switch between Manual / Automatic (Delayed) / Disabled.
    Safe,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceClassification {
    pub risk: ServiceRisk,
    pub reason: String,
    pub recommendation: String,
}

/// Lower-cased service names that must never be changed by WinClean.
pub const PROTECTED_SERVICES: &[&str] = &[
    "windefend", "sense", "wscsvc", "securityhealthservice", "mpssvc",
    "wuauserv", "bits", "cryptsvc", "trustedinstaller", "wsearch",
    "dhcp", "dnscache", "nlaSvc", "netprofm", "wlanSvc", "audiosrv",
    "audioendpointbuilder", "eventlog", "rpcss", "dcomlaunch", "lsm",
    "samss", "winmgmt", "schedule", "profSvc", "userManager", "gpsvc",
    "spooler", "plugplay", "power", "systemEventsBroker", "brokerInfrastructure",
    "coreMessagingRegistrar", "deviceAssociationService", "installservice",
    "storSvc", "amsi", "wdnisdrv", "wdfilter", "amdwddmg", "nvlddmkm",
    "amd external events utility", "nvdisplay.containerlocalsystem",
];

const CAUTION_SERVICES: &[&str] = &[
    "diagtrack", "dmwappushservice", "mapsbroker", "lfsvc", "retaildemo",
    "remoteRegistry", "fax", "xblauthmanager", "xblgamesave", "xboxnetapisvc",
    "xboxgipsvc", "wisvc", "werSvc", "pcaSvc", "trkwks", "seclogon",
];

const SAFE_SERVICES: &[&str] = &[
    "steam client service", "adobeupdateService", "googleupdaterservice",
    "edgeupdate", "gupdate", "skypeupdate", "spotify elevation service",
];

/// Trim a policy name and fold ASCII letters to lowercase for comparisons.
fn norm(name: &str) -> String {
    name.trim().to_ascii_lowercase()
}

/// Classify service names and display names with deterministic protection and risk rules.
/// Unknown services default to caution with a recommendation to retain their current start type.
pub fn classify_service(name: &str, display_name: &str) -> ServiceClassification {
    let n = norm(name);
    let d = norm(display_name);
    let hit = |list: &[&str]| list.iter().any(|p| n == norm(p) || d == norm(p));
    if hit(PROTECTED_SERVICES)
        || d.contains("windows defender")
        || d.contains("windows update")
        || d.contains("windows search")
        || d.contains("amd crash defender")
        || d.contains("nvidia display")
        || d.contains("copilot")
    {
        return ServiceClassification {
            risk: ServiceRisk::Protected,
            reason: "Essential Windows, security, networking, driver, or Copilot+ component.".into(),
            recommendation: "Leave unchanged. WinClean will never modify this service.".into(),
        };
    }
    if hit(CAUTION_SERVICES) {
        return ServiceClassification {
            risk: ServiceRisk::Caution,
            reason: "Telemetry, diagnostics, or optional shell feature. Changing it can affect troubleshooting or Store/Xbox features.".into(),
            recommendation: "Manual is the balanced choice. Snapshot is taken before any change.".into(),
        };
    }
    if hit(SAFE_SERVICES) {
        return ServiceClassification {
            risk: ServiceRisk::Safe,
            reason: "Third-party updater/helper that does not need to run at boot.".into(),
            recommendation: "Manual (trigger start) keeps updates available without boot cost.".into(),
        };
    }
    ServiceClassification {
        risk: ServiceRisk::Caution,
        reason: "Unknown service. WinClean cannot prove it is safe to disable.".into(),
        recommendation: "Keep current start type unless you recognize this service.".into(),
    }
}

/// Return whether the service policy forbids changing the supplied service.
pub fn is_protected_service(name: &str, display_name: &str) -> bool {
    classify_service(name, display_name).risk == ServiceRisk::Protected
}

/// Executables that must never receive EcoQoS throttling.
pub const PROTECTED_PROCESSES: &[&str] = &[
    "code.exe", "cursor.exe", "codex.exe", "claude.exe", "opencode.exe",
    "devenv.exe", "idea64.exe", "pycharm64.exe", "webstorm64.exe",
    "cargo.exe", "rustc.exe", "cl.exe", "link.exe", "msbuild.exe",
    "node.exe", "bun.exe", "python.exe", "ollama.exe", "lmstudio.exe",
    "explorer.exe", "dwm.exe", "csrss.exe", "winlogon.exe", "services.exe",
    "lsass.exe", "svchost.exe", "msmpeng.exe", "searchhost.exe",
    "textinputhost.exe", "shellexperiencehost.exe", "startmenuexperiencehost.exe",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EcoQosDecision {
    Protected,
    Foreground,
    Eligible,
    Unknown,
}

/// Classify EcoQoS eligibility from the executable name, foreground state, and recent CPU usage.
/// Protect listed workloads and foreground apps; leave busy or unrecognized work unclassified.
pub fn classify_process_for_ecoqos(
    exe_name: &str,
    is_foreground: bool,
    cpu_percent_recent: f32,
) -> EcoQosDecision {
    if is_foreground {
        return EcoQosDecision::Foreground;
    }
    let n = norm(exe_name);
    if PROTECTED_PROCESSES.iter().any(|p| n == norm(p)) {
        return EcoQosDecision::Protected;
    }
    // Busy background work (compilers, inference) can slip past the name list;
    // sustained CPU use means throttling would be felt. Stay conservative.
    if cpu_percent_recent >= 25.0 {
        return EcoQosDecision::Unknown;
    }
    if n.contains("update") || n.contains("index") || n.contains("telemetry")
        || n.contains("crashpad") || n.contains("onedrive") || n.contains("searchindexer")
    {
        return EcoQosDecision::Eligible;
    }
    EcoQosDecision::Unknown
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PowerProfileId {
    BatterySaver,
    Balanced,
    Coding,
    Gaming,
    AiDevelopment,
    Custom,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PowerProfile {
    pub id: PowerProfileId,
    pub label: String,
    /// Windows power scheme GUID to activate, when known.
    pub scheme_guid: Option<String>,
    pub eco_qos_for_background: bool,
    pub description: String,
}

pub const SCHEME_BALANCED: &str = "381b4222-f694-41f0-9685-ff5bb260df2e";
pub const SCHEME_POWER_SAVER: &str = "a1841308-3541-4fab-bc81-f71556f20b4a";
pub const SCHEME_HIGH_PERFORMANCE: &str = "8c5e7fda-e8bf-4a96-9a85-a6e23a8c635c";
pub const SCHEME_ULTIMATE: &str = "e9a42b02-d5df-448d-aa00-03f14749eb61";

/// Return built-in profile metadata, Windows scheme GUIDs, and background EcoQoS preferences.
pub fn builtin_profiles() -> Vec<PowerProfile> {
    vec![
        PowerProfile { id: PowerProfileId::BatterySaver, label: "Battery Saver".into(), scheme_guid: Some(SCHEME_POWER_SAVER.into()), eco_qos_for_background: true, description: "Power Saver scheme + EcoQoS for eligible background work.".into() },
        PowerProfile { id: PowerProfileId::Balanced, label: "Balanced".into(), scheme_guid: Some(SCHEME_BALANCED.into()), eco_qos_for_background: true, description: "Windows Balanced scheme. Default on AC and DC.".into() },
        PowerProfile { id: PowerProfileId::Coding, label: "Coding".into(), scheme_guid: Some(SCHEME_BALANCED.into()), eco_qos_for_background: true, description: "Balanced scheme; editors/compilers are EcoQoS-protected.".into() },
        PowerProfile { id: PowerProfileId::Gaming, label: "Gaming".into(), scheme_guid: Some(SCHEME_HIGH_PERFORMANCE.into()), eco_qos_for_background: false, description: "High Performance scheme; no background throttling while gaming.".into() },
        PowerProfile { id: PowerProfileId::AiDevelopment, label: "AI Development".into(), scheme_guid: Some(SCHEME_HIGH_PERFORMANCE.into()), eco_qos_for_background: false, description: "High Performance; inference processes protected, NPU preferred when verified.".into() },
    ]
}

/// Deterministic profile suggestion. Pure so the AI layer can only
/// propose what this function already allows.
pub fn suggest_profile(on_ac: bool, battery_percent: Option<u8>, foreground_is_game_or_inference: bool) -> PowerProfileId {
    if foreground_is_game_or_inference {
        return PowerProfileId::Gaming;
    }
    match (on_ac, battery_percent) {
        (false, Some(p)) if p <= 20 => PowerProfileId::BatterySaver,
        (false, Some(p)) if p <= 50 => PowerProfileId::Balanced,
        _ => PowerProfileId::Balanced,
    }
}

/// Inference routing order. Never puts an unverified NPU first.
pub fn route_inference(npu_verified: bool, igpu_available: bool, dgpu_awake: bool) -> Vec<&'static str> {
    let mut order: Vec<&'static str> = Vec::new();
    if npu_verified {
        order.push("VitisAIExecutionProvider (XDNA 2 NPU)");
    }
    if igpu_available {
        order.push("MIGraphXExecutionProvider / DirectML (Radeon 890M)");
    }
    if dgpu_awake {
        order.push("CUDA / DirectML (RTX 4070, already awake)");
    }
    order.push("CPU (always available)");
    order
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defender_and_update_are_protected() {
        assert!(is_protected_service("WinDefend", "Microsoft Defender Antivirus Service"));
        assert!(is_protected_service("wuauserv", "Windows Update"));
        assert!(is_protected_service("WSearch", "Windows Search"));
        assert_eq!(classify_service("DiagTrack", "Diagnostics Tracking").risk, ServiceRisk::Caution);
    }

    #[test]
    fn editors_and_compilers_never_throttled() {
        assert_eq!(classify_process_for_ecoqos("Code.exe", false, 1.0), EcoQosDecision::Protected);
        assert_eq!(classify_process_for_ecoqos("rustc.exe", false, 90.0), EcoQosDecision::Protected);
        assert_eq!(classify_process_for_ecoqos("anything.exe", true, 1.0), EcoQosDecision::Foreground);
        assert_eq!(classify_process_for_ecoqos("GoogleUpdate.exe", false, 0.5), EcoQosDecision::Eligible);
        assert_eq!(classify_process_for_ecoqos("unknown.exe", false, 80.0), EcoQosDecision::Unknown);
    }

    #[test]
    fn npu_only_routed_first_when_verified() {
        assert_eq!(route_inference(false, true, false)[0], "MIGraphXExecutionProvider / DirectML (Radeon 890M)");
        assert!(route_inference(true, true, false)[0].contains("VitisAI"));
        // Sleeping dGPU must never appear as a routing target.
        assert!(route_inference(true, true, false).iter().all(|p| !p.contains("RTX")));
    }

    #[test]
    fn low_battery_suggests_saver() {
        assert_eq!(suggest_profile(false, Some(12), false), PowerProfileId::BatterySaver);
        assert_eq!(suggest_profile(true, Some(12), false), PowerProfileId::Balanced);
        assert_eq!(suggest_profile(false, Some(90), true), PowerProfileId::Gaming);
    }
}
