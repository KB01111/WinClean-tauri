//! Capability detection: Windows build, On-device Agent Registry (MCP),
//! and Windows ML execution-provider probing. Discovery only — nothing is
//! enabled, installed, or executed here.

use serde::{Deserialize, Serialize};

use super::policy::route_inference;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityReport {
    pub windows_build: Option<u32>,
    pub copilot_plus_capable: bool,
    pub mcp_agent_registry_found: bool,
    pub mcp_registry_paths: Vec<String>,
    pub file_explorer_mcp: String,
    pub execution_providers: Vec<ExecutionProvider>,
    pub inference_route: Vec<String>,
    pub npu_verified_for_inference: bool,
    pub notes: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionProvider {
    pub name: String,
    pub status: String, // "present" | "notFound" | "unknown"
    pub detail: String,
}

fn ep(name: &str, dll_hints: &[&str]) -> ExecutionProvider {
    // Presence of the provider DLL next to the app or in System32 counts as
    // "present but unverified": a real model+session test is still required.
    let found = dll_hints.iter().any(|d| {
        std::path::Path::new(d).exists()
            || std::env::var_os("WINDIR").map(|w| std::path::Path::new(&w).join("System32").join(d).exists()).unwrap_or(false)
    });
    ExecutionProvider {
        name: name.to_string(),
        status: if found { "present".into() } else { "notFound".into() },
        detail: if found {
            "DLL found. Requires an on-device model/session test before WinClean routes inference here.".into()
        } else {
            "DLL not found. Install the matching Windows ML / ONNX Runtime package to enable.".into()
        },
    }
}

pub fn capability_report(npu_present: bool, igpu_available: bool) -> CapabilityReport {
    // On-device Agent Registry: %LOCALAPPDATA%\Microsoft\Windows\Agents and
    // the packaged-app agent manifests. Existence check only, no execution.
    let local = std::env::var_os("LOCALAPPDATA").map(std::path::PathBuf::from);
    let mut paths: Vec<String> = Vec::new();
    if let Some(l) = &local {
        for rel in ["Microsoft\\Windows\\Agents", "Microsoft\\WindowsApps"] {
            let p = l.join(rel);
            if p.exists() { paths.push(p.to_string_lossy().into_owned()); }
        }
    }
    let build = windows_build();
    let providers = vec![
        ep("VitisAIExecutionProvider", &["onnxruntime_vitisai_ep.dll", "vitisaiep.dll"]),
        ep("MIGraphXExecutionProvider", &["onnxruntime_migraphx_ep.dll"]),
        ep("DirectMLExecutionProvider", &["onnxruntime_directml_ep.dll", "DirectML.dll"]),
        ep("CPUExecutionProvider", &["onnxruntime.dll"]),
    ];
    CapabilityReport {
        windows_build: build,
        copilot_plus_capable: npu_present && build.map(|b| b >= 26100).unwrap_or(false),
        mcp_agent_registry_found: !paths.is_empty(),
        mcp_registry_paths: paths,
        file_explorer_mcp: "Discovery only. File Explorer MCP requires the connector app registered in the On-device Agent Registry and per-file user consent in Explorer. WinClean keeps its native scanner and never reads files through MCP without that consent.".into(),
        execution_providers: providers,
        inference_route: route_inference(false, igpu_available, false).iter().map(|s| s.to_string()).collect(),
        npu_verified_for_inference: false,
        notes: vec![
            "NPU inference stays disabled until a Windows ML session test with a real model passes on this device (see docs/winclean-2.0.md).".into(),
            "Microsoft Learn MCP is a development-time documentation server; it is not bundled into the shipped app.".into(),
        ],
    }
}

#[cfg(windows)]
fn windows_build() -> Option<u32> {
    // RtlGetVersion via ntdll would need an extra binding; the registry
    // CurrentBuildNumber is the documented, read-only equivalent.
    use windows_sys::Win32::System::Registry::{RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY_LOCAL_MACHINE, KEY_QUERY_VALUE};
    let sub: Vec<u16> = "SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion".encode_utf16().chain([0]).collect();
    let val: Vec<u16> = "CurrentBuildNumber".encode_utf16().chain([0]).collect();
    let mut hkey = std::ptr::null_mut();
    if unsafe { RegOpenKeyExW(HKEY_LOCAL_MACHINE, sub.as_ptr(), 0, KEY_QUERY_VALUE, &mut hkey) } != 0 { return None; }
    let mut ty = 0u32;
    let mut size = 64u32;
    let mut buf = [0u16; 32];
    let rc = unsafe { RegQueryValueExW(hkey, val.as_ptr(), std::ptr::null_mut(), &mut ty, buf.as_mut_ptr() as *mut u8, &mut size) };
    unsafe { RegCloseKey(hkey); }
    if rc != 0 { return None; }
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end]).trim().parse().ok()
}

#[cfg(not(windows))]
fn windows_build() -> Option<u32> { None }
