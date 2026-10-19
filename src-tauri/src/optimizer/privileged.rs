//! The privileged side of WinClean: everything the elevated helper may do.
//!
//! [`guarded_set_start_type`] is the single authoritative gate for service
//! writes. It re-reads the SCM itself, re-applies the full protection policy
//! (names, vendor prefixes, dependency closure, disable rules) and verifies
//! the result by re-reading — it trusts nothing about the caller's view.
//! The same function backs the already-elevated in-process path, so there is
//! exactly one way to write a service start type.

use std::io::{Read, Write};

use super::helper_protocol::{read_frame, write_frame, ErrorCode, FrameError, Request, Response, PROTOCOL_VERSION};
use super::service_plan::{validate_service_plan, ServiceChange};
use super::services::{ServiceInfo, StartType};

/// Raw SCM access used by the privileged code path. Implemented by the
/// Windows SCM and by fakes in tests.
pub trait ScmAccess: Send + Sync {
    fn list(&self) -> Result<Vec<ServiceInfo>, String>;
    fn write_start_type(&self, name: &str, target: StartType) -> Result<(), String>;
}

/// The real SCM (Windows) — writes need an elevated token.
pub struct SystemScm;

impl ScmAccess for SystemScm {
    fn list(&self) -> Result<Vec<ServiceInfo>, String> {
        super::services::list_services()
    }
    fn write_start_type(&self, name: &str, target: StartType) -> Result<(), String> {
        super::services::set_service_start_type(name, target)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Guarded {
    pub before: StartType,
    pub after: StartType,
    /// False when the service already had the requested start type.
    pub changed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GuardError {
    Rejected(String),
    Conflict(String),
    Failed(String),
    VerifyFailed(String),
}

impl GuardError {
    pub fn code(&self) -> ErrorCode {
        match self {
            GuardError::Rejected(_) => ErrorCode::Rejected,
            GuardError::Conflict(_) => ErrorCode::Conflict,
            GuardError::Failed(_) => ErrorCode::Failed,
            GuardError::VerifyFailed(_) => ErrorCode::VerifyFailed,
        }
    }
    pub fn message(&self) -> &str {
        match self {
            GuardError::Rejected(m) | GuardError::Conflict(m) | GuardError::Failed(m) | GuardError::VerifyFailed(m) => m,
        }
    }
}

/// Changes one service's start type after re-checking everything.
pub fn guarded_set_start_type(scm: &dyn ScmAccess, name: &str, target: &str, expect_from: Option<&str>) -> Result<Guarded, GuardError> {
    let target = StartType::parse(target).ok_or_else(|| GuardError::Rejected(format!("invalid start type '{target}'")))?;
    let services = scm.list().map_err(GuardError::Failed)?;
    let current = services
        .iter()
        .find(|s| s.name.eq_ignore_ascii_case(name))
        .ok_or_else(|| GuardError::Rejected("unknown service".into()))?;
    let before = current.current_start_type();

    // Already there: idempotent success (a retry after a lost response).
    if before == Some(target) && current.classification.risk != super::policy::ServiceRisk::Protected {
        return Ok(Guarded { before: target, after: target, changed: false });
    }

    let plan = validate_service_plan(&[ServiceChange { service_name: name.to_string(), start_type: target.as_str().to_string() }], &services);
    let Some(item) = plan.items.first() else {
        let reason = plan.rejected.first().map(|r| r.reason.clone()).unwrap_or_else(|| "rejected".into());
        return Err(GuardError::Rejected(reason));
    };
    if let Some(expected) = expect_from {
        let expected = StartType::parse(expected).ok_or_else(|| GuardError::Rejected(format!("invalid expected start type '{expected}'")))?;
        if expected != item.from {
            return Err(GuardError::Conflict(format!(
                "'{}' is now {} but the reviewed change expected {}; refresh and review again",
                item.display_name,
                item.from.as_str(),
                expected.as_str()
            )));
        }
    }

    scm.write_start_type(&item.name, target).map_err(GuardError::Failed)?;

    let after = scm
        .list()
        .map_err(|e| GuardError::VerifyFailed(format!("written, but re-read failed: {e}")))?
        .into_iter()
        .find(|s| s.name.eq_ignore_ascii_case(name))
        .and_then(|s| s.current_start_type());
    if after != Some(target) {
        return Err(GuardError::VerifyFailed(format!(
            "SCM reports {} after the change, expected {}",
            after.map(|a| a.as_str()).unwrap_or("unknown"),
            target.as_str()
        )));
    }
    Ok(Guarded { before: item.from, after: target, changed: true })
}

/// Most requests one helper session will serve.
pub const MAX_REQUESTS_PER_SESSION: u32 = 1000;

#[derive(Debug, PartialEq, Eq)]
pub enum EndReason {
    PeerClosed,
    Shutdown,
    BadHello,
    ProtocolViolation,
    RequestLimit,
}

/// Serves one authenticated connection until it ends.
///
/// Peer authentication (client PID and image path) happens in the transport
/// layer before this is called; here the first frame must still be a
/// version-matched `hello`.
pub fn serve<S: Read + Write>(stream: &mut S, scm: &dyn ScmAccess, audit: &mut dyn FnMut(&str)) -> EndReason {
    match read_frame::<_, Request>(stream) {
        Ok(Request::Hello { version }) if version == PROTOCOL_VERSION => {
            if write_frame(stream, &Response::ok(format!("winclean-helper protocol {PROTOCOL_VERSION}"))).is_err() {
                return EndReason::PeerClosed;
            }
        }
        Ok(Request::Hello { version }) => {
            let _ = write_frame(stream, &Response::err(ErrorCode::BadRequest, format!("unsupported protocol version {version}")));
            return EndReason::BadHello;
        }
        Ok(_) => {
            let _ = write_frame(stream, &Response::err(ErrorCode::BadRequest, "hello must be the first request"));
            return EndReason::BadHello;
        }
        Err(FrameError::Closed) => return EndReason::PeerClosed,
        Err(e) => {
            let _ = write_frame(stream, &Response::err(ErrorCode::BadRequest, e.to_string()));
            return EndReason::BadHello;
        }
    }

    for _ in 0..MAX_REQUESTS_PER_SESSION {
        let request = match read_frame::<_, Request>(stream) {
            Ok(r) => r,
            Err(FrameError::Closed) => return EndReason::PeerClosed,
            Err(e) => {
                // After a bad frame the stream cannot be re-synchronized: close.
                audit(&format!("protocol violation: {e}"));
                let _ = write_frame(stream, &Response::err(ErrorCode::BadRequest, e.to_string()));
                return EndReason::ProtocolViolation;
            }
        };
        let (response, stop) = match request {
            Request::Hello { .. } => (Response::err(ErrorCode::BadRequest, "already greeted"), false),
            Request::Ping {} => (Response::ok("pong"), false),
            Request::Shutdown {} => (Response::ok("bye"), true),
            Request::SetStartType { name, start_type, expect_from } => {
                let outcome = guarded_set_start_type(scm, &name, &start_type, expect_from.as_deref());
                match &outcome {
                    Ok(g) => audit(&format!("setStartType {name}: {} -> {} (changed={})", g.before.as_str(), g.after.as_str(), g.changed)),
                    Err(e) => audit(&format!("setStartType {name} refused: {}", e.message())),
                }
                let response = match outcome {
                    Ok(g) => Response::Ok {
                        detail: Some(if g.changed { "changed".into() } else { "already set".into() }),
                        before: Some(g.before.as_str().into()),
                        after: Some(g.after.as_str().into()),
                    },
                    Err(e) => Response::err(e.code(), e.message().to_string()),
                };
                (response, false)
            }
        };
        if write_frame(stream, &response).is_err() {
            return EndReason::PeerClosed;
        }
        if stop {
            return EndReason::Shutdown;
        }
    }
    EndReason::RequestLimit
}

#[cfg(test)]
pub(crate) mod testing {
    use super::*;
    use std::sync::Mutex;

    /// In-memory SCM that records every write.
    pub struct FakeScm {
        pub services: Mutex<Vec<ServiceInfo>>,
        pub writes: Mutex<Vec<(String, StartType)>>,
        /// When set, writes succeed but do not change state (verify must fail).
        pub swallow_writes: bool,
        /// Fail every write with this message (e.g. access denied).
        pub fail_writes: Option<String>,
    }

    impl FakeScm {
        pub fn new(services: Vec<ServiceInfo>) -> Self {
            let mut services = services;
            super::super::service_graph::finalize(&mut services);
            Self { services: Mutex::new(services), writes: Mutex::new(Vec::new()), swallow_writes: false, fail_writes: None }
        }
        pub fn writes(&self) -> Vec<(String, StartType)> {
            self.writes.lock().unwrap().clone()
        }
        pub fn start_type(&self, name: &str) -> String {
            self.services.lock().unwrap().iter().find(|s| s.name == name).unwrap().start_type.clone()
        }
    }

    impl ScmAccess for FakeScm {
        fn list(&self) -> Result<Vec<ServiceInfo>, String> {
            Ok(self.services.lock().unwrap().clone())
        }
        fn write_start_type(&self, name: &str, target: StartType) -> Result<(), String> {
            if let Some(msg) = &self.fail_writes {
                return Err(msg.clone());
            }
            self.writes.lock().unwrap().push((name.to_string(), target));
            if !self.swallow_writes {
                let mut services = self.services.lock().unwrap();
                if let Some(s) = services.iter_mut().find(|s| s.name == name) {
                    s.start_type = target.as_str().to_string();
                    s.delayed_auto_start = target == StartType::AutomaticDelayed;
                }
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::FakeScm;
    use super::*;
    use crate::optimizer::service_graph::testing::svc;
    use std::io::Cursor;

    fn fixture() -> FakeScm {
        FakeScm::new(vec![
            svc("WinDefend", "Microsoft Defender Antivirus Service", "automatic", "running", &["RpcSs"]),
            svc("RpcSs", "Remote Procedure Call (RPC)", "automatic", "running", &[]),
            svc("AsusOptimization", "ASUS Optimization", "automatic", "running", &[]),
            svc("DiagTrack", "Connected User Experiences and Telemetry", "automatic", "running", &[]),
            svc("MysteryVendorSvc", "Mystery", "automatic", "running", &[]),
        ])
    }

    /// Duplex in-memory stream: reads from `input`, collects writes in `output`.
    struct Duplex {
        input: Cursor<Vec<u8>>,
        output: Vec<u8>,
    }
    impl Read for Duplex {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.input.read(buf)
        }
    }
    impl Write for Duplex {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.output.write(buf)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn session(requests: &[Request]) -> Duplex {
        let mut input = Vec::new();
        for r in requests {
            write_frame(&mut input, r).unwrap();
        }
        Duplex { input: Cursor::new(input), output: Vec::new() }
    }

    fn responses(d: &Duplex) -> Vec<Response> {
        let mut out = Vec::new();
        let mut cursor = d.output.as_slice();
        while let Ok(r) = read_frame::<_, Response>(&mut cursor) {
            out.push(r);
        }
        out
    }

    fn set(name: &str, to: &str) -> Request {
        Request::SetStartType { name: name.into(), start_type: to.into(), expect_from: None }
    }

    #[test]
    fn helper_refuses_protected_services_even_when_the_caller_insists() {
        let scm = fixture();
        for name in ["WinDefend", "RpcSs", "AsusOptimization", "rpcss", "ASUSOPTIMIZATION"] {
            let err = guarded_set_start_type(&scm, name, "disabled", None).unwrap_err();
            assert!(matches!(err, GuardError::Rejected(ref m) if m.starts_with("protected")), "{name}: {err:?}");
            let err = guarded_set_start_type(&scm, name, "manual", None).unwrap_err();
            assert!(matches!(err, GuardError::Rejected(_)), "{name}");
        }
        assert!(scm.writes().is_empty(), "no protected service may ever reach the SCM writer");
    }

    #[test]
    fn helper_never_disables_unknown_services_but_allows_manual() {
        let scm = fixture();
        assert!(matches!(guarded_set_start_type(&scm, "MysteryVendorSvc", "disabled", None), Err(GuardError::Rejected(_))));
        assert!(scm.writes().is_empty());
        let g = guarded_set_start_type(&scm, "MysteryVendorSvc", "manual", None).unwrap();
        assert_eq!((g.before, g.after, g.changed), (StartType::Automatic, StartType::Manual, true));
        assert_eq!(scm.writes(), vec![("MysteryVendorSvc".to_string(), StartType::Manual)]);
    }

    #[test]
    fn helper_rejects_malformed_names_and_start_types_without_touching_the_scm() {
        let scm = fixture();
        for (name, to) in [("..\\evil", "manual"), ("DiagTrack", "boot"), ("DiagTrack", "AUTOMATIC "), ("", "manual"), ("Nope", "manual")] {
            assert!(guarded_set_start_type(&scm, name, to, None).is_err(), "{name}/{to}");
        }
        assert!(scm.writes().is_empty());
    }

    #[test]
    fn stale_review_is_a_conflict_and_writes_nothing() {
        let scm = fixture();
        let err = guarded_set_start_type(&scm, "DiagTrack", "manual", Some("manual")).unwrap_err();
        assert!(matches!(err, GuardError::Conflict(_)), "{err:?}");
        assert!(scm.writes().is_empty());
        assert!(guarded_set_start_type(&scm, "DiagTrack", "manual", Some("automatic")).is_ok());
    }

    #[test]
    fn repeating_a_change_is_idempotent() {
        let scm = fixture();
        guarded_set_start_type(&scm, "DiagTrack", "manual", None).unwrap();
        let again = guarded_set_start_type(&scm, "DiagTrack", "manual", None).unwrap();
        assert!(!again.changed);
        assert_eq!(scm.writes().len(), 1);
    }

    #[test]
    fn a_write_that_does_not_stick_is_reported_not_trusted() {
        let mut scm = fixture();
        scm.swallow_writes = true;
        let err = guarded_set_start_type(&scm, "DiagTrack", "manual", None).unwrap_err();
        assert!(matches!(err, GuardError::VerifyFailed(_)), "{err:?}");
    }

    #[test]
    fn scm_failures_surface_as_failed() {
        let mut scm = fixture();
        scm.fail_writes = Some("Access is denied. (os error 5)".into());
        let err = guarded_set_start_type(&scm, "DiagTrack", "manual", None).unwrap_err();
        assert_eq!(err, GuardError::Failed("Access is denied. (os error 5)".into()));
    }

    #[test]
    fn session_requires_hello_first() {
        let scm = fixture();
        let mut d = session(&[set("DiagTrack", "manual")]);
        assert_eq!(serve(&mut d, &scm, &mut |_| {}), EndReason::BadHello);
        assert!(scm.writes().is_empty());
        assert!(matches!(&responses(&d)[0], Response::Err { code: ErrorCode::BadRequest, .. }));
    }

    #[test]
    fn session_rejects_a_mismatched_protocol_version() {
        let scm = fixture();
        let mut d = session(&[Request::Hello { version: PROTOCOL_VERSION + 1 }, set("DiagTrack", "manual")]);
        assert_eq!(serve(&mut d, &scm, &mut |_| {}), EndReason::BadHello);
        assert!(scm.writes().is_empty());
    }

    #[test]
    fn full_session_applies_allowed_changes_and_refuses_the_rest() {
        let scm = fixture();
        let mut log = Vec::new();
        let mut d = session(&[
            Request::Hello { version: PROTOCOL_VERSION },
            Request::Ping {},
            set("DiagTrack", "manual"),
            set("WinDefend", "disabled"),
            Request::Shutdown {},
        ]);
        assert_eq!(serve(&mut d, &scm, &mut |l| log.push(l.to_string())), EndReason::Shutdown);
        let r = responses(&d);
        assert_eq!(r.len(), 5);
        assert!(matches!(&r[2], Response::Ok { before: Some(b), after: Some(a), .. } if b == "automatic" && a == "manual"));
        assert!(matches!(&r[3], Response::Err { code: ErrorCode::Rejected, .. }));
        assert_eq!(scm.writes(), vec![("DiagTrack".to_string(), StartType::Manual)]);
        assert!(log.iter().any(|l| l.contains("WinDefend refused")));
    }

    #[test]
    fn a_malformed_frame_ends_the_session_without_acting() {
        let scm = fixture();
        let mut input = Vec::new();
        write_frame(&mut input, &Request::Hello { version: PROTOCOL_VERSION }).unwrap();
        let junk = br#"{"op":"setStartType","name":"DiagTrack","startType":"manual","expectFrom":null,"extra":true}"#;
        input.extend_from_slice(&(junk.len() as u32).to_le_bytes());
        input.extend_from_slice(junk);
        write_frame(&mut input, &set("DiagTrack", "manual")).unwrap();
        let mut d = Duplex { input: Cursor::new(input), output: Vec::new() };
        assert_eq!(serve(&mut d, &scm, &mut |_| {}), EndReason::ProtocolViolation);
        assert!(scm.writes().is_empty(), "frames after a violation are never read");
    }

    #[test]
    fn a_session_is_bounded() {
        let scm = fixture();
        let mut requests = vec![Request::Hello { version: PROTOCOL_VERSION }];
        requests.extend((0..MAX_REQUESTS_PER_SESSION + 5).map(|_| Request::Ping {}));
        let mut d = session(&requests);
        assert_eq!(serve(&mut d, &scm, &mut |_| {}), EndReason::RequestLimit);
    }
}
