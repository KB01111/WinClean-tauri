//! Native battery telemetry.
//!
//! Watts come from the battery's own report: `IOCTL_BATTERY_QUERY_STATUS`
//! returns `BATTERY_STATUS.Rate` in milliwatts (negative while discharging),
//! unless the battery reports *relative* units, in which case no watts are
//! produced at all. State of health comes from `BATTERY_INFORMATION`
//! (`FullChargedCapacity / DesignedCapacity`). Nothing here is estimated from
//! percentage steps and nothing is ever invented: unknown stays `None`.
//!
//! The pure math in this file is cross-platform and unit-tested; the Windows
//! reader is a thin wrapper around the documented IOCTLs.

use serde::{Deserialize, Serialize};

/// One persisted telemetry sample.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TelemetrySample {
    pub at_ms: i64,
    pub percent: Option<u8>,
    pub on_ac: bool,
    /// Signed milliwatts: negative = discharging, positive = charging.
    /// `None` when the battery reports no rate or reports relative units.
    pub rate_mw: Option<i32>,
    pub voltage_mv: Option<u32>,
    pub capacity_mwh: Option<u32>,
    /// Profile WinClean had applied when the sample was taken.
    pub profile_id: Option<String>,
    pub workload: Option<String>,
}

/// Slowly-changing battery facts (`BATTERY_INFORMATION`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatteryInfo {
    pub design_capacity_mwh: Option<u32>,
    pub full_charge_capacity_mwh: Option<u32>,
    pub cycle_count: Option<u32>,
    pub chemistry: Option<String>,
    /// True when capacity/rate are in arbitrary units (no watts possible).
    pub capacity_relative: bool,
    pub health_percent: Option<f32>,
}

/// One instantaneous reading (`BATTERY_STATUS`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatteryReading {
    pub ac_online: bool,
    pub charging: bool,
    pub discharging: bool,
    pub critical: bool,
    pub capacity_mwh: Option<u32>,
    pub voltage_mv: Option<u32>,
    /// Raw signed rate, `None` for `BATTERY_UNKNOWN_RATE`.
    pub rate_mw: Option<i32>,
}

pub const BATTERY_CHARGING: u32 = 0x4;
pub const BATTERY_CRITICAL: u32 = 0x8;
pub const BATTERY_DISCHARGING: u32 = 0x2;
pub const BATTERY_POWER_ON_LINE: u32 = 0x1;
pub const BATTERY_UNKNOWN_CAPACITY: u32 = 0xFFFF_FFFF;
pub const BATTERY_UNKNOWN_VOLTAGE: u32 = 0xFFFF_FFFF;
pub const BATTERY_UNKNOWN_RATE: i32 = i32::MIN; // 0x80000000

impl BatteryReading {
    /// Builds a reading from the raw `BATTERY_STATUS` fields, mapping every
    /// "unknown" sentinel to `None`.
    pub fn from_raw(power_state: u32, capacity: u32, voltage: u32, rate: i32) -> Self {
        Self {
            ac_online: power_state & BATTERY_POWER_ON_LINE != 0,
            charging: power_state & BATTERY_CHARGING != 0,
            discharging: power_state & BATTERY_DISCHARGING != 0,
            critical: power_state & BATTERY_CRITICAL != 0,
            capacity_mwh: (capacity != BATTERY_UNKNOWN_CAPACITY).then_some(capacity),
            voltage_mv: (voltage != BATTERY_UNKNOWN_VOLTAGE).then_some(voltage),
            rate_mw: (rate != BATTERY_UNKNOWN_RATE).then_some(rate),
        }
    }
}

/// `FullChargedCapacity / DesignedCapacity` in percent, one decimal.
/// Implausible values (zero, unknown, or >120% of design) are `None`.
pub fn health_percent(design_mwh: Option<u32>, full_mwh: Option<u32>) -> Option<f32> {
    let design = design_mwh.filter(|v| *v > 0 && *v != u32::MAX)? as f64;
    let full = full_mwh.filter(|v| *v > 0 && *v != u32::MAX)? as f64;
    let pct = full / design * 100.0;
    if pct.is_finite() && pct <= 120.0 {
        Some(((pct * 10.0).round() / 10.0) as f32)
    } else {
        None
    }
}

/// Converts a reading into a stored sample. A rate is only kept when the
/// battery reports real units and the sign agrees with the power state
/// (a "discharging" battery with a positive rate is reported by a few
/// firmwares as magnitude-only; it is normalized to negative here).
pub fn sample_from_reading(
    reading: &BatteryReading,
    info: Option<&BatteryInfo>,
    percent: Option<u8>,
    on_ac: bool,
    at_ms: i64,
    profile_id: Option<String>,
    workload: Option<String>,
) -> TelemetrySample {
    let relative = info.map(|i| i.capacity_relative).unwrap_or(false);
    let rate_mw = if relative {
        None
    } else {
        reading.rate_mw.map(|r| {
            if reading.discharging && !reading.charging && r > 0 {
                -r
            } else {
                r
            }
        })
    };
    TelemetrySample {
        at_ms,
        percent,
        on_ac,
        rate_mw,
        voltage_mv: reading.voltage_mv,
        capacity_mwh: if relative { None } else { reading.capacity_mwh },
        profile_id,
        workload,
    }
}

/// Instantaneous discharge power in watts, `None` unless discharging with a
/// real rate.
pub fn discharge_watts_from_rate(rate_mw: Option<i32>) -> Option<f64> {
    match rate_mw {
        Some(r) if r < 0 => Some((-(r as i64)) as f64 / 1000.0),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WattStats {
    pub mean_w: f64,
    pub stdev_w: f64,
    pub min_w: f64,
    pub max_w: f64,
    pub samples: usize,
}

/// Statistics over *discharge-only* samples. The mean is a 10%-trimmed mean
/// so a single resume spike or idle dip cannot dominate a window.
pub fn discharge_stats(samples: &[TelemetrySample]) -> Option<WattStats> {
    let mut watts: Vec<f64> = samples
        .iter()
        .filter(|s| !s.on_ac)
        .filter_map(|s| discharge_watts_from_rate(s.rate_mw))
        .collect();
    if watts.is_empty() {
        return None;
    }
    watts.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = watts.len();
    let trim = if n >= 10 { n / 10 } else { 0 };
    let kept = &watts[trim..n - trim];
    let mean = kept.iter().sum::<f64>() / kept.len() as f64;
    let var = if kept.len() > 1 {
        kept.iter().map(|w| (w - mean).powi(2)).sum::<f64>() / (kept.len() - 1) as f64
    } else {
        0.0
    };
    Some(WattStats {
        mean_w: round1(mean),
        stdev_w: round1(var.sqrt()),
        min_w: round1(*watts.first().unwrap()),
        max_w: round1(*watts.last().unwrap()),
        samples: kept.len(),
    })
}

/// Independent cross-check of the reported rate: average power implied by the
/// battery's own remaining-capacity counter over consecutive discharge
/// samples. Returns `None` when capacity is not reported or did not fall.
pub fn energy_delta_watts(samples: &[TelemetrySample]) -> Option<f64> {
    let mut used_mwh = 0.0f64;
    let mut hours = 0.0f64;
    for pair in samples.windows(2) {
        let (a, b) = (&pair[0], &pair[1]);
        if a.on_ac || b.on_ac || b.at_ms <= a.at_ms {
            continue;
        }
        // Skip long gaps (sleep/resume): the counter moved while we did not look.
        if b.at_ms - a.at_ms > 15 * 60_000 {
            continue;
        }
        if let (Some(ca), Some(cb)) = (a.capacity_mwh, b.capacity_mwh) {
            if cb < ca {
                used_mwh += (ca - cb) as f64;
                hours += (b.at_ms - a.at_ms) as f64 / 3_600_000.0;
            } else {
                hours += (b.at_ms - a.at_ms) as f64 / 3_600_000.0;
            }
        }
    }
    if hours <= 0.0 || used_mwh <= 0.0 {
        return None;
    }
    Some(round1(used_mwh / hours / 1000.0))
}

/// Remaining runtime in minutes from remaining energy and average draw.
pub fn estimated_runtime_minutes(remaining_mwh: Option<u32>, avg_discharge_w: f64) -> Option<f64> {
    let remaining = remaining_mwh.filter(|v| *v != u32::MAX)? as f64;
    if avg_discharge_w <= 0.0 || !avg_discharge_w.is_finite() {
        return None;
    }
    Some(round1(remaining / 1000.0 / avg_discharge_w * 60.0))
}

fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

/// Everything the Battery view needs in one read-only object.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatteryTelemetry {
    pub present: bool,
    pub info: Option<BatteryInfo>,
    pub reading: Option<BatteryReading>,
    pub discharge_watts: Option<f64>,
    pub charge_watts: Option<f64>,
    /// Average over the recent window (trimmed mean), when enough samples exist.
    pub average_discharge_watts: Option<f64>,
    pub energy_cross_check_watts: Option<f64>,
    pub estimated_runtime_minutes: Option<f64>,
    /// Why a figure is missing, in plain words (never a guess).
    pub notes: Vec<String>,
}

/// Assembles the telemetry summary from a live reading plus recent history.
pub fn summarize(
    info: Option<BatteryInfo>,
    reading: Option<BatteryReading>,
    recent: &[TelemetrySample],
) -> BatteryTelemetry {
    let mut notes = Vec::new();
    let relative = info.as_ref().map(|i| i.capacity_relative).unwrap_or(false);
    if relative {
        notes.push("This battery reports relative units, so watts and runtime cannot be computed.".to_string());
    }
    let rate = if relative { None } else { reading.and_then(|r| r.rate_mw) };
    let discharge = discharge_watts_from_rate(rate);
    let charge = rate.filter(|r| *r > 0).map(|r| r as f64 / 1000.0);
    if !relative && reading.is_some() && rate.is_none() {
        notes.push("The battery did not report a power rate in this reading.".to_string());
    }
    let stats = discharge_stats(recent);
    let avg = stats.map(|s| s.mean_w);
    if avg.is_none() && !recent.is_empty() {
        notes.push("Not enough discharge samples yet for an average (needs the laptop on battery).".to_string());
    }
    let runtime = match (reading.and_then(|r| r.capacity_mwh), avg.or(discharge)) {
        (Some(_), Some(w)) if !relative => estimated_runtime_minutes(reading.and_then(|r| r.capacity_mwh), w),
        _ => None,
    };
    BatteryTelemetry {
        present: reading.is_some() || info.is_some(),
        info,
        reading,
        discharge_watts: discharge,
        charge_watts: charge,
        average_discharge_watts: avg,
        energy_cross_check_watts: energy_delta_watts(recent),
        estimated_runtime_minutes: runtime,
        notes,
    }
}

/// Source of battery data, abstracted so sampling logic is testable.
pub trait BatterySource: Send {
    fn info(&mut self) -> Result<BatteryInfo, String>;
    fn read(&mut self) -> Result<BatteryReading, String>;
}

/// The platform battery reader (IOCTL-based on Windows).
pub fn system_source() -> Box<dyn BatterySource> {
    #[cfg(windows)]
    {
        Box::new(win::IoctlBattery::default())
    }
    #[cfg(not(windows))]
    {
        Box::new(UnsupportedBattery)
    }
}

#[cfg(not(windows))]
struct UnsupportedBattery;

#[cfg(not(windows))]
impl BatterySource for UnsupportedBattery {
    fn info(&mut self) -> Result<BatteryInfo, String> {
        Err("Battery telemetry is only available on Windows.".into())
    }
    fn read(&mut self) -> Result<BatteryReading, String> {
        Err("Battery telemetry is only available on Windows.".into())
    }
}

#[cfg(windows)]
mod win {
    use super::*;
    use std::ffi::c_void;
    use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
        SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInterfaces, SetupDiGetClassDevsW,
        SetupDiGetDeviceInterfaceDetailW, DIGCF_DEVICEINTERFACE, DIGCF_PRESENT,
        SP_DEVICE_INTERFACE_DATA, SP_DEVICE_INTERFACE_DETAIL_DATA_W,
    };
    use windows_sys::Win32::Foundation::{
        CloseHandle, GetLastError, ERROR_FILE_NOT_FOUND, ERROR_NO_SUCH_DEVICE, GENERIC_READ,
        GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows_sys::Win32::System::IO::DeviceIoControl;
    use windows_sys::Win32::System::Power::{
        BatteryInformation, BATTERY_CAPACITY_RELATIVE, BATTERY_INFORMATION,
        BATTERY_QUERY_INFORMATION, BATTERY_STATUS, BATTERY_SYSTEM_BATTERY, BATTERY_WAIT_STATUS,
        GUID_DEVICE_BATTERY, IOCTL_BATTERY_QUERY_INFORMATION, IOCTL_BATTERY_QUERY_STATUS,
        IOCTL_BATTERY_QUERY_TAG,
    };

    /// Owns the battery device handle and the current battery tag.
    #[derive(Default)]
    pub struct IoctlBattery {
        device: Option<Device>,
        info: Option<BatteryInfo>,
    }

    struct Device {
        handle: HANDLE,
        tag: u32,
    }

    impl Drop for Device {
        fn drop(&mut self) {
            unsafe { CloseHandle(self.handle) };
        }
    }

    impl Drop for IoctlBattery {
        fn drop(&mut self) {
            self.device = None;
        }
    }

    // SAFETY: the raw handle is only ever used from the thread that owns the
    // `IoctlBattery` (the sampler thread); the type is moved there once.
    unsafe impl Send for IoctlBattery {}

    impl IoctlBattery {
        /// Opens the first *system* battery device and queries its tag.
        fn open() -> Result<Device, String> {
            unsafe {
                let set = SetupDiGetClassDevsW(
                    &GUID_DEVICE_BATTERY,
                    std::ptr::null(),
                    std::ptr::null_mut(),
                    DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
                );
                if set == -1 || set == 0 {
                    return Err(format!("SetupDiGetClassDevs(battery) failed: {}", std::io::Error::last_os_error()));
                }
                let result = Self::open_from_set(set);
                SetupDiDestroyDeviceInfoList(set);
                result
            }
        }

        unsafe fn open_from_set(set: isize) -> Result<Device, String> {
            let mut index = 0u32;
            loop {
                let mut data: SP_DEVICE_INTERFACE_DATA = std::mem::zeroed();
                data.cbSize = std::mem::size_of::<SP_DEVICE_INTERFACE_DATA>() as u32;
                if SetupDiEnumDeviceInterfaces(set, std::ptr::null(), &GUID_DEVICE_BATTERY, index, &mut data) == 0 {
                    return Err("No battery device found (desktop PC, or the battery driver is not loaded).".into());
                }
                index += 1;
                let mut required = 0u32;
                SetupDiGetDeviceInterfaceDetailW(set, &data, std::ptr::null_mut(), 0, &mut required, std::ptr::null_mut());
                if required == 0 {
                    continue;
                }
                // u32 backing store keeps the detail struct 4-byte aligned.
                let mut buf = vec![0u32; (required as usize).div_ceil(4)];
                let detail = buf.as_mut_ptr() as *mut SP_DEVICE_INTERFACE_DETAIL_DATA_W;
                (*detail).cbSize = if cfg!(target_pointer_width = "64") { 8 } else { 6 };
                if SetupDiGetDeviceInterfaceDetailW(set, &data, detail, required, std::ptr::null_mut(), std::ptr::null_mut()) == 0 {
                    continue;
                }
                let path = (*detail).DevicePath.as_ptr();
                let handle = CreateFileW(
                    path,
                    GENERIC_READ | GENERIC_WRITE,
                    FILE_SHARE_READ | FILE_SHARE_WRITE,
                    std::ptr::null(),
                    OPEN_EXISTING,
                    FILE_ATTRIBUTE_NORMAL,
                    std::ptr::null_mut(),
                );
                if handle == INVALID_HANDLE_VALUE || handle.is_null() {
                    continue;
                }
                let device = Device { handle, tag: 0 };
                let mut wait: u32 = 0;
                let mut tag: u32 = 0;
                let mut returned = 0u32;
                let ok = DeviceIoControl(
                    handle,
                    IOCTL_BATTERY_QUERY_TAG,
                    &mut wait as *mut u32 as *const c_void,
                    4,
                    &mut tag as *mut u32 as *mut c_void,
                    4,
                    &mut returned,
                    std::ptr::null_mut(),
                );
                if ok == 0 || tag == 0 {
                    continue;
                }
                let device = Device { tag, ..device };
                // Only the battery that powers the system counts (not a UPS/HID battery).
                match Self::query_info(&device) {
                    Ok(info) if info.0 & BATTERY_SYSTEM_BATTERY != 0 => return Ok(device),
                    _ => continue,
                }
            }
        }

        /// Returns `(capabilities, info)`.
        fn query_info(device: &Device) -> Result<(u32, BatteryInfo), (u32, String)> {
            let query = BATTERY_QUERY_INFORMATION { BatteryTag: device.tag, InformationLevel: BatteryInformation, AtRate: 0 };
            let mut out: BATTERY_INFORMATION = unsafe { std::mem::zeroed() };
            let mut returned = 0u32;
            let ok = unsafe {
                DeviceIoControl(
                    device.handle,
                    IOCTL_BATTERY_QUERY_INFORMATION,
                    &query as *const _ as *const c_void,
                    std::mem::size_of::<BATTERY_QUERY_INFORMATION>() as u32,
                    &mut out as *mut _ as *mut c_void,
                    std::mem::size_of::<BATTERY_INFORMATION>() as u32,
                    &mut returned,
                    std::ptr::null_mut(),
                )
            };
            if ok == 0 {
                let code = unsafe { GetLastError() };
                return Err((code, format!("IOCTL_BATTERY_QUERY_INFORMATION failed ({code})")));
            }
            let relative = out.Capabilities & BATTERY_CAPACITY_RELATIVE != 0;
            let chem = String::from_utf8_lossy(&out.Chemistry).trim_end_matches('\0').trim().to_string();
            let design = (out.DesignedCapacity != u32::MAX && out.DesignedCapacity != 0).then_some(out.DesignedCapacity);
            let full = (out.FullChargedCapacity != u32::MAX && out.FullChargedCapacity != 0).then_some(out.FullChargedCapacity);
            Ok((
                out.Capabilities,
                BatteryInfo {
                    design_capacity_mwh: design,
                    full_charge_capacity_mwh: full,
                    cycle_count: (out.CycleCount != 0 && out.CycleCount != u32::MAX).then_some(out.CycleCount),
                    chemistry: (!chem.is_empty()).then_some(chem),
                    capacity_relative: relative,
                    health_percent: if relative { None } else { health_percent(design, full) },
                },
            ))
        }

        fn ensure_device(&mut self) -> Result<&Device, String> {
            if self.device.is_none() {
                self.device = Some(Self::open()?);
            }
            Ok(self.device.as_ref().unwrap())
        }

        fn is_stale_tag(code: u32) -> bool {
            code == ERROR_NO_SUCH_DEVICE || code == ERROR_FILE_NOT_FOUND
        }

        fn query_status(device: &Device) -> Result<BatteryReading, u32> {
            let wait = BATTERY_WAIT_STATUS { BatteryTag: device.tag, Timeout: 0, PowerState: 0, LowCapacity: 0, HighCapacity: 0 };
            let mut status = BATTERY_STATUS::default();
            let mut returned = 0u32;
            let ok = unsafe {
                DeviceIoControl(
                    device.handle,
                    IOCTL_BATTERY_QUERY_STATUS,
                    &wait as *const _ as *const c_void,
                    std::mem::size_of::<BATTERY_WAIT_STATUS>() as u32,
                    &mut status as *mut _ as *mut c_void,
                    std::mem::size_of::<BATTERY_STATUS>() as u32,
                    &mut returned,
                    std::ptr::null_mut(),
                )
            };
            if ok == 0 {
                return Err(unsafe { GetLastError() });
            }
            Ok(BatteryReading::from_raw(status.PowerState, status.Capacity, status.Voltage, status.Rate))
        }
    }

    impl BatterySource for IoctlBattery {
        fn info(&mut self) -> Result<BatteryInfo, String> {
            if let Some(info) = &self.info {
                return Ok(info.clone());
            }
            self.ensure_device()?;
            let result = Self::query_info(self.device.as_ref().unwrap());
            match result {
                Ok((_, info)) => {
                    self.info = Some(info.clone());
                    Ok(info)
                }
                Err((code, msg)) => {
                    if Self::is_stale_tag(code) {
                        self.device = None;
                    }
                    Err(msg)
                }
            }
        }

        fn read(&mut self) -> Result<BatteryReading, String> {
            // One retry: a stale tag means the battery was swapped/re-enumerated.
            for attempt in 0..2 {
                let device = self.ensure_device()?;
                match Self::query_status(device) {
                    Ok(reading) => return Ok(reading),
                    Err(code) if attempt == 0 && Self::is_stale_tag(code) => {
                        self.device = None;
                        self.info = None;
                    }
                    Err(code) => return Err(format!("IOCTL_BATTERY_QUERY_STATUS failed ({code})")),
                }
            }
            Err("battery device kept changing; try again".into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(at_ms: i64, on_ac: bool, rate_mw: Option<i32>, capacity: Option<u32>) -> TelemetrySample {
        TelemetrySample { at_ms, percent: Some(50), on_ac, rate_mw, voltage_mv: Some(15_000), capacity_mwh: capacity, profile_id: None, workload: None }
    }

    #[test]
    fn unknown_sentinels_become_none_never_zero() {
        let r = BatteryReading::from_raw(BATTERY_DISCHARGING, BATTERY_UNKNOWN_CAPACITY, BATTERY_UNKNOWN_VOLTAGE, BATTERY_UNKNOWN_RATE);
        assert_eq!((r.capacity_mwh, r.voltage_mv, r.rate_mw), (None, None, None));
        assert!(r.discharging && !r.charging && !r.ac_online);
    }

    #[test]
    fn rate_sign_gives_discharge_vs_charge_watts() {
        assert_eq!(discharge_watts_from_rate(Some(-12_500)), Some(12.5));
        assert_eq!(discharge_watts_from_rate(Some(30_000)), None);
        assert_eq!(discharge_watts_from_rate(Some(0)), None);
        assert_eq!(discharge_watts_from_rate(None), None);
    }

    #[test]
    fn relative_batteries_never_produce_watts() {
        let info = BatteryInfo { design_capacity_mwh: None, full_charge_capacity_mwh: None, cycle_count: None, chemistry: None, capacity_relative: true, health_percent: None };
        let reading = BatteryReading::from_raw(BATTERY_DISCHARGING, 50, 12_000, -200);
        let sample = sample_from_reading(&reading, Some(&info), Some(50), false, 1, None, None);
        assert_eq!(sample.rate_mw, None);
        assert_eq!(sample.capacity_mwh, None);
        let t = summarize(Some(info), Some(reading), &[sample]);
        assert_eq!(t.discharge_watts, None);
        assert!(t.notes.iter().any(|n| n.contains("relative")));
    }

    #[test]
    fn magnitude_only_firmware_is_normalized_to_negative_while_discharging() {
        let r = BatteryReading::from_raw(BATTERY_DISCHARGING, 40_000, 15_000, 9_000);
        let sample = sample_from_reading(&r, None, Some(60), false, 1, None, None);
        assert_eq!(sample.rate_mw, Some(-9_000));
        // A charging battery keeps its positive rate.
        let c = BatteryReading::from_raw(BATTERY_CHARGING | BATTERY_POWER_ON_LINE, 40_000, 15_000, 20_000);
        assert_eq!(sample_from_reading(&c, None, Some(60), true, 1, None, None).rate_mw, Some(20_000));
    }

    #[test]
    fn health_is_ratio_of_full_to_design_and_rejects_nonsense() {
        assert_eq!(health_percent(Some(90_000), Some(81_000)), Some(90.0));
        assert_eq!(health_percent(Some(90_000), Some(0)), None);
        assert_eq!(health_percent(None, Some(81_000)), None);
        assert_eq!(health_percent(Some(50_000), Some(90_000)), None); // 180%: firmware lie
        assert_eq!(health_percent(Some(u32::MAX), Some(1)), None);
    }

    #[test]
    fn stats_use_discharge_samples_only_and_trim_outliers() {
        let mut v: Vec<TelemetrySample> = (0..20).map(|i| s(i * 1000, false, Some(-10_000), None)).collect();
        v.push(s(21_000, true, Some(45_000), None)); // AC charge sample: ignored
        v.push(s(22_000, false, Some(-90_000), None)); // spike: trimmed
        let stats = discharge_stats(&v).unwrap();
        assert_eq!(stats.mean_w, 10.0);
        assert_eq!(stats.max_w, 90.0);
        assert!(discharge_stats(&[s(0, true, Some(-5_000), None)]).is_none());
    }

    #[test]
    fn energy_cross_check_matches_reported_rate() {
        // 10 W for 6 minutes = 1000 mWh.
        let v = vec![
            s(0, false, Some(-10_000), Some(50_000)),
            s(180_000, false, Some(-10_000), Some(49_500)),
            s(360_000, false, Some(-10_000), Some(49_000)),
        ];
        assert_eq!(energy_delta_watts(&v), Some(10.0));
        // Capacity that did not move yields no figure rather than 0 W.
        let flat = vec![s(0, false, Some(-10_000), Some(50_000)), s(60_000, false, Some(-10_000), Some(50_000))];
        assert_eq!(energy_delta_watts(&flat), None);
        // A multi-minute gap (sleep) is not attributed to the draw.
        let gap = vec![s(0, false, None, Some(50_000)), s(3_600_000, false, None, Some(40_000))];
        assert_eq!(energy_delta_watts(&gap), None);
    }

    #[test]
    fn runtime_uses_remaining_energy_over_average_draw() {
        // 45 Wh at 15 W = 3 h.
        assert_eq!(estimated_runtime_minutes(Some(45_000), 15.0), Some(180.0));
        assert_eq!(estimated_runtime_minutes(Some(45_000), 0.0), None);
        assert_eq!(estimated_runtime_minutes(None, 15.0), None);
    }

    #[test]
    fn summary_reports_why_figures_are_missing() {
        let r = BatteryReading::from_raw(BATTERY_POWER_ON_LINE, 50_000, 15_000, BATTERY_UNKNOWN_RATE);
        let t = summarize(None, Some(r), &[]);
        assert_eq!(t.estimated_runtime_minutes, None);
        assert!(t.notes.iter().any(|n| n.contains("did not report a power rate")));
    }
}
