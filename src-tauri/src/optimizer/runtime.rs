//! The optimizer's background runtime: one thread that sleeps on a condition
//! variable and wakes for (a) power-setting events, (b) foreground changes and
//! (c) a slow adaptive timer. Each wake it records telemetry (only when
//! something changed or the timer fired) and evaluates the automatic
//! profile rules.
//!
//! `Runtime::tick` is the whole decision loop with every OS dependency behind
//! [`Environment`], so the switching behavior is tested end to end with fakes.

use std::time::Duration;

use serde_json::{json, Value};

use crate::db::Database;

use super::auto_switch::{decide, AutoDecision, AutoSwitchInput};
use super::backend::Backends;
use super::ecoqos::now_ms;
use super::orchestrator::{apply_profile, Ctx, Origin};
use super::power::SystemPower;
use super::profiles;
use super::telemetry::{next_interval, IntervalContext, SharedTelemetry};
use super::workload::{WorkloadKind, WorkloadSnapshot};

/// Coalesces bursts of events (alt-tab storms, percentage ticks).
const DEBOUNCE: Duration = Duration::from_millis(1500);
/// Even with events the runtime re-checks this often while auto-switch is on.
const AUTO_SAFETY_NET: Duration = Duration::from_secs(120);
/// Workload sampling is rate-limited: it walks the process list.
const WORKLOAD_MIN_AGE_MS: i64 = 4_000;

/// Everything the runtime reads from the machine.
pub trait Environment: Send {
    fn power(&mut self) -> Result<SystemPower, String>;
    fn workload(&mut self) -> WorkloadSnapshot;
    fn ghelper_running(&mut self) -> bool;
    fn now_ms(&mut self) -> i64;
}

pub struct SystemEnvironment {
    tracker: super::workload::CpuTracker,
}

impl SystemEnvironment {
    pub fn new() -> Self {
        Self { tracker: super::workload::CpuTracker::default() }
    }
}

impl Default for SystemEnvironment {
    fn default() -> Self {
        Self::new()
    }
}

impl Environment for SystemEnvironment {
    fn power(&mut self) -> Result<SystemPower, String> {
        super::power::system_power()
    }
    fn workload(&mut self) -> WorkloadSnapshot {
        super::workload::sample(&mut self.tracker)
    }
    fn ghelper_running(&mut self) -> bool {
        super::processes::light_snapshot().iter().any(|p| p.name.eq_ignore_ascii_case("ghelper.exe"))
    }
    fn now_ms(&mut self) -> i64 {
        now_ms()
    }
}

pub type Emitter = Box<dyn Fn(&str, Value) + Send>;

pub struct Runtime {
    db: Database,
    backends: Backends,
    telemetry: SharedTelemetry,
    env: Box<dyn Environment>,
    emit: Emitter,
    last_recorded: Option<(bool, Option<u8>)>,
    workload: Option<(WorkloadSnapshot, i64)>,
}

impl Runtime {
    pub fn new(db: Database, backends: Backends, telemetry: SharedTelemetry, env: Box<dyn Environment>, emit: Emitter) -> Self {
        Self { db, backends, telemetry, env, emit, last_recorded: None, workload: None }
    }

    fn current_workload(&mut self, now: i64, needed: bool, force: bool) -> Option<WorkloadSnapshot> {
        if !needed {
            return None;
        }
        let stale = self.workload.as_ref().map(|(_, at)| now - at >= WORKLOAD_MIN_AGE_MS).unwrap_or(true);
        if stale || force {
            let fresh = self.env.workload();
            let changed = self.workload.as_ref().map(|(w, _)| w.kind != fresh.kind).unwrap_or(true);
            if changed {
                (self.emit)("workload", json!({ "kind": fresh.kind, "evidence": fresh.evidence, "foreground": fresh.foreground }));
            }
            self.workload = Some((fresh, now));
        }
        self.workload.as_ref().map(|(w, _)| w.clone())
    }

    /// One iteration. `woken` is true when an event (not the timer) caused it.
    /// Returns how long to sleep before the next iteration.
    pub fn tick(&mut self, woken: bool) -> Duration {
        let now = self.env.now_ms();
        let power = self.env.power().ok();
        let auto = profiles::get_auto_switch(&self.db);
        let active = profiles::get_active(&self.db);
        let ui = self.telemetry.lease_active(now);
        let measuring = self.telemetry.is_measuring();
        let workload = self.current_workload(now, measuring || ui || auto.enabled, woken);

        // Telemetry: on the timer always; on events only when AC/DC or the percentage moved.
        if let Some(p) = power {
            let state = (p.on_ac, p.percent);
            let moved = self.last_recorded != Some(state);
            if !woken || moved {
                let kind = workload.as_ref().map(|w| w.kind.as_str().to_string());
                match self.telemetry.sample_once(p, now, active.as_ref().map(|a| a.profile_id.clone()), kind) {
                    Ok(sample) => {
                        let _ = self.db.record_power_telemetry(&sample);
                        self.last_recorded = Some(state);
                        if ui {
                            (self.emit)("telemetry", serde_json::to_value(&sample).unwrap_or(Value::Null));
                        }
                    }
                    Err(_) => {
                        // No battery (desktop) or a transient IOCTL error: remember the state anyway.
                        self.last_recorded = Some(state);
                    }
                }
            }
        }

        // Automatic profile switching.
        if auto.enabled {
            if let Some(p) = power {
                let ghelper = auto.respect_ghelper && self.env.ghelper_running();
                let input = AutoSwitchInput {
                    on_ac: p.on_ac,
                    ac_known: p.ac_known,
                    percent: p.percent,
                    workload: workload.as_ref().map(|w| w.kind).unwrap_or(WorkloadKind::General),
                    ghelper_running: ghelper,
                    now_ms: now,
                    last_switch_ms: profiles::last_auto_switch_ms(&self.db),
                    current_profile: active.as_ref().map(|a| a.profile_id.clone()),
                    last_manual_ms: profiles::last_manual_ms(&self.db),
                };
                if let AutoDecision::Switch { profile_id, reason } = decide(&auto, &input) {
                    let ctx = Ctx { db: &self.db, backends: &self.backends };
                    match apply_profile(&ctx, &profile_id, Origin::Auto) {
                        Ok(outcome) => {
                            profiles::set_last_auto_switch_ms(&self.db, now);
                            (self.emit)("profileChanged", json!({ "profileId": outcome.profile_id, "reason": reason, "automatic": true, "snapshotId": outcome.snapshot_id }));
                        }
                        Err(e) => {
                            // Do not retry in a tight loop: remember the attempt like a switch.
                            profiles::set_last_auto_switch_ms(&self.db, now);
                            (self.emit)("autoSwitchError", json!({ "profileId": profile_id, "message": e.message }));
                        }
                    }
                }
            }
        }

        let interval = next_interval(&IntervalContext { on_ac: power.map(|p| p.on_ac).unwrap_or(true), percent: power.and_then(|p| p.percent), ui_lease_active: ui, measuring });
        if auto.enabled { interval.min(AUTO_SAFETY_NET) } else { interval }
    }
}

/// Starts the background runtime thread.
pub fn start(app: tauri::AppHandle, db: Database, backends: Backends, telemetry: SharedTelemetry) {
    use tauri::Emitter as _;
    let emit: Emitter = Box::new(move |kind, payload| {
        let _ = app.emit("optimizer-event", json!({ "kind": kind, "data": payload }));
    });
    let spawn = std::thread::Builder::new().name("winclean-optimizer-runtime".into()).spawn(move || {
        let (_events, notes) = super::power_events::PowerEvents::start(telemetry.wake.clone());
        for note in notes {
            eprintln!("winclean: {note}");
        }
        let mut runtime = Runtime::new(db, backends, telemetry.clone(), Box::new(SystemEnvironment::new()), emit);
        let mut woken = false;
        loop {
            if telemetry.should_stop() {
                break;
            }
            let sleep = runtime.tick(woken).clamp(Duration::from_secs(1), Duration::from_secs(3600));
            woken = telemetry.wake.wait(sleep);
            if woken {
                std::thread::sleep(DEBOUNCE);
            }
        }
    });
    if let Err(e) = spawn {
        eprintln!("winclean: could not start the optimizer runtime: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::optimizer::auto_switch::AutoSwitchConfig;
    use crate::optimizer::backend::fakes::{proc, FakePower, FakeProcesses, FakeServices, Fakes};
    use crate::optimizer::backend::WriteCapability;
    use crate::optimizer::battery::{BatteryInfo, BatteryReading, BatterySource, BATTERY_DISCHARGING};
    use crate::optimizer::policy::{SCHEME_BALANCED, SCHEME_HIGH_PERFORMANCE, SCHEME_POWER_SAVER};
    use crate::optimizer::telemetry::TelemetryService;
    use std::sync::{Arc, Mutex};

    struct Scripted {
        power: SystemPower,
        workload: WorkloadKind,
        ghelper: bool,
        now: i64,
    }
    struct FakeEnv(Arc<Mutex<Scripted>>);
    impl Environment for FakeEnv {
        fn power(&mut self) -> Result<SystemPower, String> {
            Ok(self.0.lock().unwrap().power)
        }
        fn workload(&mut self) -> WorkloadSnapshot {
            WorkloadSnapshot { kind: self.0.lock().unwrap().workload, evidence: vec!["scripted".into()], foreground: None }
        }
        fn ghelper_running(&mut self) -> bool {
            self.0.lock().unwrap().ghelper
        }
        fn now_ms(&mut self) -> i64 {
            self.0.lock().unwrap().now
        }
    }
    struct FakeBattery;
    impl BatterySource for FakeBattery {
        fn info(&mut self) -> Result<BatteryInfo, String> {
            Err("none".into())
        }
        fn read(&mut self) -> Result<BatteryReading, String> {
            Ok(BatteryReading::from_raw(BATTERY_DISCHARGING, 40_000, 15_000, -9_000))
        }
    }

    struct Harness {
        _dir: tempfile::TempDir,
        db: Database,
        fakes: Fakes,
        script: Arc<Mutex<Scripted>>,
        runtime: Runtime,
        events: Arc<Mutex<Vec<(String, Value)>>>,
    }

    fn harness() -> Harness {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(dir.path().join("t.sqlite3")).unwrap();
        db.migrate().unwrap();
        let fakes = Fakes {
            services: Arc::new(FakeServices::new(vec![], WriteCapability::Helper)),
            processes: Arc::new(FakeProcesses::new(vec![proc(10, "GoogleUpdate.exe", 0.0, false, 1_000), proc(12, "Code.exe", 1.0, false, 1_000)])),
            power: Arc::new(FakePower::new(SCHEME_BALANCED)),
        };
        let script = Arc::new(Mutex::new(Scripted { power: SystemPower { on_ac: true, ac_known: true, percent: Some(80), charging: false }, workload: WorkloadKind::General, ghelper: false, now: 10_000_000 }));
        let events: Arc<Mutex<Vec<(String, Value)>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = events.clone();
        let telemetry: SharedTelemetry = Arc::new(TelemetryService::new(Box::new(FakeBattery)));
        let runtime = Runtime::new(db.clone(), fakes.backends(), telemetry, Box::new(FakeEnv(script.clone())), Box::new(move |k, v| sink.lock().unwrap().push((k.to_string(), v))));
        Harness { _dir: dir, db, fakes, script, runtime, events }
    }

    fn enable_auto(h: &Harness) {
        profiles::set_auto_switch(&h.db, AutoSwitchConfig { enabled: true, ..Default::default() }).unwrap();
    }
    fn set(h: &Harness, f: impl FnOnce(&mut Scripted)) {
        f(&mut h.script.lock().unwrap());
    }
    fn advance(h: &Harness, ms: i64) {
        h.script.lock().unwrap().now += ms;
    }
    fn active(h: &Harness) -> Option<String> {
        profiles::get_active(&h.db).map(|a| a.profile_id)
    }

    #[test]
    fn nothing_is_switched_while_auto_switching_is_off() {
        let mut h = harness();
        h.runtime.tick(false);
        assert_eq!(active(&h), None);
        assert!(h.fakes.power.set_calls.lock().unwrap().is_empty());
    }

    #[test]
    fn telemetry_is_recorded_on_the_timer_and_on_real_changes_only() {
        let mut h = harness();
        h.runtime.tick(false);
        assert_eq!(h.db.power_telemetry_count().unwrap(), 1);
        h.runtime.tick(true); // event with nothing changed (e.g. alt-tab): no new row
        assert_eq!(h.db.power_telemetry_count().unwrap(), 1);
        set(&h, |s| s.power.percent = Some(79));
        h.runtime.tick(true); // percentage moved: record
        assert_eq!(h.db.power_telemetry_count().unwrap(), 2);
        h.runtime.tick(false); // timer: always record
        assert_eq!(h.db.power_telemetry_count().unwrap(), 3);
    }

    #[test]
    fn unplugging_at_low_battery_switches_to_the_saver_and_plugging_in_switches_back() {
        let mut h = harness();
        enable_auto(&h);
        h.runtime.tick(false);
        assert_eq!(active(&h).as_deref(), Some("balanced"));

        advance(&h, 200_000);
        set(&h, |s| s.power = SystemPower { on_ac: false, ac_known: true, percent: Some(15), charging: false });
        h.runtime.tick(true);
        assert_eq!(active(&h).as_deref(), Some("batterySaver"));
        assert_eq!(h.fakes.power.scheme(), SCHEME_POWER_SAVER);

        advance(&h, 200_000);
        set(&h, |s| s.power = SystemPower { on_ac: true, ac_known: true, percent: Some(16), charging: true });
        h.runtime.tick(true);
        assert_eq!(active(&h).as_deref(), Some("balanced"));
        assert_eq!(h.fakes.power.scheme(), SCHEME_BALANCED);
        let kinds: Vec<String> = h.events.lock().unwrap().iter().map(|(k, _)| k.clone()).collect();
        assert_eq!(kinds.iter().filter(|k| *k == "profileChanged").count(), 3);
    }

    #[test]
    fn a_game_coming_to_the_front_switches_to_gaming_and_back_when_it_leaves() {
        let mut h = harness();
        enable_auto(&h);
        h.runtime.tick(false);
        advance(&h, 200_000);
        set(&h, |s| s.workload = WorkloadKind::Gaming);
        h.runtime.tick(true);
        assert_eq!(active(&h).as_deref(), Some("gaming"));
        assert_eq!(h.fakes.power.scheme(), SCHEME_HIGH_PERFORMANCE);
        assert!(!h.fakes.processes.throttled(10), "no background throttling while gaming");

        advance(&h, 200_000);
        set(&h, |s| s.workload = WorkloadKind::General);
        h.runtime.tick(true);
        assert_eq!(active(&h).as_deref(), Some("balanced"));
        assert!(h.fakes.processes.throttled(10), "background work is throttled again outside the game");
        assert!(!h.fakes.processes.throttled(12), "the editor is never throttled");
    }

    #[test]
    fn the_dwell_time_stops_rapid_flapping() {
        let mut h = harness();
        enable_auto(&h);
        h.runtime.tick(false);
        let calls_after_first = h.fakes.power.set_calls.lock().unwrap().len();
        advance(&h, 10_000); // well under the 90 s dwell
        set(&h, |s| s.workload = WorkloadKind::Gaming);
        h.runtime.tick(true);
        assert_eq!(h.fakes.power.set_calls.lock().unwrap().len(), calls_after_first, "too soon after the last switch");
        advance(&h, 100_000);
        h.runtime.tick(true);
        assert_eq!(active(&h).as_deref(), Some("gaming"));
    }

    #[test]
    fn g_helper_running_pauses_automatic_switching() {
        let mut h = harness();
        enable_auto(&h);
        set(&h, |s| s.ghelper = true);
        h.runtime.tick(false);
        assert_eq!(active(&h), None);
        assert!(h.fakes.power.set_calls.lock().unwrap().is_empty(), "WinClean must not fight G-Helper over the power plan");
        set(&h, |s| s.ghelper = false);
        h.runtime.tick(true);
        assert_eq!(active(&h).as_deref(), Some("balanced"));
    }

    #[test]
    fn a_manual_choice_holds_off_automation() {
        let mut h = harness();
        enable_auto(&h);
        let ctx = Ctx { db: &h.db, backends: &h.fakes.backends() };
        apply_profile(&ctx, "coding", Origin::User).unwrap();
        // apply_profile stamps real wall-clock time; line the script up with it.
        let applied_at = profiles::get_active(&h.db).unwrap().applied_at_ms;
        set(&h, |s| s.now = applied_at + 60_000);
        set(&h, |s| s.workload = WorkloadKind::Gaming);
        h.runtime.tick(true);
        assert_eq!(active(&h).as_deref(), Some("coding"), "the user's choice wins for the override window");
        set(&h, |s| s.now = applied_at + 31 * 60_000);
        h.runtime.tick(true);
        assert_eq!(active(&h).as_deref(), Some("gaming"));
    }

    #[test]
    fn a_failing_switch_is_reported_and_not_retried_in_a_tight_loop() {
        let mut h = harness();
        enable_auto(&h);
        *h.fakes.power.fail_set.lock().unwrap() = true;
        h.runtime.tick(false);
        h.runtime.tick(true);
        h.runtime.tick(true);
        assert_eq!(h.fakes.power.set_calls.lock().unwrap().len(), 1, "the dwell time also covers failed attempts");
        assert!(h.events.lock().unwrap().iter().any(|(k, _)| k == "autoSwitchError"));
    }

    #[test]
    fn tick_chooses_the_sleep_from_the_situation() {
        let mut h = harness();
        assert_eq!(h.runtime.tick(false), Duration::from_secs(300), "plugged in, nobody watching");
        set(&h, |s| s.power = SystemPower { on_ac: false, ac_known: true, percent: Some(70), charging: false });
        assert_eq!(h.runtime.tick(false), Duration::from_secs(60));
        enable_auto(&h);
        assert_eq!(h.runtime.tick(false), Duration::from_secs(60).min(AUTO_SAFETY_NET));
    }
}
