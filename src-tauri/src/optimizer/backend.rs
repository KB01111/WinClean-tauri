//! Backends: the seams between orchestration logic and the operating system.
//!
//! Every mutation the optimizer performs goes through one of these traits.
//! Production uses the Windows implementations; tests use the in-memory
//! fakes in [`fakes`], which makes service protection, rollback, EcoQoS
//! restoration, profile switching and the elevation boundary testable on any
//! OS.

use std::sync::Arc;

use serde::Serialize;

use super::helper_client::HelperManager;
use super::policy::EcoQosMode;
use super::power::BatteryStatus;
use super::privileged::{guarded_set_start_type, GuardError, SystemScm};
use super::processes::{ProcessInfo, ProcessLiveness};
use super::services::{ServiceInfo, StartType};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BackendError {
    /// The write needs administrator rights and no elevated path exists.
    ElevationRequired(String),
    /// Policy refused the change; nothing was modified.
    Rejected(String),
    /// The target moved since it was reviewed; nothing was modified.
    Conflict(String),
    Failed(String),
}

impl BackendError {
    pub fn message(&self) -> &str {
        match self {
            BackendError::ElevationRequired(m) | BackendError::Rejected(m) | BackendError::Conflict(m) | BackendError::Failed(m) => m,
        }
    }
    pub fn is_elevation(&self) -> bool {
        matches!(self, BackendError::ElevationRequired(_))
    }
    /// True when the backend guarantees it did not modify anything.
    pub fn nothing_changed(&self) -> bool {
        matches!(self, BackendError::ElevationRequired(_) | BackendError::Rejected(_) | BackendError::Conflict(_))
    }
}

impl From<GuardError> for BackendError {
    fn from(e: GuardError) -> Self {
        match e {
            GuardError::Rejected(m) => BackendError::Rejected(m),
            GuardError::Conflict(m) => BackendError::Conflict(m),
            GuardError::Failed(m) | GuardError::VerifyFailed(m) => BackendError::Failed(m),
        }
    }
}

/// How service writes would currently be performed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum WriteCapability {
    /// WinClean itself runs elevated.
    Elevated,
    /// The administrator helper is connected.
    Helper,
    Unavailable,
}

pub trait ServiceBackend: Send + Sync {
    fn list(&self) -> Result<Vec<ServiceInfo>, String>;
    fn write_capability(&self) -> WriteCapability;
    /// `expect_from` is the start type the change was reviewed against.
    fn set_start_type(&self, name: &str, target: StartType, expect_from: Option<StartType>) -> Result<(), BackendError>;
}

pub trait ProcessBackend: Send + Sync {
    fn list_all(&self) -> Result<Vec<ProcessInfo>, String>;
    fn set_ecoqos(&self, pid: u32, expected_name: &str, not_started_after_ms: Option<i64>, enable: bool, mode: EcoQosMode) -> Result<String, String>;
    fn liveness(&self, pid: u32) -> ProcessLiveness;
    fn ecoqos_state(&self, pid: u32) -> Option<bool>;
}

pub trait PowerBackend: Send + Sync {
    fn status(&self) -> Result<BatteryStatus, String>;
    fn set_scheme(&self, guid: &str) -> Result<(), String>;
}

/// The set of backends an optimizer operation runs against.
#[derive(Clone)]
pub struct Backends {
    pub services: Arc<dyn ServiceBackend>,
    pub processes: Arc<dyn ProcessBackend>,
    pub power: Arc<dyn PowerBackend>,
}

impl Backends {
    /// Production backends: Windows APIs, with service writes routed through
    /// the elevated helper (or directly when WinClean itself is elevated).
    pub fn system(helper: Arc<HelperManager>) -> Self {
        Self {
            services: Arc::new(SystemServiceBackend { helper }),
            processes: Arc::new(SystemProcessBackend),
            power: Arc::new(SystemPowerBackend),
        }
    }
}

pub struct SystemServiceBackend {
    helper: Arc<HelperManager>,
}

impl ServiceBackend for SystemServiceBackend {
    fn list(&self) -> Result<Vec<ServiceInfo>, String> {
        super::services::list_services()
    }

    fn write_capability(&self) -> WriteCapability {
        if super::helper_windows::process_is_elevated() {
            WriteCapability::Elevated
        } else if self.helper.is_connected() {
            WriteCapability::Helper
        } else {
            WriteCapability::Unavailable
        }
    }

    fn set_start_type(&self, name: &str, target: StartType, expect_from: Option<StartType>) -> Result<(), BackendError> {
        match self.write_capability() {
            WriteCapability::Elevated => guarded_set_start_type(&SystemScm, name, target.as_str(), expect_from.map(|e| e.as_str())).map(|_| ()).map_err(BackendError::from),
            WriteCapability::Helper => self.helper.set_start_type(name, target, expect_from),
            WriteCapability::Unavailable => Err(BackendError::ElevationRequired(
                "Changing a service needs administrator rights. Enable the administrator helper (UAC prompt) and try again.".into(),
            )),
        }
    }
}

pub struct SystemProcessBackend;

impl ProcessBackend for SystemProcessBackend {
    fn list_all(&self) -> Result<Vec<ProcessInfo>, String> {
        super::processes::list_all_processes()
    }
    fn set_ecoqos(&self, pid: u32, expected_name: &str, not_started_after_ms: Option<i64>, enable: bool, mode: EcoQosMode) -> Result<String, String> {
        super::processes::set_process_ecoqos(pid, expected_name, not_started_after_ms, enable, mode)
    }
    fn liveness(&self, pid: u32) -> ProcessLiveness {
        super::processes::process_liveness(pid)
    }
    fn ecoqos_state(&self, pid: u32) -> Option<bool> {
        super::processes::process_ecoqos_state(pid)
    }
}

pub struct SystemPowerBackend;

impl PowerBackend for SystemPowerBackend {
    fn status(&self) -> Result<BatteryStatus, String> {
        super::power::battery_status()
    }
    fn set_scheme(&self, guid: &str) -> Result<(), String> {
        super::power::set_active_scheme(guid)
    }
}

/// In-memory fakes with the same safety semantics as the real backends.
#[cfg(test)]
pub mod fakes {
    use super::*;
    use crate::optimizer::policy::{classify_process_for_ecoqos_mode, EcoQosDecision};
    use crate::optimizer::privileged::testing::FakeScm;
    use std::sync::Mutex;

    pub struct FakeServices {
        pub scm: FakeScm,
        pub capability: Mutex<WriteCapability>,
    }

    impl FakeServices {
        pub fn new(services: Vec<ServiceInfo>, capability: WriteCapability) -> Self {
            Self { scm: FakeScm::new(services), capability: Mutex::new(capability) }
        }
    }

    impl ServiceBackend for FakeServices {
        fn list(&self) -> Result<Vec<ServiceInfo>, String> {
            use crate::optimizer::privileged::ScmAccess;
            self.scm.list()
        }
        fn write_capability(&self) -> WriteCapability {
            *self.capability.lock().unwrap()
        }
        fn set_start_type(&self, name: &str, target: StartType, expect_from: Option<StartType>) -> Result<(), BackendError> {
            if self.write_capability() == WriteCapability::Unavailable {
                return Err(BackendError::ElevationRequired("needs the administrator helper".into()));
            }
            guarded_set_start_type(&self.scm, name, target.as_str(), expect_from.map(|e| e.as_str())).map(|_| ()).map_err(BackendError::from)
        }
    }

    #[derive(Clone, Debug)]
    pub struct FakeProc {
        pub info: ProcessInfo,
        pub started_ms: i64,
        pub alive: bool,
        /// Set when another app (not WinClean) throttled it.
        pub external: bool,
    }

    pub struct FakeProcesses {
        pub procs: Mutex<Vec<FakeProc>>,
        pub set_calls: Mutex<Vec<(u32, bool)>>,
    }

    impl FakeProcesses {
        pub fn new(procs: Vec<FakeProc>) -> Self {
            Self { procs: Mutex::new(procs), set_calls: Mutex::new(Vec::new()) }
        }
        pub fn throttled(&self, pid: u32) -> bool {
            self.procs.lock().unwrap().iter().find(|p| p.info.pid == pid).map(|p| p.info.eco_qos_applied).unwrap_or(false)
        }
    }

    pub fn proc(pid: u32, name: &str, cpu: f32, fg: bool, started_ms: i64) -> FakeProc {
        FakeProc {
            info: ProcessInfo {
                pid,
                parent_pid: 0,
                name: name.to_string(),
                working_set_mb: 10.0,
                cpu_percent_recent: cpu,
                is_foreground: fg,
                in_foreground_tree: false,
                eco_decision: classify_process_for_ecoqos_mode(name, fg, cpu, EcoQosMode::Conservative),
                eco_qos_applied: false,
            },
            started_ms,
            alive: true,
            external: false,
        }
    }

    impl ProcessBackend for FakeProcesses {
        fn list_all(&self) -> Result<Vec<ProcessInfo>, String> {
            Ok(self.procs.lock().unwrap().iter().filter(|p| p.alive).map(|p| p.info.clone()).collect())
        }
        fn set_ecoqos(&self, pid: u32, expected_name: &str, not_started_after_ms: Option<i64>, enable: bool, mode: EcoQosMode) -> Result<String, String> {
            self.set_calls.lock().unwrap().push((pid, enable));
            let mut procs = self.procs.lock().unwrap();
            let p = procs.iter_mut().find(|p| p.info.pid == pid && p.alive).ok_or_else(|| format!("OpenProcess({pid}) failed"))?;
            if !expected_name.is_empty() && !p.info.name.eq_ignore_ascii_case(expected_name) {
                return Err(format!("pid {pid} now belongs to '{}', not '{expected_name}'", p.info.name));
            }
            if let Some(limit) = not_started_after_ms {
                if p.started_ms > limit {
                    return Err(format!("pid {pid} now belongs to a newer '{}' process", p.info.name));
                }
            }
            if enable {
                if p.info.eco_qos_applied {
                    return Err(format!("'{}' is already EcoQoS-throttled; WinClean will not claim it", p.info.name));
                }
                let decision = classify_process_for_ecoqos_mode(&p.info.name, p.info.is_foreground, p.info.cpu_percent_recent, mode);
                if decision != EcoQosDecision::Eligible {
                    return Err(format!("'{}' is {decision:?} and will not be throttled", p.info.name));
                }
            }
            p.info.eco_qos_applied = enable;
            Ok(p.info.name.clone())
        }
        fn liveness(&self, pid: u32) -> ProcessLiveness {
            match self.procs.lock().unwrap().iter().find(|p| p.info.pid == pid) {
                Some(p) if p.alive => ProcessLiveness::Alive { started_ms: p.started_ms },
                Some(_) | None => ProcessLiveness::Gone,
            }
        }
        fn ecoqos_state(&self, pid: u32) -> Option<bool> {
            self.procs.lock().unwrap().iter().find(|p| p.info.pid == pid && p.alive).map(|p| p.info.eco_qos_applied)
        }
    }

    pub struct FakePower {
        pub scheme: Mutex<String>,
        pub on_ac: Mutex<bool>,
        pub percent: Mutex<Option<u8>>,
        pub fail_set: Mutex<bool>,
        pub set_calls: Mutex<Vec<String>>,
    }

    impl FakePower {
        pub fn new(scheme: &str) -> Self {
            Self { scheme: Mutex::new(scheme.to_string()), on_ac: Mutex::new(true), percent: Mutex::new(Some(80)), fail_set: Mutex::new(false), set_calls: Mutex::new(Vec::new()) }
        }
        pub fn scheme(&self) -> String {
            self.scheme.lock().unwrap().clone()
        }
    }

    impl PowerBackend for FakePower {
        fn status(&self) -> Result<BatteryStatus, String> {
            Ok(BatteryStatus {
                present: true,
                on_ac: *self.on_ac.lock().unwrap(),
                ac_known: true,
                percent: *self.percent.lock().unwrap(),
                seconds_remaining: None,
                active_scheme_guid: Some(self.scheme()),
                active_scheme_name: Some("fake".into()),
            })
        }
        fn set_scheme(&self, guid: &str) -> Result<(), String> {
            self.set_calls.lock().unwrap().push(guid.to_string());
            if *self.fail_set.lock().unwrap() {
                return Err("PowerSetActiveScheme failed".into());
            }
            *self.scheme.lock().unwrap() = guid.to_string();
            Ok(())
        }
    }

    pub struct Fakes {
        pub services: Arc<FakeServices>,
        pub processes: Arc<FakeProcesses>,
        pub power: Arc<FakePower>,
    }

    impl Fakes {
        pub fn backends(&self) -> Backends {
            Backends { services: self.services.clone(), processes: self.processes.clone(), power: self.power.clone() }
        }
    }
}
