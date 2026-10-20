//! Client side of the administrator helper.
//!
//! `HelperManager` owns at most one connection to the elevated helper. The
//! primary app stays unprivileged: it only ever sends the tiny requests
//! defined in `helper_protocol`, and the helper re-validates each one.

use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use serde::Serialize;

use super::backend::BackendError;
use super::helper_protocol::{read_frame, write_frame, ErrorCode, Request, Response, PROTOCOL_VERSION};
use super::services::StartType;

/// Byte stream to the helper (a named pipe in production, a socket in tests).
pub trait Transport: Read + Write + Send {}
impl<T: Read + Write + Send> Transport for T {}

/// Starts the helper and returns an *authenticated* transport: the launcher
/// is responsible for proving that the other end is the helper process it
/// just started (see `helper_windows`).
pub trait HelperLauncher: Send + Sync {
    fn supported(&self) -> bool;
    /// Whether the executable location is safe to elevate (not user-writable).
    fn location_trusted(&self) -> bool {
        true
    }
    fn launch(&self) -> Result<Box<dyn Transport>, String>;
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HelperStatus {
    pub supported: bool,
    pub connected: bool,
    /// WinClean itself is already running elevated (no helper needed).
    pub process_elevated: bool,
    /// False when the executable sits in a user-writable folder; elevating it
    /// would let any program running as the user swap the binary.
    pub location_trusted: bool,
    pub protocol_version: u32,
    pub last_error: Option<String>,
}

pub struct HelperManager {
    launcher: Box<dyn HelperLauncher>,
    conn: Mutex<Option<Box<dyn Transport>>>,
    connected: AtomicBool,
    last_error: Mutex<Option<String>>,
}

impl HelperManager {
    pub fn new(launcher: Box<dyn HelperLauncher>) -> Self {
        Self { launcher, conn: Mutex::new(None), connected: AtomicBool::new(false), last_error: Mutex::new(None) }
    }

    /// Cheap, never blocks on an in-flight request.
    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Acquire)
    }

    pub fn status(&self) -> HelperStatus {
        HelperStatus {
            supported: self.launcher.supported(),
            connected: self.is_connected(),
            process_elevated: super::helper_windows::process_is_elevated(),
            location_trusted: self.launcher.location_trusted(),
            protocol_version: PROTOCOL_VERSION,
            last_error: self.last_error.lock().unwrap_or_else(|p| p.into_inner()).clone(),
        }
    }

    fn remember_error(&self, error: Option<String>) {
        *self.last_error.lock().unwrap_or_else(|p| p.into_inner()) = error;
    }

    /// Launches the helper (UAC prompt) and performs the hello handshake.
    /// Blocks until the user answers the prompt, so callers run it on a
    /// blocking thread.
    pub fn connect(&self, allow_untrusted_location: bool) -> Result<(), String> {
        if self.is_connected() {
            return Ok(());
        }
        if !self.launcher.supported() {
            return Err("The administrator helper is only available on Windows.".into());
        }
        if !self.launcher.location_trusted() && !allow_untrusted_location {
            let msg = "WinClean is running from a folder that your user account can modify. Elevating it there would let any program running as you replace the executable. Install WinClean under Program Files, or explicitly confirm this risk.".to_string();
            self.remember_error(Some(msg.clone()));
            return Err(msg);
        }
        let mut guard = self.conn.lock().unwrap_or_else(|p| p.into_inner());
        let mut transport = match self.launcher.launch() {
            Ok(t) => t,
            Err(e) => {
                self.remember_error(Some(e.clone()));
                return Err(e);
            }
        };
        let hello = write_frame(&mut transport, &Request::Hello { version: PROTOCOL_VERSION })
            .map_err(|e| e.to_string())
            .and_then(|_| read_frame::<_, Response>(&mut transport).map_err(|e| e.to_string()));
        match hello {
            Ok(Response::Ok { .. }) => {
                *guard = Some(transport);
                self.connected.store(true, Ordering::Release);
                self.remember_error(None);
                Ok(())
            }
            Ok(Response::Err { message, .. }) => {
                self.remember_error(Some(message.clone()));
                Err(format!("The administrator helper refused the handshake: {message}"))
            }
            Err(e) => {
                self.remember_error(Some(e.clone()));
                Err(format!("The administrator helper did not answer: {e}"))
            }
        }
    }

    fn call(&self, request: &Request) -> Result<Response, BackendError> {
        let mut guard = self.conn.lock().unwrap_or_else(|p| p.into_inner());
        let Some(transport) = guard.as_mut() else {
            self.connected.store(false, Ordering::Release);
            return Err(BackendError::ElevationRequired("The administrator helper is not running. Enable it and try again.".into()));
        };
        let result = write_frame(transport, request).map_err(|e| e.to_string()).and_then(|_| read_frame::<_, Response>(transport).map_err(|e| e.to_string()));
        match result {
            Ok(r) => Ok(r),
            Err(e) => {
                // A broken pipe means the helper exited: forget the connection
                // so the next privileged action asks for elevation again.
                *guard = None;
                self.connected.store(false, Ordering::Release);
                self.remember_error(Some(e.clone()));
                Err(BackendError::ElevationRequired(format!("The administrator helper disconnected ({e}). Enable it again to continue.")))
            }
        }
    }

    pub fn ping(&self) -> bool {
        matches!(self.call(&Request::Ping {}), Ok(Response::Ok { .. }))
    }

    pub fn set_start_type(&self, name: &str, target: StartType, expect_from: Option<StartType>) -> Result<(), BackendError> {
        let request = Request::SetStartType { name: name.to_string(), start_type: target.as_str().to_string(), expect_from: expect_from.map(|e| e.as_str().to_string()) };
        match self.call(&request)? {
            Response::Ok { .. } => Ok(()),
            Response::Err { code, message } => Err(match code {
                ErrorCode::Rejected => BackendError::Rejected(message),
                ErrorCode::Conflict => BackendError::Conflict(message),
                ErrorCode::BadRequest => BackendError::Rejected(message),
                ErrorCode::Failed | ErrorCode::VerifyFailed => BackendError::Failed(message),
            }),
        }
    }

    /// Asks the helper to exit and forgets the connection.
    pub fn disconnect(&self) {
        let mut guard = self.conn.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(t) = guard.as_mut() {
            let _ = write_frame(t, &Request::Shutdown {});
            let _ = read_frame::<_, Response>(t);
        }
        *guard = None;
        self.connected.store(false, Ordering::Release);
    }
}

/// A launcher for platforms without a helper.
pub struct UnsupportedLauncher;

impl HelperLauncher for UnsupportedLauncher {
    fn supported(&self) -> bool {
        false
    }
    fn launch(&self) -> Result<Box<dyn Transport>, String> {
        Err("The administrator helper is only available on Windows.".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::optimizer::privileged::{serve, testing::FakeScm};
    use crate::optimizer::service_graph::testing::svc;
    use std::net::{TcpListener, TcpStream};
    use std::sync::Arc;

    /// Runs the real `serve` loop against a fake SCM over a real socket.
    struct LoopbackLauncher {
        scm: Arc<FakeScm>,
        trusted: bool,
        launches: Mutex<u32>,
    }

    impl HelperLauncher for LoopbackLauncher {
        fn supported(&self) -> bool {
            true
        }
        fn location_trusted(&self) -> bool {
            self.trusted
        }
        fn launch(&self) -> Result<Box<dyn Transport>, String> {
            *self.launches.lock().unwrap() += 1;
            let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
            let addr = listener.local_addr().unwrap();
            let scm = self.scm.clone();
            std::thread::spawn(move || {
                if let Ok((mut stream, _)) = listener.accept() {
                    serve(&mut stream, scm.as_ref(), &mut |_| {});
                }
            });
            let stream = TcpStream::connect(addr).map_err(|e| e.to_string())?;
            Ok(Box::new(stream))
        }
    }

    fn manager(trusted: bool) -> (HelperManager, Arc<FakeScm>) {
        let scm = Arc::new(FakeScm::new(vec![
            svc("WinDefend", "Microsoft Defender Antivirus Service", "automatic", "running", &[]),
            svc("DiagTrack", "Connected User Experiences and Telemetry", "automatic", "running", &[]),
        ]));
        (HelperManager::new(Box::new(LoopbackLauncher { scm: scm.clone(), trusted, launches: Mutex::new(0) })), scm)
    }

    #[test]
    fn without_a_helper_writes_ask_for_elevation_and_touch_nothing() {
        let (m, scm) = manager(true);
        let err = m.set_start_type("DiagTrack", StartType::Manual, None).unwrap_err();
        assert!(err.is_elevation());
        assert!(scm.writes().is_empty());
    }

    #[test]
    fn full_stack_change_goes_through_framing_helper_policy_and_the_scm() {
        let (m, scm) = manager(true);
        m.connect(false).unwrap();
        assert!(m.is_connected());
        assert!(m.ping());
        m.set_start_type("DiagTrack", StartType::Manual, Some(StartType::Automatic)).unwrap();
        assert_eq!(scm.start_type("DiagTrack"), "manual");
    }

    #[test]
    fn the_helper_enforces_protection_even_if_the_app_side_is_bypassed() {
        let (m, scm) = manager(true);
        m.connect(false).unwrap();
        let err = m.set_start_type("WinDefend", StartType::Disabled, None).unwrap_err();
        assert!(matches!(err, BackendError::Rejected(ref r) if r.starts_with("protected")), "{err:?}");
        assert!(err.nothing_changed());
        assert!(scm.writes().is_empty());
        assert!(m.is_connected(), "a policy refusal must not drop the session");
    }

    #[test]
    fn stale_review_is_reported_as_a_conflict() {
        let (m, scm) = manager(true);
        m.connect(false).unwrap();
        let err = m.set_start_type("DiagTrack", StartType::Manual, Some(StartType::Disabled)).unwrap_err();
        assert!(matches!(err, BackendError::Conflict(_)), "{err:?}");
        assert!(scm.writes().is_empty());
    }

    #[test]
    fn helper_exit_is_detected_and_requires_re_elevation() {
        let (m, scm) = manager(true);
        m.connect(false).unwrap();
        m.disconnect();
        assert!(!m.is_connected());
        let err = m.set_start_type("DiagTrack", StartType::Manual, None).unwrap_err();
        assert!(err.is_elevation());
        assert!(scm.writes().is_empty());
    }

    #[test]
    fn an_untrusted_install_location_needs_explicit_confirmation() {
        let (m, _) = manager(false);
        assert!(!m.status().location_trusted);
        let err = m.connect(false).unwrap_err();
        assert!(err.contains("folder that your user account can modify"));
        assert!(!m.is_connected());
        m.connect(true).unwrap();
        assert!(m.is_connected());
    }

    #[test]
    fn unsupported_platforms_report_it_plainly() {
        let m = HelperManager::new(Box::new(UnsupportedLauncher));
        assert!(!m.status().supported);
        assert!(m.connect(true).unwrap_err().contains("only available on Windows"));
    }
}
