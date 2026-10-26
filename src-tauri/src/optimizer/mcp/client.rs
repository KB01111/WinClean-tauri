//! A small, strict MCP client (JSON-RPC 2.0 over a [`McpTransport`]).
//!
//! Beyond the happy path it handles what a real host has to: requests the
//! *server* sends (`ping`, everything else answered "method not found"),
//! notifications, stray non-protocol lines, protocol-version negotiation,
//! paginated `tools/list`, cancellation and timeouts (with
//! `notifications/cancelled`), bounded results, and `structuredContent` as well
//! as text content. Server messages are untrusted input throughout.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::policy::{classify_tool, ApprovedCall};
use super::registry::{clean, ToolDecl};
use super::schema::InputSchema;
use super::transport::{McpTransport, Recv};

const SUPPORTED_VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];
const PREFERRED_VERSION: &str = "2025-06-18";
const MAX_TOOLS: usize = 200;
const MAX_PAGES: usize = 5;
const MAX_JUNK_LINES: u32 = 100;
const MAX_RESULT_BYTES: usize = 1024 * 1024;
const POLL_SLICE: Duration = Duration::from_millis(100);

#[derive(Clone, Debug, PartialEq)]
pub enum McpError {
    Cancelled,
    Timeout(String),
    Closed,
    Protocol(String),
    Server { code: i64, message: String },
    /// Windows (or the connector) refused access: consent not granted, location not allowed, policy.
    Permission(String),
    Transport(String),
}

impl std::fmt::Display for McpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            McpError::Cancelled => write!(f, "the operation was cancelled"),
            McpError::Timeout(m) => write!(f, "{m}"),
            McpError::Closed => write!(f, "the server closed the connection"),
            McpError::Protocol(m) => write!(f, "protocol error: {m}"),
            McpError::Server { message, .. } => write!(f, "{message}"),
            McpError::Permission(m) => write!(f, "permission not granted: {m}"),
            McpError::Transport(m) => write!(f, "{m}"),
        }
    }
}

/// Does an error message from a server look like a refusal of access?
pub fn looks_like_permission_failure(text: &str) -> bool {
    let t = text.to_lowercase();
    ["access is denied", "access denied", "permission", "consent", "not allowed", "unauthorized", "unauthorised", "forbidden", "blocked by", "not authorized", "not authorised"].iter().any(|k| t.contains(k))
}

/// A hint that the agent user has not been provisioned yet.
pub fn looks_like_agent_user_missing(text: &str) -> bool {
    let t = text.to_lowercase();
    t.contains("agent user") || t.contains("provision") || t.contains("agent account")
}

#[derive(Clone, Debug, PartialEq)]
pub struct Handshake {
    pub protocol_version: String,
    pub server_name: Option<String>,
    pub server_version: Option<String>,
    pub supports_tools: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LiveTools {
    pub tools: Vec<ToolDecl>,
    pub notes: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ToolOutcome {
    /// `structuredContent` when the server sent it.
    pub structured: Option<Value>,
    /// Text parts of `content`.
    pub texts: Vec<String>,
}

pub struct McpClient<T: McpTransport> {
    transport: T,
    next_id: u64,
    timeout: Duration,
    cancel: Arc<AtomicBool>,
    junk: u32,
}

impl<T: McpTransport> McpClient<T> {
    pub fn new(transport: T, timeout: Duration, cancel: Arc<AtomicBool>) -> Self {
        Self { transport, next_id: 1, timeout, cancel, junk: 0 }
    }

    fn send(&mut self, message: &Value) -> Result<(), McpError> {
        self.transport.send_line(&message.to_string()).map_err(McpError::Transport)
    }

    fn notify_cancelled(&mut self, id: u64, reason: &str) {
        let _ = self.send(&json!({ "jsonrpc": "2.0", "method": "notifications/cancelled", "params": { "requestId": id, "reason": reason } }));
    }

    /// Sends a request and waits for the response with the same id.
    fn request(&mut self, method: &str, params: Value) -> Result<Value, McpError> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))?;
        let deadline = Instant::now() + self.timeout;
        loop {
            if self.cancel.load(Ordering::Relaxed) {
                self.notify_cancelled(id, "cancelled by the user");
                return Err(McpError::Cancelled);
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                self.notify_cancelled(id, "timed out");
                return Err(McpError::Timeout(format!("the server did not answer '{method}' within {} s", self.timeout.as_secs())));
            }
            let line = match self.transport.recv_line(remaining.min(POLL_SLICE)) {
                Recv::Line(l) => l,
                Recv::Timeout => continue,
                Recv::Closed => return Err(McpError::Closed),
                Recv::TooLong => return Err(McpError::Protocol("the server sent a message that is too large".into())),
            };
            let Ok(Value::Object(msg)) = serde_json::from_str::<Value>(&line) else {
                self.junk += 1;
                if self.junk > MAX_JUNK_LINES {
                    return Err(McpError::Protocol("the server keeps printing text that is not JSON-RPC".into()));
                }
                continue;
            };
            if let Some(m) = msg.get("method").and_then(|m| m.as_str()) {
                // A request or notification from the server.
                if let Some(rid) = msg.get("id").cloned() {
                    let reply = if m == "ping" {
                        json!({ "jsonrpc": "2.0", "id": rid, "result": {} })
                    } else {
                        json!({ "jsonrpc": "2.0", "id": rid, "error": { "code": -32601, "message": "WinClean does not support this request" } })
                    };
                    self.send(&reply)?;
                }
                continue;
            }
            if msg.get("id").and_then(|v| v.as_u64()) != Some(id) {
                continue; // a late answer to an earlier, abandoned request
            }
            if let Some(err) = msg.get("error") {
                let message = clean(err.get("message").and_then(|m| m.as_str()).unwrap_or("server error"), 300);
                let code = err.get("code").and_then(|c| c.as_i64()).unwrap_or(0);
                return Err(if looks_like_permission_failure(&message) { McpError::Permission(message) } else { McpError::Server { code, message } });
            }
            let result = msg.get("result").cloned().unwrap_or(Value::Null);
            if result.to_string().len() > MAX_RESULT_BYTES {
                return Err(McpError::Protocol("the server's answer is larger than WinClean accepts".into()));
            }
            return Ok(result);
        }
    }

    pub fn initialize(&mut self) -> Result<Handshake, McpError> {
        let result = self.request("initialize", json!({ "protocolVersion": PREFERRED_VERSION, "capabilities": {}, "clientInfo": { "name": "WinClean", "version": env!("CARGO_PKG_VERSION") } }))?;
        let version = result.get("protocolVersion").and_then(|v| v.as_str()).ok_or_else(|| McpError::Protocol("the server's initialize answer has no protocolVersion".into()))?;
        if !SUPPORTED_VERSIONS.contains(&version) {
            return Err(McpError::Protocol(format!("the server speaks protocol version '{}', which WinClean does not support", clean(version, 32))));
        }
        let handshake = Handshake {
            protocol_version: version.to_string(),
            server_name: result.pointer("/serverInfo/name").and_then(|n| n.as_str()).map(|n| clean(n, 80)),
            server_version: result.pointer("/serverInfo/version").and_then(|n| n.as_str()).map(|n| clean(n, 40)),
            supports_tools: result.pointer("/capabilities/tools").map(|t| t.is_object()).unwrap_or(false),
        };
        self.send(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }))?;
        Ok(handshake)
    }

    /// Lists the server's tools with their real input schemas (all pages).
    pub fn list_tools(&mut self) -> Result<LiveTools, McpError> {
        let mut tools = Vec::new();
        let mut notes = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..MAX_PAGES {
            let params = match &cursor {
                Some(c) => json!({ "cursor": c }),
                None => json!({}),
            };
            let result = self.request("tools/list", params)?;
            let page = result.get("tools").and_then(|t| t.as_array()).ok_or_else(|| McpError::Protocol("tools/list returned no tool array".into()))?;
            for t in page {
                if tools.len() >= MAX_TOOLS {
                    notes.push(format!("Only the first {MAX_TOOLS} tools are listed."));
                    return Ok(LiveTools { tools, notes });
                }
                let Some(name) = t.get("name").and_then(|n| n.as_str()) else { continue };
                if name.is_empty() || name.len() > 64 || !name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.')) {
                    notes.push("A tool with an invalid name was ignored.".into());
                    continue;
                }
                tools.push(ToolDecl {
                    name: name.to_string(),
                    description: t.get("description").and_then(|d| d.as_str()).map(|d| clean(d, 400)),
                    policy: classify_tool(name),
                    input_schema: t.get("inputSchema").map(InputSchema::from_value).unwrap_or_default(),
                });
            }
            match result.get("nextCursor").and_then(|c| c.as_str()) {
                Some(c) if !c.is_empty() && c.len() < 512 => cursor = Some(c.to_string()),
                _ => return Ok(LiveTools { tools, notes }),
            }
        }
        notes.push("The tool list has more pages than WinClean reads.".into());
        Ok(LiveTools { tools, notes })
    }

    /// Sends a call that passed the policy. A refusal of access is reported as
    /// `Permission`, never retried, never worked around.
    pub fn call(&mut self, call: &ApprovedCall) -> Result<ToolOutcome, McpError> {
        let result = self.request("tools/call", json!({ "name": call.tool(), "arguments": call.args() }))?;
        let texts: Vec<String> = result
            .get("content")
            .and_then(|c| c.as_array())
            .map(|parts| parts.iter().filter(|p| p.get("type").and_then(|t| t.as_str()) == Some("text")).filter_map(|p| p.get("text").and_then(|t| t.as_str())).map(|t| t.chars().take(MAX_RESULT_BYTES).collect()).collect())
            .unwrap_or_default();
        if result.get("isError").and_then(|e| e.as_bool()) == Some(true) {
            let message = clean(&texts.join(" "), 300);
            let message = if message.is_empty() { "the tool reported an error".to_string() } else { message };
            return Err(if looks_like_permission_failure(&message) { McpError::Permission(message) } else { McpError::Server { code: 0, message } });
        }
        Ok(ToolOutcome { structured: result.get("structuredContent").filter(|s| !s.is_null()).cloned(), texts })
    }

    /// Closes the connection and everything behind it.
    pub fn close(&mut self) {
        self.transport.close();
    }
}

impl<T: McpTransport> Drop for McpClient<T> {
    fn drop(&mut self) {
        self.transport.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::optimizer::mcp::policy::approve_metadata_call;
    use crate::optimizer::mcp::transport::scripted::ScriptedTransport;
    use std::sync::Mutex;

    fn reply(id: &Value, result: Value) -> String {
        json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string()
    }

    fn client(respond: impl FnMut(&Value) -> Vec<String> + Send + 'static, timeout_ms: u64) -> (McpClient<ScriptedTransport>, Arc<Mutex<Vec<Value>>>, Arc<AtomicBool>) {
        let t = ScriptedTransport::new(respond);
        let sent = t.sent.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        (McpClient::new(t, Duration::from_millis(timeout_ms), cancel.clone()), sent, cancel)
    }

    fn call() -> ApprovedCall {
        let schema = InputSchema::from_value(&json!({ "type": "object", "properties": { "path": { "type": "string" } } }));
        approve_metadata_call("get_file_details", "C:\\Users\\me\\Downloads\\a.iso", &["C:\\Users\\me\\Downloads".to_string()], &schema).unwrap()
    }

    #[test]
    fn the_handshake_negotiates_the_protocol_and_confirms_with_initialized() {
        let (mut c, sent, _) = client(|m| match m["method"].as_str() {
            Some("initialize") => vec![reply(&m["id"], json!({ "protocolVersion": "2025-03-26", "serverInfo": { "name": "FileExplorer", "version": "9" }, "capabilities": { "tools": { "listChanged": true } } }))],
            _ => vec![],
        }, 2000);
        let h = c.initialize().unwrap();
        assert_eq!((h.protocol_version.as_str(), h.server_name.as_deref(), h.supports_tools), ("2025-03-26", Some("FileExplorer"), true));
        let sent = sent.lock().unwrap();
        assert_eq!(sent[0]["params"]["protocolVersion"], "2025-06-18");
        assert_eq!(sent[0]["params"]["clientInfo"]["name"], "WinClean");
        assert_eq!(sent[1]["method"], "notifications/initialized");
        assert!(sent[1].get("id").is_none(), "a notification has no id");
    }

    #[test]
    fn unsupported_or_missing_protocol_versions_are_refused() {
        let (mut c, _, _) = client(|m| vec![reply(&m["id"], json!({ "protocolVersion": "1999-01-01" }))], 2000);
        assert!(matches!(c.initialize().unwrap_err(), McpError::Protocol(m) if m.contains("does not support")));
        let (mut c, _, _) = client(|m| vec![reply(&m["id"], json!({ "serverInfo": {} }))], 2000);
        assert!(matches!(c.initialize().unwrap_err(), McpError::Protocol(m) if m.contains("no protocolVersion")));
    }

    #[test]
    fn requests_from_the_server_are_answered_and_noise_is_ignored() {
        let (mut c, sent, _) = client(|m| {
            if m["method"] == "tools/list" {
                vec![
                    "starting up…".into(),
                    json!({ "jsonrpc": "2.0", "method": "notifications/message", "params": {} }).to_string(),
                    json!({ "jsonrpc": "2.0", "id": "srv-1", "method": "ping" }).to_string(),
                    json!({ "jsonrpc": "2.0", "id": "srv-2", "method": "sampling/createMessage" }).to_string(),
                    reply(&json!(999), json!({ "late": true })),
                    reply(&m["id"], json!({ "tools": [] })),
                ]
            } else {
                vec![]
            }
        }, 2000);
        assert_eq!(c.list_tools().unwrap().tools.len(), 0);
        let sent = sent.lock().unwrap();
        assert!(sent.iter().any(|m| m["id"] == "srv-1" && m["result"] == json!({})), "ping must be answered");
        assert!(sent.iter().any(|m| m["id"] == "srv-2" && m["error"]["code"] == -32601), "unsupported server requests are refused politely");
    }

    #[test]
    fn a_server_that_spams_non_protocol_text_is_dropped() {
        let (mut c, _, _) = client(|_| (0..150).map(|i| format!("log line {i}")).collect(), 2000);
        assert!(matches!(c.list_tools().unwrap_err(), McpError::Protocol(m) if m.contains("not JSON-RPC")));
    }

    #[test]
    fn tool_listing_reads_real_schemas_across_pages_and_skips_bad_names() {
        let pages = Mutex::new(0);
        let (mut c, sent, _) = client(move |m| {
            let mut p = pages.lock().unwrap();
            *p += 1;
            if *p == 1 {
                vec![reply(&m["id"], json!({ "tools": [
                    { "name": "search_files", "description": "Search", "inputSchema": { "type": "object", "properties": { "startingDir": { "type": "string" } } } },
                    { "name": "bad name!" }, { "name": "move_file" } ], "nextCursor": "p2" }))]
            } else {
                vec![reply(&m["id"], json!({ "tools": [ { "name": "get_file_details" } ] }))]
            }
        }, 2000);
        let l = c.list_tools().unwrap();
        let names: Vec<_> = l.tools.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["search_files", "move_file", "get_file_details"]);
        assert!(l.tools[0].input_schema.has("startingDir") && !l.tools[2].input_schema.declared);
        assert!(l.notes.iter().any(|n| n.contains("invalid name")));
        assert_eq!(sent.lock().unwrap()[1]["params"]["cursor"], "p2");
    }

    #[test]
    fn runaway_pagination_and_oversized_lists_are_bounded() {
        let (mut c, sent, _) = client(|m| vec![reply(&m["id"], json!({ "tools": [ { "name": "t" } ], "nextCursor": "again" }))], 2000);
        let l = c.list_tools().unwrap();
        assert_eq!(l.tools.len(), MAX_PAGES);
        assert_eq!(sent.lock().unwrap().len(), MAX_PAGES);
        let many: Vec<Value> = (0..400).map(|i| json!({ "name": format!("t{i}") })).collect();
        let (mut c, _, _) = client(move |m| vec![reply(&m["id"], json!({ "tools": many.clone() }))], 2000);
        assert_eq!(c.list_tools().unwrap().tools.len(), 200);
    }

    #[test]
    fn structured_and_text_results_are_both_understood() {
        let (mut c, sent, _) = client(|m| vec![reply(&m["id"], json!({ "structuredContent": { "extension": ".iso", "size": 7 }, "content": [ { "type": "text", "text": "{\"size\":7}" }, { "type": "image", "data": "x" } ] }))], 2000);
        let o = c.call(&call()).unwrap();
        assert_eq!(o.structured.unwrap()["size"], 7);
        assert_eq!(o.texts, vec!["{\"size\":7}"]);
        assert_eq!(sent.lock().unwrap()[0]["params"]["name"], "get_file_details");
        let (mut c, _, _) = client(|m| vec![reply(&m["id"], json!({ "content": [ { "type": "text", "text": "plain" } ] }))], 2000);
        let o = c.call(&call()).unwrap();
        assert!(o.structured.is_none() && o.texts == vec!["plain"]);
    }

    #[test]
    fn refusals_of_access_are_permission_failures_other_errors_are_not() {
        let (mut c, _, _) = client(|m| vec![reply(&m["id"], json!({ "isError": true, "content": [ { "type": "text", "text": "Access is denied. The user has not granted consent." } ] }))], 2000);
        assert!(matches!(c.call(&call()).unwrap_err(), McpError::Permission(m) if m.contains("denied")));
        let (mut c, _, _) = client(|m| vec![json!({ "jsonrpc": "2.0", "id": m["id"], "error": { "code": -32000, "message": "Consent required for this host" } }).to_string()], 2000);
        assert!(matches!(c.call(&call()).unwrap_err(), McpError::Permission(_)));
        let (mut c, _, _) = client(|m| vec![reply(&m["id"], json!({ "isError": true, "content": [ { "type": "text", "text": "path does not exist" } ] }))], 2000);
        assert!(matches!(c.call(&call()).unwrap_err(), McpError::Server { .. }));
        assert!(looks_like_agent_user_missing("The agent user must be provisioned first"));
        assert!(!looks_like_agent_user_missing("path does not exist"));
    }

    #[test]
    fn a_silent_server_times_out_and_is_told_the_request_was_cancelled() {
        let (mut c, sent, _) = client(|_| vec![], 250);
        let started = Instant::now();
        let e = c.list_tools().unwrap_err();
        assert!(matches!(e, McpError::Timeout(_)));
        assert!(started.elapsed() < Duration::from_secs(5));
        assert_eq!(sent.lock().unwrap().last().unwrap()["method"], "notifications/cancelled");
    }

    #[test]
    fn the_cancel_flag_stops_a_wait_promptly() {
        let (mut c, sent, cancel) = client(|_| vec![], 30_000);
        let flag = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            flag.store(true, Ordering::Relaxed);
        });
        let started = Instant::now();
        assert_eq!(c.list_tools().unwrap_err(), McpError::Cancelled);
        assert!(started.elapsed() < Duration::from_secs(3));
        assert_eq!(sent.lock().unwrap().last().unwrap()["method"], "notifications/cancelled");
    }

    #[test]
    fn closed_connections_and_oversized_messages_are_protocol_level_errors() {
        let (mut c, _, _) = client(|_| vec!["<CLOSED>".into()], 2000);
        assert_eq!(c.list_tools().unwrap_err(), McpError::Closed);
        let (mut c, _, _) = client(|_| vec!["<TOO LONG>".into()], 2000);
        assert!(matches!(c.list_tools().unwrap_err(), McpError::Protocol(_)));
        let huge = "x".repeat(MAX_RESULT_BYTES + 10);
        let (mut c, _, _) = client(move |m| vec![reply(&m["id"], json!({ "content": [ { "type": "text", "text": huge } ] }))], 2000);
        assert!(matches!(c.call(&call()).unwrap_err(), McpError::Protocol(m) if m.contains("larger")));
    }

    #[test]
    fn closing_the_client_closes_the_transport_even_on_drop() {
        let t = ScriptedTransport::new(|_| vec![]);
        let closed = t.closed.clone();
        drop(McpClient::new(t, Duration::from_secs(1), Arc::new(AtomicBool::new(false))));
        assert!(closed.load(Ordering::SeqCst));
    }
}
