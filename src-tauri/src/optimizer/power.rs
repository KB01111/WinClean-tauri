//! AC/DC state, battery percentage and power-scheme control
//! (`GetSystemPowerStatus` + PowrProf). Real wattage and battery health live
//! in `battery.rs` (IOCTL based); nothing here estimates power.

use serde::{Deserialize, Serialize};

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

/// Lowest-resolution history sample kept for compatibility with 1.x databases.
/// New telemetry is stored in `power_telemetry` (see `battery.rs`).

#[cfg(windows)]
mod win {
    use super::*;
    use windows_sys::Win32::System::Power::{
        GetSystemPowerStatus, PowerGetActiveScheme, PowerReadFriendlyName, SYSTEM_POWER_STATUS,
    };

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
        let (d1, d2, d3, d4) = super::parse_guid_parts(s)?;
        Ok(windows_sys::core::GUID { data1: d1, data2: d2, data3: d3, data4: d4 })
    }
}

/// Strictly parses `8-4-4-4-12` hex GUID text (optionally braced) into
/// `(data1, data2, data3, data4)`. Validates every group's shape before
/// slicing so malformed input returns an error instead of panicking.
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
fn parse_guid_parts(s: &str) -> Result<(u32, u16, u16, [u8; 8]), String> {
    let bad = || format!("invalid scheme GUID: {s}");
    let parts: Vec<&str> = s.trim().trim_matches(|c| c == '{' || c == '}').split('-').collect();
    let widths = [8usize, 4, 4, 4, 12];
    if parts.len() != 5
        || parts.iter().zip(widths).any(|(p, w)| p.len() != w || !p.bytes().all(|b| b.is_ascii_hexdigit()))
    {
        return Err(bad());
    }
    let d1 = u32::from_str_radix(parts[0], 16).map_err(|_| bad())?;
    let d2 = u16::from_str_radix(parts[1], 16).map_err(|_| bad())?;
    let d3 = u16::from_str_radix(parts[2], 16).map_err(|_| bad())?;
    let tail = format!("{}{}", parts[3], parts[4]);
    let mut d4 = [0u8; 8];
    for (i, byte) in d4.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&tail[i * 2..i * 2 + 2], 16).map_err(|_| bad())?;
    }
    Ok((d1, d2, d3, d4))
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

/// The cheap part of the power state (no scheme lookup): what the sampler and
/// the auto-switch controller need on every tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemPower {
    pub on_ac: bool,
    /// False when Windows reports ACLineStatus=255 (unknown).
    pub ac_known: bool,
    pub percent: Option<u8>,
    pub charging: bool,
}

#[cfg(windows)]
pub fn system_power() -> Result<SystemPower, String> {
    use windows_sys::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
    let mut st = SYSTEM_POWER_STATUS { ACLineStatus: 0, BatteryFlag: 0, BatteryLifePercent: 0, SystemStatusFlag: 0, BatteryLifeTime: 0, BatteryFullLifeTime: 0 };
    if unsafe { GetSystemPowerStatus(&mut st) } == 0 {
        return Err(format!("GetSystemPowerStatus failed: {}", std::io::Error::last_os_error()));
    }
    Ok(SystemPower {
        on_ac: st.ACLineStatus == 1,
        ac_known: st.ACLineStatus != 255,
        percent: if st.BatteryLifePercent == 255 { None } else { Some(st.BatteryLifePercent) },
        charging: st.BatteryFlag != 255 && st.BatteryFlag & 8 != 0,
    })
}
#[cfg(not(windows))]
pub fn system_power() -> Result<SystemPower, String> {
    Err("Power status is only available on Windows.".into())
}

/// True for a well-formed `8-4-4-4-12` hex GUID (optionally braced).
pub fn is_valid_guid(s: &str) -> bool { parse_guid_parts(s).is_ok() }

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn guid_parsing_is_strict_and_never_panics() {
        let (d1, d2, d3, d4) = parse_guid_parts("{381b4222-f694-41f0-9685-ff5bb260df2e}").unwrap();
        assert_eq!((d1, d2, d3), (0x381b4222, 0xf694, 0x41f0));
        assert_eq!(d4, [0x96, 0x85, 0xff, 0x5b, 0xb2, 0x60, 0xdf, 0x2e]);
        // Short, long, non-hex and non-ASCII groups are rejected, not sliced.
        for bad in ["", "a-b-c-d-e", "381b4222-f694-41f0-96-ff5bb260df2e", "381b4222-f694-41f0-9685-ff5bb260df2e0",
                    "381b4222-f694-41f0-9685-ff5bb260df2g", "381b4222-f694-41f0-96é5-ff5bb260df2e"] {
            assert!(parse_guid_parts(bad).is_err(), "{bad} should be rejected");
            assert!(!is_valid_guid(bad));
        }
        assert!(is_valid_guid("{381b4222-f694-41f0-9685-ff5bb260df2e}"));
        assert!(is_valid_guid(crate::optimizer::policy::SCHEME_POWER_SAVER));
    }
}
