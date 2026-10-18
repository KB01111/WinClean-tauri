//! Battery telemetry (GetSystemPowerStatus) + power schemes (PowrProf).
//! Discharge watts are derived from sampled capacity deltas by the caller
//! and persisted as power history; no mock values are ever produced.

use serde::{Deserialize, Serialize};

use super::policy::builtin_profiles;
pub use super::policy::{PowerProfile, PowerProfileId};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatteryStatus {
    pub present: bool,
    pub on_ac: bool,
    /// False when Windows reports ACLineStatus=255 (unknown): on_ac is then meaningless and must not drive DC rules.
    pub ac_known: bool,
    pub percent: Option<u8>,
    pub seconds_remaining: Option<i32>,
    pub active_scheme_guid: Option<String>,
    pub active_scheme_name: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PowerSample {
    pub at_ms: i64,
    pub percent: u8,
    pub on_ac: bool,
}

/// Discharge watts between two samples; None when charging or unusable.
pub fn discharge_watts(prev: &PowerSample, next: &PowerSample, design_capacity_mwh: Option<f64>) -> Option<f64> {
    if prev.on_ac || next.on_ac || next.at_ms <= prev.at_ms || next.percent >= prev.percent {
        return None;
    }
    let capacity = design_capacity_mwh?;
    let hours = (next.at_ms - prev.at_ms) as f64 / 3_600_000.0;
    let used_mwh = (prev.percent as f64 - next.percent as f64) / 100.0 * capacity;
    let w = used_mwh / hours / 1000.0;
    if w.is_finite() && w > 0.0 { Some((w * 10.0).round() / 10.0) } else { None }
}

/// Estimate remaining minutes from charge percentage, watts, and design capacity in mWh.
/// Return `None` when discharge power or design capacity is nonpositive.
pub fn estimated_runtime_minutes(percent: u8, discharge_w: f64, design_capacity_mwh: f64) -> Option<f64> {
    if discharge_w <= 0.0 || design_capacity_mwh <= 0.0 { return None; }
    Some((percent as f64 / 100.0 * design_capacity_mwh / 1000.0) / discharge_w * 60.0)
}

#[cfg(windows)]
mod win {
    use super::*;
    use windows_sys::Win32::System::Power::{
        GetSystemPowerStatus, PowerGetActiveScheme, PowerReadFriendlyName, SYSTEM_POWER_STATUS,
    };
    use windows_sys::Win32::System::Registry::{RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY_LOCAL_MACHINE, KEY_READ};

    /// Format a Windows GUID as a lowercase, hyphen-separated string.
    fn guid_string(g: &windows_sys::core::GUID) -> String {
        format!("{:08x}-{:04x}-{:04x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
            g.data1, g.data2, g.data3, g.data4[0], g.data4[1], g.data4[2], g.data4[3], g.data4[4], g.data4[5], g.data4[6], g.data4[7])
    }

    /// Read a power scheme’s friendly name, returning `None` if Windows cannot supply it.
    fn scheme_name(guid: &windows_sys::core::GUID) -> Option<String> {
        let mut size = 0u32;
        unsafe { PowerReadFriendlyName(std::ptr::null_mut(), guid, std::ptr::null(), std::ptr::null(), std::ptr::null_mut(), &mut size); }
        if size == 0 { return None; }
        let mut buf = vec![0u16; ((size as usize + 1) / 2).max(1)];
        let rc = unsafe { PowerReadFriendlyName(std::ptr::null_mut(), guid, std::ptr::null(), std::ptr::null(), buf.as_mut_ptr() as *mut u8, &mut size) };
        if rc != 0 { return None; }
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        Some(String::from_utf16_lossy(&buf[..end]))
    }

    /// Read Windows battery telemetry and the active power scheme, preserving unknown values as `None`.
    pub fn battery_status_impl() -> Result<BatteryStatus, String> {
        let mut st = SYSTEM_POWER_STATUS { ACLineStatus: 0, BatteryFlag: 0, BatteryLifePercent: 0, SystemStatusFlag: 0, BatteryLifeTime: 0, BatteryFullLifeTime: 0 };
        if unsafe { GetSystemPowerStatus(&mut st) } == 0 {
            return Err(format!("GetSystemPowerStatus failed: {}", std::io::Error::last_os_error()));
        }
        let (guid, name) = unsafe {
            let mut p: *mut windows_sys::core::GUID = std::ptr::null_mut();
            if PowerGetActiveScheme(std::ptr::null_mut(), &mut p) == 0 && !p.is_null() {
                let g = *p;
                windows_sys::Win32::Foundation::LocalFree(p as _);
                (Some(guid_string(&g)), scheme_name(&g))
            } else { (None, None) }
        };
        Ok(BatteryStatus {
            present: st.BatteryFlag != 128, // BATTERY_FLAG_NO_BATTERY
            on_ac: st.ACLineStatus == 1,
            ac_known: st.ACLineStatus != 255,
            percent: if st.BatteryLifePercent == 255 { None } else { Some(st.BatteryLifePercent) },
            seconds_remaining: if st.BatteryLifeTime == u32::MAX { None } else { Some(st.BatteryLifeTime as i32) },
            active_scheme_guid: guid,
            active_scheme_name: name,
        })
    }

    /// Parse a scheme GUID and activate it with PowrProf, returning Windows errors to the caller.
    pub fn set_active_scheme_impl(guid_str: &str) -> Result<(), String> {
        let guid = parse_guid(guid_str)?;
        let rc = unsafe { windows_sys::Win32::System::Power::PowerSetActiveScheme(std::ptr::null_mut(), &guid) };
        if rc != 0 {
            return Err(format!("PowerSetActiveScheme({guid_str}) failed (Win32 {rc}). Switching schemes may require elevation on managed devices."));
        }
        Ok(())
    }

    /// Parse a hyphen-separated scheme identifier, optionally enclosed in braces, into a Windows GUID.
    fn parse_guid(s: &str) -> Result<windows_sys::core::GUID, String> {
        let parts: Vec<&str> = s.trim_matches(|c| c == '{' || c == '}').split('-').collect();
        if parts.len() != 5 { return Err(format!("invalid scheme GUID: {s}")); }
        let d1 = u32::from_str_radix(parts[0], 16).map_err(|_| "bad GUID".to_string())?;
        let d2 = u16::from_str_radix(parts[1], 16).map_err(|_| "bad GUID".to_string())?;
        let d3 = u16::from_str_radix(parts[2], 16).map_err(|_| "bad GUID".to_string())?;
        let mut d4 = [0u8; 8];
        let p3: Vec<u8> = (0..4).map(|i| u8::from_str_radix(&parts[3][i * 2..i * 2 + 2], 16).unwrap_or(0)).collect();
        // Tail is exactly 6 bytes (12 hex chars); reading beyond that panics.
        if parts[3].len() != 4 || parts[4].len() != 12 { return Err(format!("invalid scheme GUID: {s}")); }
        let p4: Vec<u8> = (0..6).map(|i| u8::from_str_radix(&parts[4][i * 2..i * 2 + 2], 16).unwrap_or(0)).collect();
        d4[0] = p3[0]; d4[1] = p3[1];
        for i in 0..6 { d4[2 + i] = p4[i]; }
        Ok(windows_sys::core::GUID { data1: d1, data2: d2, data3: d3, data4: d4 })
    }
}

/// Return battery telemetry and active power-scheme information from Windows.
#[cfg(windows)]
pub fn battery_status() -> Result<BatteryStatus, String> { win::battery_status_impl() }
/// Report that battery telemetry is unavailable outside Windows.
#[cfg(not(windows))]
pub fn battery_status() -> Result<BatteryStatus, String> {
    Err("Battery telemetry is only available on Windows.".into())
}

/// Activate the requested Windows power scheme, returning parse or PowrProf errors.
#[cfg(windows)]
pub fn set_active_scheme(guid: &str) -> Result<(), String> { win::set_active_scheme_impl(guid) }
/// Report that changing power schemes is unavailable outside Windows.
#[cfg(not(windows))]
pub fn set_active_scheme(_guid: &str) -> Result<(), String> {
    Err("Power schemes are only available on Windows.".into())
}

/// Return the built-in power profiles exposed to the optimizer UI.
pub fn list_profiles() -> Vec<PowerProfile> { builtin_profiles() }

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn discharge_watts_needs_real_drop() {
        let a = PowerSample { at_ms: 0, percent: 80, on_ac: false };
        let b = PowerSample { at_ms: 600_000, percent: 78, on_ac: false };
        // 2% of 50 Wh in 10 min = 6 W
        assert_eq!(discharge_watts(&a, &b, Some(50_000.0)), Some(6.0));
        assert_eq!(discharge_watts(&a, &b, None), None);
        let c = PowerSample { at_ms: 600_000, percent: 79, on_ac: true };
        assert_eq!(discharge_watts(&a, &c, Some(50_000.0)), None);
    }
}
