//! Adaptive battery telemetry sampling.
//!
//! Goals: near-zero idle cost and no polling that is not needed.
//!
//! * The sampling interval is a pure function of the situation
//!   ([`next_interval`]): slow in the background, faster while the Battery
//!   view is open or a measurement is running, slowest while plugged in.
//! * The sampler sleeps on a condition variable, so power events (AC/DC,
//!   percentage, foreground change) wake it immediately instead of being
//!   discovered by polling, and shutdown is instant.
//! * One sample is a single `DeviceIoControl` plus `GetSystemPowerStatus`;
//!   battery facts (design/full capacity) are cached for ten minutes.
//! * No GPU API is ever called, so sampling cannot wake the RTX 4070.

use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use super::battery::{sample_from_reading, summarize, BatteryInfo, BatterySource, BatteryTelemetry, TelemetrySample};
use super::power::SystemPower;

/// How long the cached `BatteryInfo` is trusted.
const INFO_TTL: Duration = Duration::from_secs(600);
/// Samples used for the rolling average shown in the UI.
const SUMMARY_WINDOW: u32 = 60;

#[derive(Clone, Copy, Debug)]
pub struct IntervalContext {
    pub on_ac: bool,
    pub percent: Option<u8>,
    /// The Battery view (or another live consumer) asked for fresh data.
    pub ui_lease_active: bool,
    pub measuring: bool,
}

/// Seconds until the next sample.
pub fn next_interval(c: &IntervalContext) -> Duration {
    let secs = if c.measuring {
        10
    } else if c.ui_lease_active {
        if c.on_ac { 15 } else { 5 }
    } else if c.on_ac {
        300
    } else if c.percent.map(|p| p <= 20).unwrap_or(false) {
        30
    } else {
        60
    };
    Duration::from_secs(secs)
}

/// A wake-up primitive: events set it, the sampler waits on it.
#[derive(Default)]
pub struct Wake {
    flag: Mutex<bool>,
    cv: Condvar,
}

impl Wake {
    pub fn notify(&self) {
        *self.flag.lock().unwrap_or_else(|p| p.into_inner()) = true;
        self.cv.notify_all();
    }

    /// Waits up to `timeout`; returns `true` when woken by an event.
    pub fn wait(&self, timeout: Duration) -> bool {
        let mut flag = self.flag.lock().unwrap_or_else(|p| p.into_inner());
        if !*flag {
            let (g, _) = self.cv.wait_timeout_while(flag, timeout, |f| !*f).unwrap_or_else(|p| p.into_inner());
            flag = g;
        }
        let woken = *flag;
        *flag = false;
        woken
    }
}

pub struct TelemetryService {
    source: Mutex<Box<dyn BatterySource>>,
    info: Mutex<Option<(BatteryInfo, Instant)>>,
    last: Mutex<Option<TelemetrySample>>,
    /// Unix ms until which the UI wants fresh samples (0 = no lease).
    lease_until_ms: AtomicI64,
    measuring: AtomicBool,
    stop: AtomicBool,
    pub wake: Arc<Wake>,
}

impl TelemetryService {
    pub fn new(source: Box<dyn BatterySource>) -> Self {
        Self {
            source: Mutex::new(source),
            info: Mutex::new(None),
            last: Mutex::new(None),
            lease_until_ms: AtomicI64::new(0),
            measuring: AtomicBool::new(false),
            stop: AtomicBool::new(false),
            wake: Arc::new(Wake::default()),
        }
    }

    pub fn system() -> Self {
        Self::new(super::battery::system_source())
    }

    /// The UI keeps this alive (re-calling it every ~20 s) while the Battery
    /// view is visible; when it stops, sampling falls back to the slow rate.
    pub fn lease(&self, now_ms: i64, active: bool) {
        self.lease_until_ms.store(if active { now_ms + 30_000 } else { 0 }, Ordering::Relaxed);
        self.wake.notify();
    }

    pub fn set_measuring(&self, on: bool) {
        self.measuring.store(on, Ordering::Relaxed);
        self.wake.notify();
    }

    pub fn is_measuring(&self) -> bool {
        self.measuring.load(Ordering::Relaxed)
    }

    pub fn lease_active(&self, now_ms: i64) -> bool {
        self.lease_until_ms.load(Ordering::Relaxed) > now_ms
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
        self.wake.notify();
    }

    pub fn should_stop(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    pub fn last_sample(&self) -> Option<TelemetrySample> {
        self.last.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    /// Battery facts, cached for ten minutes.
    pub fn battery_info(&self) -> Option<BatteryInfo> {
        let mut cache = self.info.lock().unwrap_or_else(|p| p.into_inner());
        if let Some((info, at)) = cache.as_ref() {
            if at.elapsed() < INFO_TTL {
                return Some(info.clone());
            }
        }
        let fresh = self.source.lock().unwrap_or_else(|p| p.into_inner()).info().ok();
        if let Some(info) = &fresh {
            *cache = Some((info.clone(), Instant::now()));
        }
        fresh
    }

    /// Takes one sample: one IOCTL (the caller already read the power status).
    pub fn sample_once(&self, power: SystemPower, now_ms: i64, profile_id: Option<String>, workload: Option<String>) -> Result<TelemetrySample, String> {
        let reading = self.source.lock().unwrap_or_else(|p| p.into_inner()).read()?;
        let info = self.battery_info();
        let sample = sample_from_reading(&reading, info.as_ref(), power.percent, if power.ac_known { power.on_ac } else { reading.ac_online }, now_ms, profile_id, workload);
        *self.last.lock().unwrap_or_else(|p| p.into_inner()) = Some(sample.clone());
        Ok(sample)
    }

    /// Live reading plus summary over `recent` samples (read-only; the
    /// caller supplies history from the database).
    pub fn telemetry(&self, recent: &[TelemetrySample]) -> BatteryTelemetry {
        let info = self.battery_info();
        let reading = self.source.lock().unwrap_or_else(|p| p.into_inner()).read().ok();
        summarize(info, reading, &recent[recent.len().saturating_sub(SUMMARY_WINDOW as usize)..])
    }

    pub fn summary_window() -> u32 {
        SUMMARY_WINDOW
    }
}

pub type SharedTelemetry = Arc<TelemetryService>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::optimizer::battery::{BatteryReading, BATTERY_DISCHARGING};

    fn ctx(on_ac: bool, percent: Option<u8>, ui: bool, measuring: bool) -> IntervalContext {
        IntervalContext { on_ac, percent, ui_lease_active: ui, measuring }
    }

    #[test]
    fn interval_adapts_to_the_situation() {
        let secs = |c: IntervalContext| next_interval(&c).as_secs();
        assert_eq!(secs(ctx(true, Some(90), false, false)), 300, "plugged in and unobserved: very slow");
        assert_eq!(secs(ctx(false, Some(80), false, false)), 60, "background on battery");
        assert_eq!(secs(ctx(false, Some(15), false, false)), 30, "low battery: a little more attention");
        assert_eq!(secs(ctx(false, Some(80), true, false)), 5, "Battery view open on battery");
        assert_eq!(secs(ctx(true, Some(80), true, false)), 15, "Battery view open on AC");
        assert_eq!(secs(ctx(false, Some(80), false, true)), 10, "measurement running");
        assert_eq!(secs(ctx(true, None, false, true)), 10);
    }

    /// Monitoring overhead budget: the number of wake-ups per hour in each
    /// steady state is bounded — this is what keeps idle CPU near zero.
    #[test]
    fn samples_per_hour_stay_within_budget() {
        let per_hour = |c: IntervalContext| 3600 / next_interval(&c).as_secs();
        assert!(per_hour(ctx(true, Some(80), false, false)) <= 12);
        assert!(per_hour(ctx(false, Some(80), false, false)) <= 60);
        assert!(per_hour(ctx(false, Some(10), false, false)) <= 120);
        // Even the busiest mode is one cheap IOCTL every five seconds.
        assert!(per_hour(ctx(false, Some(80), true, true)) <= 720);
    }

    #[test]
    fn lease_expires_and_can_be_cancelled() {
        let t = TelemetryService::new(Box::new(Fake::default()));
        assert!(!t.lease_active(1_000));
        t.lease(1_000, true);
        assert!(t.lease_active(1_000 + 29_000));
        assert!(!t.lease_active(1_000 + 31_000), "a lease the UI stops renewing lapses by itself");
        t.lease(2_000, true);
        t.lease(2_001, false);
        assert!(!t.lease_active(2_002));
    }

    #[derive(Default)]
    struct Fake {
        reads: std::sync::atomic::AtomicU32,
    }
    impl BatterySource for Fake {
        fn info(&mut self) -> Result<BatteryInfo, String> {
            Ok(BatteryInfo { design_capacity_mwh: Some(90_000), full_charge_capacity_mwh: Some(81_000), cycle_count: Some(100), chemistry: Some("LION".into()), capacity_relative: false, health_percent: Some(90.0) })
        }
        fn read(&mut self) -> Result<BatteryReading, String> {
            self.reads.fetch_add(1, Ordering::Relaxed);
            Ok(BatteryReading::from_raw(BATTERY_DISCHARGING, 40_000, 15_000, -12_000))
        }
    }
    #[test]
    fn wake_returns_immediately_for_an_event_and_times_out_otherwise() {
        let w = Wake::default();
        let start = Instant::now();
        assert!(!w.wait(Duration::from_millis(30)));
        assert!(start.elapsed() >= Duration::from_millis(25));
        w.notify();
        let start = Instant::now();
        assert!(w.wait(Duration::from_secs(5)), "a pending notification is consumed at once");
        assert!(start.elapsed() < Duration::from_secs(1));
        assert!(!w.wait(Duration::from_millis(10)), "and only once");
    }

    #[test]
    fn a_wake_from_another_thread_interrupts_a_long_wait() {
        let w = Arc::new(Wake::default());
        let w2 = w.clone();
        let t = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(40));
            w2.notify();
        });
        let start = Instant::now();
        assert!(w.wait(Duration::from_secs(30)));
        assert!(start.elapsed() < Duration::from_secs(5));
        t.join().unwrap();
    }

    #[test]
    fn battery_info_is_cached_so_sampling_stays_cheap() {
        struct Counting(Arc<std::sync::atomic::AtomicU32>);
        impl BatterySource for Counting {
            fn info(&mut self) -> Result<BatteryInfo, String> {
                self.0.fetch_add(1, Ordering::Relaxed);
                Ok(BatteryInfo { design_capacity_mwh: Some(1), full_charge_capacity_mwh: Some(1), cycle_count: None, chemistry: None, capacity_relative: false, health_percent: None })
            }
            fn read(&mut self) -> Result<BatteryReading, String> {
                Err("n/a".into())
            }
        }
        let calls = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let t = TelemetryService::new(Box::new(Counting(calls.clone())));
        for _ in 0..50 {
            t.battery_info();
        }
        assert_eq!(calls.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn stop_is_sticky_and_wakes_the_sampler() {
        let t = TelemetryService::new(Box::new(Fake::default()));
        assert!(!t.should_stop());
        t.stop();
        assert!(t.should_stop());
        assert!(t.wake.wait(Duration::from_millis(10)));
    }
}
