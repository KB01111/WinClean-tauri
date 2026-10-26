//! Pure, cross-platform safety policy for WinClean 2.0.
//! No Windows calls here: every rule is unit-testable on any OS, and both the
//! unprivileged app and the elevated helper call the same functions, so the
//! helper never has to trust the caller's classification.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Services
// ---------------------------------------------------------------------------

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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ServiceCategory {
    WindowsCore,
    Security,
    Networking,
    Driver,
    Search,
    Update,
    Copilot,
    /// ASUS / hardware-vendor control components (fans, GPU mode, battery
    /// limit, ACPI/WMI bridges). G-Helper depends on these.
    VendorControl,
    Telemetry,
    Gaming,
    ThirdPartyUpdater,
    Optional,
    Unknown,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceClassification {
    pub known: bool,
    pub risk: ServiceRisk,
    pub category: ServiceCategory,
    /// Whether `Disabled` is an acceptable target. Unknown services can be
    /// moved between Automatic and Manual but never disabled.
    pub can_disable: bool,
    pub reason: String,
    pub recommendation: String,
}

/// Curated knowledge about services WinClean has an opinion on.
#[derive(Clone, Copy, Debug)]
pub struct ServiceKnowledge {
    pub name: &'static str,
    pub category: ServiceCategory,
    pub risk: ServiceRisk,
    /// Start type WinClean would recommend, when it recommends one.
    pub suggested: Option<&'static str>,
    pub can_disable: bool,
    pub why: &'static str,
}

const fn know(
    name: &'static str,
    category: ServiceCategory,
    risk: ServiceRisk,
    suggested: Option<&'static str>,
    can_disable: bool,
    why: &'static str,
) -> ServiceKnowledge {
    ServiceKnowledge { name, category, risk, suggested, can_disable, why }
}

use ServiceCategory as C;
use ServiceRisk as R;

/// Lower-cased service names that must never be changed by WinClean.
pub const PROTECTED_SERVICES: &[&str] = &[
    // Security
    "windefend", "wdnissvc", "wdnisdrv", "wdfilter", "sense", "wscsvc", "securityhealthservice",
    "mpssvc", "bfe", "keyiso", "vaultsvc", "samss", "lsm", "wbiosrvc", "tbs", "bdesvc", "sgrmbroker",
    "amsi", "appinfo", "seclogon2", "lsalso", "cryptsvc", "trustedinstaller",
    // Update
    "wuauserv", "bits", "usosvc", "waasmedicsvc", "dosvc", "installservice", "uhssvc",
    // Search (Copilot+/Recall semantic index relies on it)
    "wsearch",
    // Networking
    "dhcp", "dnscache", "nlasvc", "netprofm", "wlansvc", "wcmsvc", "netman", "wwansvc", "nsi",
    "lanmanworkstation", "lanmanserver", "iphlpsvc", "netsetupsvc", "rasman", "ikeext",
    // Core Windows plumbing
    "eventlog", "eventsystem", "rpcss", "rpceptmapper", "dcomlaunch", "winmgmt", "schedule",
    "profsvc", "usermanager", "gpsvc", "plugplay", "power", "systemeventsbroker",
    "brokerinfrastructure", "coremessagingregistrar", "deviceassociationservice", "storsvc",
    "staterepository", "tokenbroker", "timebrokersvc", "appxsvc", "clipsvc", "sens", "themes",
    "audiosrv", "audioendpointbuilder", "spooler", "dsmsvc", "devicesflowusersvc", "camsvc",
    // GPU / driver containers
    "amdwddmg", "nvlddmkm", "nvdisplay.containerlocalsystem",
    "amd external events utility",
    // Copilot+ / AI
    "wsaifabricsvc",
];

/// Prefixes (on the lower-cased service *name*) that are always protected.
const PROTECTED_NAME_PREFIXES: &[(&str, ServiceCategory)] = &[
    ("asus", C::VendorControl),
    ("atk", C::VendorControl),
    ("armoury", C::VendorControl),
    ("rog", C::VendorControl),
    ("amd", C::Driver),
    ("nvidia", C::Driver),
    ("nvcontainer", C::Driver),
    ("nvdisplay", C::Driver),
    ("nvtelemetry", C::Driver),
    ("nvlddmkm", C::Driver),
    ("rtk", C::Driver),
    ("realtek", C::Driver),
    ("intel", C::Driver),
    ("mtk", C::Driver),
    ("mediatek", C::Driver),
    ("killer", C::Driver),
    ("bth", C::Networking),
    ("wdi", C::WindowsCore),
];

/// Display-name fragments (lower-cased) that are always protected.
const PROTECTED_DISPLAY_FRAGMENTS: &[(&str, ServiceCategory)] = &[
    ("windows defender", C::Security),
    ("microsoft defender", C::Security),
    ("windows security", C::Security),
    ("windows firewall", C::Security),
    ("windows update", C::Update),
    ("update orchestrator", C::Update),
    ("windows search", C::Search),
    ("copilot", C::Copilot),
    ("windows ai", C::Copilot),
    ("studio effects", C::Copilot),
    ("recall", C::Copilot),
    ("amd crash defender", C::Driver),
    ("nvidia display", C::Driver),
    ("asus ", C::VendorControl),
    ("armoury crate", C::VendorControl),
];

pub const KNOWLEDGE: &[ServiceKnowledge] = &[
    // --- Caution: telemetry / optional shell features ---
    know("diagtrack", C::Telemetry, R::Caution, Some("manual"), true, "Connected User Experiences and Telemetry. Changing it affects diagnostics data and some troubleshooting tools."),
    know("dmwappushservice", C::Telemetry, R::Caution, Some("manual"), true, "WAP push routing used by telemetry/MDM scenarios."),
    know("mapsbroker", C::Optional, R::Caution, Some("manual"), true, "Downloaded Maps Manager. Only needed by the Maps app."),
    know("lfsvc", C::Optional, R::Caution, Some("manual"), true, "Geolocation service. Apps that use location start it on demand."),
    know("retaildemo", C::Optional, R::Caution, Some("disabled"), true, "Retail demo mode; never needed on a personal PC."),
    know("remoteregistry", C::Optional, R::Caution, Some("disabled"), true, "Lets remote users edit the registry; disabling is a security hardening step."),
    know("fax", C::Optional, R::Caution, Some("manual"), true, "Fax service; only needed with fax hardware."),
    know("xblauthmanager", C::Gaming, R::Caution, Some("manual"), true, "Xbox Live sign-in. Needed by Xbox/Game Pass games; starts on demand."),
    know("xblgamesave", C::Gaming, R::Caution, Some("manual"), true, "Xbox cloud saves. Starts on demand."),
    know("xboxnetapisvc", C::Gaming, R::Caution, Some("manual"), true, "Xbox networking. Starts on demand."),
    know("xboxgipsvc", C::Gaming, R::Caution, Some("manual"), true, "Xbox accessory management. Starts on demand."),
    know("wisvc", C::Optional, R::Caution, Some("manual"), true, "Windows Insider Service; unnecessary outside the Insider program."),
    know("wersvc", C::Telemetry, R::Caution, Some("manual"), true, "Windows Error Reporting."),
    know("pcasvc", C::Optional, R::Caution, None, true, "Program Compatibility Assistant. Low cost; WinClean makes no recommendation."),
    know("trkwks", C::Optional, R::Caution, Some("manual"), true, "Distributed Link Tracking Client; rarely useful on a single PC."),
    know("ssdpsrv", C::Networking, R::Caution, Some("manual"), true, "SSDP discovery of UPnP devices; a common network-exposure hardening target."),
    know("upnphost", C::Networking, R::Caution, Some("manual"), true, "UPnP device host."),
    know("wmpnetworksvc", C::Optional, R::Caution, Some("manual"), true, "Windows Media Player network sharing."),
    know("sysmain", C::WindowsCore, R::Caution, None, false, "Prefetch/SuperFetch. Benefit is small on NVMe, but WinClean never recommends disabling it."),
    know("tabletinputservice", C::Optional, R::Caution, None, false, "Touch keyboard and handwriting. Needed on touch/pen devices."),
    know("wpnservice", C::WindowsCore, R::Caution, None, false, "Push notifications; many apps rely on it."),
    know("lmhosts", C::Networking, R::Caution, Some("manual"), true, "NetBIOS-over-TCP helper; only for legacy name resolution."),
    know("seclogon", C::WindowsCore, R::Caution, Some("manual"), false, "Secondary Logon (run as a different user)."),
    know("workfolderssvc", C::Optional, R::Caution, Some("manual"), true, "Work Folders sync; only for enterprise setups."),
    know("wlidsvc", C::WindowsCore, R::Caution, None, false, "Microsoft account sign-in assistant."),
    // --- Safe: third-party updaters that do not need to run at boot ---
    know("steam client service", C::ThirdPartyUpdater, R::Safe, Some("manual"), true, "Steam helper; starts when Steam needs it."),
    know("adobeupdateservice", C::ThirdPartyUpdater, R::Safe, Some("manual"), true, "Adobe updater."),
    know("adobearmservice", C::ThirdPartyUpdater, R::Safe, Some("manual"), true, "Adobe Acrobat updater."),
    know("googleupdaterservice", C::ThirdPartyUpdater, R::Safe, Some("manual"), true, "Google updater."),
    know("gupdate", C::ThirdPartyUpdater, R::Safe, Some("manual"), true, "Google updater."),
    know("gupdatem", C::ThirdPartyUpdater, R::Safe, Some("manual"), true, "Google updater (manual)."),
    know("edgeupdate", C::ThirdPartyUpdater, R::Safe, Some("manual"), true, "Microsoft Edge updater."),
    know("edgeupdatem", C::ThirdPartyUpdater, R::Safe, Some("manual"), true, "Microsoft Edge updater (manual)."),
    know("mozillamaintenance", C::ThirdPartyUpdater, R::Safe, Some("manual"), true, "Firefox background updater."),
    know("dbupdate", C::ThirdPartyUpdater, R::Safe, Some("manual"), true, "Dropbox updater."),
    know("dbupdatem", C::ThirdPartyUpdater, R::Safe, Some("manual"), true, "Dropbox updater (manual)."),
    know("spotify elevation service", C::ThirdPartyUpdater, R::Safe, Some("manual"), true, "Spotify helper."),
    know("skypeupdate", C::ThirdPartyUpdater, R::Safe, Some("manual"), true, "Skype updater."),
];

/// Updater services recognised by pattern (version-suffixed names).
const SAFE_NAME_PREFIXES: &[&str] = &["googleupdater", "gupdate", "edgeupdate", "adobearm", "mozillamaintenance", "dbupdate"];

fn norm(name: &str) -> String {
    name.trim().to_ascii_lowercase()
}

/// Knowledge entry for a service name, if WinClean has one.
pub fn knowledge(name: &str, display_name: &str) -> Option<&'static ServiceKnowledge> {
    let n = norm(name);
    let d = norm(display_name);
    KNOWLEDGE.iter().find(|k| k.name == n || k.name == d)
}

/// The start type WinClean would recommend for a service, if any. Curated
/// entries carry their own suggestion; version-suffixed updater services
/// matched by pattern default to Manual.
pub fn suggested_start_type(name: &str, display_name: &str) -> Option<&'static str> {
    if let Some(k) = knowledge(name, display_name) {
        return k.suggested;
    }
    let c = classify_service(name, display_name);
    (c.known && c.risk == ServiceRisk::Safe && c.category == ServiceCategory::ThirdPartyUpdater).then_some("manual")
}

/// Services whose per-user instance carries a `_xxxxx` suffix (for example
/// `CDPUserSvc_5f3a1`); the base name decides the class.
fn base_service_name(n: &str) -> &str {
    match n.rsplit_once('_') {
        Some((base, suffix)) if !base.is_empty() && suffix.len() >= 4 && suffix.bytes().all(|b| b.is_ascii_hexdigit()) => base,
        _ => n,
    }
}

fn protected(category: ServiceCategory, reason: &str) -> ServiceClassification {
    ServiceClassification {
        known: true,
        risk: ServiceRisk::Protected,
        category,
        can_disable: false,
        reason: reason.to_string(),
        recommendation: "Leave unchanged. WinClean will never modify this service.".to_string(),
    }
}

/// Classifies a service from its SCM name and display name.
pub fn classify_service(name: &str, display_name: &str) -> ServiceClassification {
    classify_service_with_path(name, display_name, None)
}

/// Like [`classify_service`] but uses the service binary path as an extra
/// signal: a path inside the Windows directory marks an unknown service as
/// Microsoft-owned (still unknown, still not disable-able).
pub fn classify_service_with_path(name: &str, display_name: &str, binary_path: Option<&str>) -> ServiceClassification {
    let n = norm(name);
    let base = base_service_name(&n).to_string();
    let d = norm(display_name);

    if let Some(k) = knowledge(&base, &d) {
        if k.risk == ServiceRisk::Caution || k.risk == ServiceRisk::Safe {
            return ServiceClassification {
                known: true,
                risk: k.risk,
                category: k.category,
                can_disable: k.can_disable,
                reason: k.why.to_string(),
                recommendation: match k.suggested {
                    Some(s) => format!("Recommended start type: {s}. A snapshot is taken before any change."),
                    None => "No change recommended; you can still adjust it manually.".to_string(),
                },
            };
        }
    }

    if PROTECTED_SERVICES.contains(&base.as_str()) || PROTECTED_SERVICES.contains(&d.as_str()) {
        return protected(protected_category(&base), "Essential Windows, security, networking, driver, or Copilot+ component.");
    }
    if let Some((_, cat)) = PROTECTED_NAME_PREFIXES.iter().find(|(p, _)| base.starts_with(p)) {
        let reason = match cat {
            C::VendorControl => "ASUS firmware/fan/GPU-mode/battery control component. G-Helper and the platform depend on it; WinClean never touches it.",
            C::Driver => "Hardware driver or vendor service (AMD/NVIDIA/audio/Wi-Fi). Never changed by WinClean.",
            _ => "Essential platform component.",
        };
        return protected(*cat, reason);
    }
    if let Some((_, cat)) = PROTECTED_DISPLAY_FRAGMENTS.iter().find(|(f, _)| d.contains(f)) {
        return protected(*cat, "Essential Windows, security, vendor-control, or Copilot+ component.");
    }
    if SAFE_NAME_PREFIXES.iter().any(|p| base.starts_with(p)) {
        return ServiceClassification {
            known: true,
            risk: ServiceRisk::Safe,
            category: C::ThirdPartyUpdater,
            can_disable: true,
            reason: "Third-party updater that does not need to run at boot.".to_string(),
            recommendation: "Recommended start type: manual. Updates keep working via the app's own scheduled task.".to_string(),
        };
    }
    let microsoft_owned = binary_path
        .map(|p| {
            let l = p.to_ascii_lowercase();
            l.contains("\\windows\\system32\\") || l.contains("\\windows\\syswow64\\")
        })
        .unwrap_or(false);
    ServiceClassification {
        known: false,
        risk: ServiceRisk::Caution,
        category: C::Unknown,
        can_disable: false,
        reason: if microsoft_owned {
            "Windows component that WinClean has no recommendation for.".to_string()
        } else {
            "Unknown service. WinClean cannot prove it is safe to disable.".to_string()
        },
        recommendation: "Keep the current start type unless you recognize this service.".to_string(),
    }
}

fn protected_category(base: &str) -> ServiceCategory {
    match base {
        "windefend" | "wdnissvc" | "wdnisdrv" | "wdfilter" | "sense" | "wscsvc" | "securityhealthservice" | "mpssvc" | "bfe"
        | "keyiso" | "vaultsvc" | "samss" | "lsm" | "wbiosrvc" | "tbs" | "bdesvc" | "sgrmbroker" | "amsi" | "appinfo"
        | "cryptsvc" | "trustedinstaller" => C::Security,
        "wuauserv" | "bits" | "usosvc" | "waasmedicsvc" | "dosvc" | "installservice" | "uhssvc" => C::Update,
        "wsearch" => C::Search,
        "dhcp" | "dnscache" | "nlasvc" | "netprofm" | "wlansvc" | "wcmsvc" | "netman" | "wwansvc" | "nsi" | "lanmanworkstation"
        | "lanmanserver" | "iphlpsvc" | "netsetupsvc" | "rasman" | "ikeext" => C::Networking,
        "amdwddmg" | "nvlddmkm" | "nvdisplay.containerlocalsystem" | "amd external events utility" => C::Driver,
        "wsaifabricsvc" => C::Copilot,
        _ => C::WindowsCore,
    }
}

/// Which start-type changes a classification permits. Unknown services
/// may be switched between Automatic/Manual but never Disabled.
pub fn start_type_allowed(classification: &ServiceClassification, target_is_disabled: bool) -> bool {
    if classification.risk == ServiceRisk::Protected {
        return false;
    }
    if target_is_disabled && !classification.can_disable {
        return false;
    }
    true
}

pub fn is_protected_service(name: &str, display_name: &str) -> bool {
    classify_service(name, display_name).risk == ServiceRisk::Protected
}

/// Service names are validated before they reach the SCM so a malformed or
/// hostile name can never address anything but a plain service.
pub fn is_valid_service_name(name: &str) -> bool {
    !name.is_empty()
        && name.chars().count() <= 256
        && !name.contains(['\\', '/', '\0'])
        && !name.chars().any(|c| c.is_control())
}

// ---------------------------------------------------------------------------
// Processes / EcoQoS
// ---------------------------------------------------------------------------

/// Executables that must never receive EcoQoS throttling.
pub const PROTECTED_PROCESSES: &[&str] = &[
    // Editors, agents, toolchains
    "code.exe", "cursor.exe", "codex.exe", "claude.exe", "opencode.exe", "windsurf.exe", "zed.exe",
    "devenv.exe", "idea64.exe", "pycharm64.exe", "webstorm64.exe", "rustrover64.exe", "clion64.exe",
    "cargo.exe", "rustc.exe", "rust-analyzer.exe", "rustup.exe", "cl.exe", "link.exe", "msbuild.exe",
    "node.exe", "bun.exe", "deno.exe", "npm.exe", "pnpm.exe", "tsc.exe", "python.exe", "pythonw.exe",
    "git.exe", "gradle.exe", "java.exe", "dotnet.exe", "clang.exe", "gcc.exe", "ninja.exe", "cmake.exe",
    // Local inference
    "ollama.exe", "ollama app.exe", "ollama_llama_server.exe", "llama-server.exe", "lmstudio.exe",
    "lm studio.exe", "lms.exe", "lemonade-server.exe", "koboldcpp.exe",
    // Windows shell and security
    "explorer.exe", "dwm.exe", "csrss.exe", "winlogon.exe", "services.exe", "lsass.exe",
    "svchost.exe", "msmpeng.exe", "searchhost.exe", "textinputhost.exe", "shellexperiencehost.exe",
    "startmenuexperiencehost.exe", "ctfmon.exe", "audiodg.exe", "fontdrvhost.exe", "sihost.exe",
    // Hardware control (G-Helper / ASUS)
    "ghelper.exe", "armourycrate.service.exe", "asusoptimization.exe", "asussystemanalysis.exe",
    // Remote / VPN / audio-video calls must stay responsive
    "teams.exe", "ms-teams.exe", "zoom.exe", "discord.exe", "obs64.exe", "tailscaled.exe", "tailscale-ipn.exe",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EcoQosDecision {
    Protected,
    Foreground,
    Eligible,
    Unknown,
}

/// How eagerly background work is moved to EcoQoS.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum EcoQosMode {
    /// Never throttle anything.
    Off,
    /// Updaters, indexers, telemetry, crash reporters, cloud sync.
    #[default]
    Conservative,
    /// Conservative + known idle background helpers. Battery profiles only.
    Battery,
}

/// Name fragments that make background work eligible in `Conservative`.
const ECO_ELIGIBLE_FRAGMENTS: &[&str] = &["update", "index", "telemetry", "crashpad", "onedrive", "searchindexer"];
/// Extra background helpers eligible in `Battery` mode.
const ECO_BATTERY_FRAGMENTS: &[&str] = &["dropbox", "googledrivefs", "ccxprocess", "creativecloud", "adobe", "spotifywebhelper", "gamebar", "widgets", "phoneexperiencehost", "yourphone"];

/// CPU at or above this share of one core means throttling would be felt.
pub const ECO_BUSY_CPU_PERCENT: f32 = 25.0;

pub fn classify_process_for_ecoqos(exe_name: &str, is_foreground: bool, cpu_percent_recent: f32) -> EcoQosDecision {
    classify_process_for_ecoqos_mode(exe_name, is_foreground, cpu_percent_recent, EcoQosMode::Conservative)
}

pub fn classify_process_for_ecoqos_mode(exe_name: &str, is_foreground: bool, cpu_percent_recent: f32, mode: EcoQosMode) -> EcoQosDecision {
    if is_foreground {
        return EcoQosDecision::Foreground;
    }
    let n = norm(exe_name);
    if PROTECTED_PROCESSES.iter().any(|p| n == norm(p)) {
        return EcoQosDecision::Protected;
    }
    if mode == EcoQosMode::Off {
        return EcoQosDecision::Unknown;
    }
    // Busy background work (compilers, inference) can slip past the name list;
    // sustained CPU use means throttling would be felt. A missing sample
    // (negative) is never treated as idle.
    if cpu_percent_recent < 0.0 || cpu_percent_recent >= ECO_BUSY_CPU_PERCENT {
        return EcoQosDecision::Unknown;
    }
    let by_name = ECO_ELIGIBLE_FRAGMENTS.iter().any(|f| n.contains(f))
        || (mode == EcoQosMode::Battery && ECO_BATTERY_FRAGMENTS.iter().any(|f| n.contains(f)));
    if by_name {
        EcoQosDecision::Eligible
    } else {
        EcoQosDecision::Unknown
    }
}

// ---------------------------------------------------------------------------
// Power profiles
// ---------------------------------------------------------------------------

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

impl PowerProfileId {
    pub fn as_str(&self) -> &'static str {
        match self {
            PowerProfileId::BatterySaver => "batterySaver",
            PowerProfileId::Balanced => "balanced",
            PowerProfileId::Coding => "coding",
            PowerProfileId::Gaming => "gaming",
            PowerProfileId::AiDevelopment => "aiDevelopment",
            PowerProfileId::Custom => "custom",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PowerProfile {
    /// `batterySaver | balanced | coding | gaming | aiDevelopment` or `custom:<id>`.
    pub id: String,
    pub label: String,
    /// Windows power scheme GUID to activate, when known.
    pub scheme_guid: Option<String>,
    pub eco_qos_for_background: bool,
    pub eco_qos_mode: EcoQosMode,
    pub builtin: bool,
    pub description: String,
}

pub const SCHEME_BALANCED: &str = "381b4222-f694-41f0-9685-ff5bb260df2e";
pub const SCHEME_POWER_SAVER: &str = "a1841308-3541-4fab-bc81-f71556f20b4a";
pub const SCHEME_HIGH_PERFORMANCE: &str = "8c5e7fda-e8bf-4a96-9a85-a6e23a8c635c";
pub const SCHEME_ULTIMATE: &str = "e9a42b02-d5df-448d-aa00-03f14749eb61";

/// Scheme GUIDs a *custom* profile may reference without enumerating the
/// machine: the stock Windows plans.
pub const STOCK_SCHEMES: &[&str] = &[SCHEME_BALANCED, SCHEME_POWER_SAVER, SCHEME_HIGH_PERFORMANCE, SCHEME_ULTIMATE];

fn profile(id: PowerProfileId, label: &str, scheme: &str, mode: EcoQosMode, description: &str) -> PowerProfile {
    PowerProfile {
        id: id.as_str().to_string(),
        label: label.to_string(),
        scheme_guid: Some(scheme.to_string()),
        eco_qos_for_background: mode != EcoQosMode::Off,
        eco_qos_mode: mode,
        builtin: true,
        description: description.to_string(),
    }
}

pub fn builtin_profiles() -> Vec<PowerProfile> {
    vec![
        profile(PowerProfileId::BatterySaver, "Battery Saver", SCHEME_POWER_SAVER, EcoQosMode::Battery, "Power Saver scheme + EcoQoS for idle background helpers. Editors, compilers and inference stay untouched."),
        profile(PowerProfileId::Balanced, "Balanced", SCHEME_BALANCED, EcoQosMode::Conservative, "Windows Balanced scheme. Default on AC and DC."),
        profile(PowerProfileId::Coding, "Coding", SCHEME_BALANCED, EcoQosMode::Conservative, "Balanced scheme; editors, language servers and compilers are never throttled."),
        profile(PowerProfileId::Gaming, "Gaming", SCHEME_HIGH_PERFORMANCE, EcoQosMode::Off, "High Performance scheme; no background throttling while gaming."),
        profile(PowerProfileId::AiDevelopment, "AI Development", SCHEME_HIGH_PERFORMANCE, EcoQosMode::Off, "High Performance; inference processes protected, NPU preferred when verified."),
    ]
}

pub const MAX_CUSTOM_PROFILES: usize = 12;

/// Validates a user-defined profile. Custom profiles can only reference a
/// stock power scheme or a well-formed GUID, and can never request an EcoQoS
/// mode broader than the three built-in modes.
pub fn validate_custom_profile(p: &PowerProfile) -> Result<(), String> {
    if !p.id.starts_with("custom:") || p.id.len() < 8 || p.id.len() > 48 || !p.id[7..].bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
        return Err("custom profile id must look like custom:<letters-digits-dashes>".into());
    }
    let label = p.label.trim();
    if label.is_empty() || label.chars().count() > 40 || label.chars().any(|c| c.is_control()) {
        return Err("profile name must be 1-40 printable characters".into());
    }
    if p.description.chars().count() > 200 {
        return Err("description is limited to 200 characters".into());
    }
    match &p.scheme_guid {
        Some(g) if crate::optimizer::power::is_valid_guid(g) => {}
        Some(_) => return Err("scheme GUID is not a valid GUID".into()),
        None => return Err("custom profiles must name a Windows power scheme".into()),
    }
    Ok(())
}

/// Deterministic profile suggestion from power state alone.
pub fn suggest_profile(on_ac: bool, battery_percent: Option<u8>, foreground_is_game_or_inference: bool) -> PowerProfileId {
    if foreground_is_game_or_inference {
        return PowerProfileId::Gaming;
    }
    match (on_ac, battery_percent) {
        (false, Some(p)) if p <= 20 => PowerProfileId::BatterySaver,
        _ => PowerProfileId::Balanced,
    }
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
        assert!(is_protected_service("UsoSvc", "Update Orchestrator"));
        assert!(is_protected_service("BFE", "Base Filtering Engine"));
        assert!(!start_type_allowed(&classify_service("SomeRandomSvc", "Some Random Service"), true));
        assert!(start_type_allowed(&classify_service("SomeRandomSvc", "Some Random Service"), false));
        assert!(start_type_allowed(&classify_service("DiagTrack", "Diagnostics Tracking"), true));
    }

    #[test]
    fn asus_amd_nvidia_and_copilot_components_are_protected() {
        for (name, display) in [
            ("AsusOptimization", "ASUS Optimization"),
            ("ArmouryCrateService", "ArmouryCrate Service"),
            ("ATKWMIACPIIO", "ASUS System Control Interface"),
            ("AMD External Events Utility", "AMD External Events Utility"),
            ("AMDRyzenMasterDriverV30", "AMD Ryzen Master"),
            ("amdfendr", "AMD Crash Defender"),
            ("NvContainerLocalSystem", "NVIDIA LocalSystem Container"),
            ("NVDisplay.ContainerLocalSystem", "NVIDIA Display Container LS"),
            ("WSAIFabricSvc", "Windows AI Fabric Service"),
            ("RtkAudioUniversalService", "Realtek Audio Universal Service"),
            ("BthAvctpSvc", "AVCTP service"),
        ] {
            assert!(is_protected_service(name, display), "{name} must be protected");
            assert!(!start_type_allowed(&classify_service(name, display), false), "{name} must reject every change");
        }
        assert_eq!(classify_service("AsusOptimization", "x").category, ServiceCategory::VendorControl);
    }

    #[test]
    fn per_user_service_suffixes_use_the_base_name() {
        assert!(is_protected_service("WpnUserService_5f3a1", "x") || classify_service("WpnUserService_5f3a1", "x").risk == ServiceRisk::Caution);
        assert_eq!(base_service_name("cdpusersvc_5f3a1"), "cdpusersvc");
        assert_eq!(base_service_name("plain_name"), "plain_name");
    }

    #[test]
    fn unknown_services_can_change_between_automatic_and_manual_but_never_disable() {
        let c = classify_service("MysteryVendorSvc", "Mystery Vendor Service");
        assert!(!c.known);
        assert!(!c.can_disable);
        assert!(start_type_allowed(&c, false));
        assert!(!start_type_allowed(&c, true));
    }

    #[test]
    fn sysmain_is_known_but_never_disable_able() {
        let c = classify_service("SysMain", "SysMain");
        assert!(c.known);
        assert_eq!(c.risk, ServiceRisk::Caution);
        assert!(!start_type_allowed(&c, true));
        assert!(start_type_allowed(&c, false));
    }

    #[test]
    fn updater_patterns_are_safe_and_version_suffixes_match() {
        let c = classify_service("GoogleUpdaterInternalService148.0.1", "Google Updater Internal Service");
        assert_eq!(c.risk, ServiceRisk::Safe);
        assert!(c.can_disable);
    }

    #[test]
    fn microsoft_path_does_not_make_unknown_services_disable_able() {
        let c = classify_service_with_path("Odd", "Odd", Some("C:\\Windows\\System32\\odd.exe"));
        assert!(!c.known && !c.can_disable);
        assert!(c.reason.contains("Windows component"));
    }

    #[test]
    fn service_names_are_validated_before_reaching_the_scm() {
        assert!(is_valid_service_name("wuauserv"));
        assert!(is_valid_service_name("Steam Client Service"));
        for bad in ["", "a\\b", "a/b", "a\0b", &"x".repeat(257), "line\nbreak"] {
            assert!(!is_valid_service_name(bad), "{bad:?}");
        }
    }

    #[test]
    fn editors_and_compilers_never_throttled() {
        assert_eq!(classify_process_for_ecoqos("Code.exe", false, 1.0), EcoQosDecision::Protected);
        assert_eq!(classify_process_for_ecoqos("rustc.exe", false, 90.0), EcoQosDecision::Protected);
        assert_eq!(classify_process_for_ecoqos("anything.exe", true, 1.0), EcoQosDecision::Foreground);
        assert_eq!(classify_process_for_ecoqos("GoogleUpdate.exe", false, 0.5), EcoQosDecision::Eligible);
        assert_eq!(classify_process_for_ecoqos("unknown.exe", false, 80.0), EcoQosDecision::Unknown);
        for exe in ["cursor.exe", "codex.exe", "claude.exe", "opencode.exe", "node.exe", "cargo.exe", "ollama.exe", "GHelper.exe"] {
            for mode in [EcoQosMode::Off, EcoQosMode::Conservative, EcoQosMode::Battery] {
                assert_eq!(classify_process_for_ecoqos_mode(exe, false, 0.0, mode), EcoQosDecision::Protected, "{exe} {mode:?}");
            }
        }
    }

    #[test]
    fn busy_or_unsampled_background_work_is_never_eligible() {
        assert_eq!(classify_process_for_ecoqos("GoogleUpdate.exe", false, 25.0), EcoQosDecision::Unknown);
        assert_eq!(classify_process_for_ecoqos("SearchIndexer.exe", false, 80.0), EcoQosDecision::Unknown);
        assert_eq!(classify_process_for_ecoqos("GoogleUpdate.exe", false, 24.9), EcoQosDecision::Eligible);
        assert_eq!(classify_process_for_ecoqos("OneDrive.exe", false, -1.0), EcoQosDecision::Unknown);
        assert_eq!(classify_process_for_ecoqos("OneDrive.exe", false, 0.0), EcoQosDecision::Eligible);
    }

    #[test]
    fn ecoqos_mode_gates_how_much_is_eligible() {
        assert_eq!(classify_process_for_ecoqos_mode("GoogleUpdate.exe", false, 0.0, EcoQosMode::Off), EcoQosDecision::Unknown);
        assert_eq!(classify_process_for_ecoqos_mode("Dropbox.exe", false, 0.0, EcoQosMode::Conservative), EcoQosDecision::Unknown);
        assert_eq!(classify_process_for_ecoqos_mode("Dropbox.exe", false, 0.0, EcoQosMode::Battery), EcoQosDecision::Eligible);
        // Foreground and protected always win over mode.
        assert_eq!(classify_process_for_ecoqos_mode("Dropbox.exe", true, 0.0, EcoQosMode::Battery), EcoQosDecision::Foreground);
    }

    #[test]
    fn builtin_profiles_cover_the_required_set() {
        let ids: Vec<String> = builtin_profiles().into_iter().map(|p| p.id).collect();
        assert_eq!(ids, ["batterySaver", "balanced", "coding", "gaming", "aiDevelopment"]);
        let gaming = builtin_profiles().into_iter().find(|p| p.id == "gaming").unwrap();
        assert!(!gaming.eco_qos_for_background);
        assert_eq!(gaming.eco_qos_mode, EcoQosMode::Off);
    }

    #[test]
    fn custom_profiles_are_validated() {
        let mut p = PowerProfile {
            id: "custom:mine-1".into(),
            label: "Mine".into(),
            scheme_guid: Some(SCHEME_BALANCED.into()),
            eco_qos_for_background: true,
            eco_qos_mode: EcoQosMode::Conservative,
            builtin: false,
            description: String::new(),
        };
        assert!(validate_custom_profile(&p).is_ok());
        p.id = "balanced".into();
        assert!(validate_custom_profile(&p).is_err(), "must not shadow a built-in id");
        p.id = "custom:ok".into();
        p.scheme_guid = Some("not-a-guid".into());
        assert!(validate_custom_profile(&p).is_err());
        p.scheme_guid = None;
        assert!(validate_custom_profile(&p).is_err());
        p.scheme_guid = Some(SCHEME_BALANCED.into());
        p.label = "  ".into();
        assert!(validate_custom_profile(&p).is_err());
    }

    #[test]
    fn low_battery_suggests_saver() {
        assert_eq!(suggest_profile(false, Some(12), false), PowerProfileId::BatterySaver);
        assert_eq!(suggest_profile(true, Some(12), false), PowerProfileId::Balanced);
        assert_eq!(suggest_profile(false, Some(90), true), PowerProfileId::Gaming);
    }
}
