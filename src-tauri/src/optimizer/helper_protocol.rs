//! Wire protocol between the unprivileged WinClean app and its elevated
//! helper.
//!
//! Design rules:
//! * The surface is deliberately tiny: `hello`, `ping`, `setStartType`,
//!   `shutdown`. There is no "run command", no path, no registry, no file
//!   operation — nothing a caller could stretch into code execution.
//! * Every message is a length-prefixed JSON object, at most
//!   [`MAX_FRAME_BYTES`], parsed with `deny_unknown_fields`.
//! * The helper never trusts the caller's view of a service: it re-reads the
//!   SCM and applies the full protection policy itself (see `privileged`).

use std::io::{self, Read, Write};

use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u32 = 1;

/// Requests are tiny; anything bigger is an attack or a bug.
pub const MAX_FRAME_BYTES: usize = 16 * 1024;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase", deny_unknown_fields)]
pub enum Request {
    /// Must be the first frame of every connection.
    #[serde(rename_all = "camelCase")]
    Hello { version: u32 },
    /// Struct-shaped on purpose: serde ignores extra fields on *unit* variants
    /// of internally tagged enums, but denies them on struct variants.
    Ping {},
    /// Changes one service's start type. `expect_from` is the start type the
    /// caller reviewed; a mismatch aborts the change (state moved underneath).
    #[serde(rename_all = "camelCase")]
    SetStartType { name: String, start_type: String, expect_from: Option<String> },
    Shutdown {},
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum Response {
    #[serde(rename_all = "camelCase")]
    Ok { detail: Option<String>, before: Option<String>, after: Option<String> },
    #[serde(rename_all = "camelCase")]
    Err { code: ErrorCode, message: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ErrorCode {
    /// Malformed request, wrong protocol version, or hello missing.
    BadRequest,
    /// Policy refused the change (protected, unknown, dependents…).
    Rejected,
    /// The service is not in the state the caller reviewed.
    Conflict,
    /// The SCM call failed.
    Failed,
    /// A change was written but could not be verified by re-reading.
    VerifyFailed,
}

impl Response {
    pub fn ok(detail: impl Into<String>) -> Self {
        Response::Ok { detail: Some(detail.into()), before: None, after: None }
    }
    pub fn err(code: ErrorCode, message: impl Into<String>) -> Self {
        Response::Err { code, message: message.into() }
    }
}

#[derive(Debug)]
pub enum FrameError {
    Io(io::Error),
    TooLarge(usize),
    Json(String),
    Closed,
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FrameError::Io(e) => write!(f, "I/O error: {e}"),
            FrameError::TooLarge(n) => write!(f, "frame of {n} bytes exceeds the {MAX_FRAME_BYTES}-byte limit"),
            FrameError::Json(e) => write!(f, "invalid message: {e}"),
            FrameError::Closed => write!(f, "connection closed"),
        }
    }
}

/// Writes one length-prefixed JSON frame.
pub fn write_frame<W: Write, T: Serialize>(w: &mut W, value: &T) -> Result<(), FrameError> {
    let body = serde_json::to_vec(value).map_err(|e| FrameError::Json(e.to_string()))?;
    if body.len() > MAX_FRAME_BYTES {
        return Err(FrameError::TooLarge(body.len()));
    }
    w.write_all(&(body.len() as u32).to_le_bytes()).map_err(FrameError::Io)?;
    w.write_all(&body).map_err(FrameError::Io)?;
    w.flush().map_err(FrameError::Io)
}

/// Reads one frame. The length is validated *before* any allocation so a
/// hostile length prefix cannot make the helper allocate gigabytes.
pub fn read_frame<R: Read, T: for<'de> Deserialize<'de>>(r: &mut R) -> Result<T, FrameError> {
    let mut len_bytes = [0u8; 4];
    match r.read_exact(&mut len_bytes) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Err(FrameError::Closed),
        Err(e) => return Err(FrameError::Io(e)),
    }
    let len = u32::from_le_bytes(len_bytes) as usize;
    if len == 0 || len > MAX_FRAME_BYTES {
        return Err(FrameError::TooLarge(len));
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body).map_err(|e| if e.kind() == io::ErrorKind::UnexpectedEof { FrameError::Closed } else { FrameError::Io(e) })?;
    serde_json::from_slice(&body).map_err(|e| FrameError::Json(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip() {
        let mut buf = Vec::new();
        let req = Request::SetStartType { name: "DiagTrack".into(), start_type: "manual".into(), expect_from: Some("automatic".into()) };
        write_frame(&mut buf, &req).unwrap();
        let back: Request = read_frame(&mut buf.as_slice()).unwrap();
        assert_eq!(back, req);
    }

    #[test]
    fn oversized_and_zero_length_prefixes_are_rejected_before_allocating() {
        let huge = (u32::MAX).to_le_bytes();
        match read_frame::<_, Request>(&mut huge.as_slice()) {
            Err(FrameError::TooLarge(n)) => assert_eq!(n, u32::MAX as usize),
            other => panic!("{other:?}"),
        }
        let zero = 0u32.to_le_bytes();
        assert!(matches!(read_frame::<_, Request>(&mut zero.as_slice()), Err(FrameError::TooLarge(0))));
    }

    #[test]
    fn truncated_frames_report_closed_not_panic() {
        let mut buf = Vec::new();
        write_frame(&mut buf, &Request::Ping {}).unwrap();
        buf.truncate(buf.len() - 1);
        assert!(matches!(read_frame::<_, Request>(&mut buf.as_slice()), Err(FrameError::Closed)));
        assert!(matches!(read_frame::<_, Request>(&mut [].as_slice()), Err(FrameError::Closed)));
    }

    #[test]
    fn unknown_ops_and_extra_fields_are_rejected() {
        for body in [
            r#"{"op":"runCommand","cmd":"cmd.exe"}"#,
            r#"{"op":"setStartType","name":"a","startType":"manual","expectFrom":null,"binPath":"evil.exe"}"#,
            r#"{"op":"ping","extra":1}"#,
            r#"{"name":"a"}"#,
        ] {
            let mut frame = (body.len() as u32).to_le_bytes().to_vec();
            frame.extend_from_slice(body.as_bytes());
            assert!(matches!(read_frame::<_, Request>(&mut frame.as_slice()), Err(FrameError::Json(_))), "{body}");
        }
    }
}
