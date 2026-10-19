//! The Windows MCP manager: discovery, verification, consented search.
//!
//! Servers only run while an operation needs them. Every operation opens a
//! contained session, does its work, and closes it (killing the whole process
//! tree) before returning, so nothing stays resident and nothing is leaked.
//! Discovery results are cached briefly so the UI cannot cause repeated
//! `odr.exe` launches.

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::Value;

use super::client::{looks_like_agent_user_missing, McpClient, McpError};
use super::odr::{self, FileProbe, Listing, ListAttempt, OdrEnv, OdrRunner, ResolvedOdr, TrustedLocation, MIN_ODR_BUILD};
use super::policy::{approve_metadata_call, approve_search_call, classify_tool, policy_note, SearchPlan, SearchRequest, ToolPolicy};
use super::registry::{ConnectionMethod, RegisteredServer, RegistryFormat, ServerIdentity, ToolDecl};
use super::schema::InputSchema;
use super::search::{parse_directories, parse_hits, SearchHit};
use super::transport::{McpTransport, StdioTransport};

const LISTING_TTL: Duration = Duration::from_secs(30);
const SESSION_TIMEOUT: Duration = Duration::from_secs(25);

impl McpTransport for Box<dyn McpTransport> {
    fn send_line(&mut self, line: &str) -> Result<(), String> {
        (**self).send_line(line)
    }
    fn recv_line(&mut self, wait: Duration) -> super::transport::Recv {
        (**self).recv_line(wait)
    }
    fn close(&mut self) {
        (**self).close()
    }
}

/// How a session to a registered server is opened (injectable for tests).
pub trait Launcher: Send + Sync {
    fn connect(&self, odr_path: &str, args: &[String]) -> Result<Box<dyn McpTransport>, String>;
}

pub struct StdioLauncher;
impl Launcher for StdioLauncher {
    fn connect(&self, odr_path: &str, args: &[String]) -> Result<Box<dyn McpTransport>, String> {
        Ok(Box::new(StdioTransport::spawn(odr_path, args, &[])?))
    }
}

// ---------------------------------------------------------------------------
// Status model (what the UI shows)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolView {
    pub name: String,
    pub description: Option<String>,
    pub policy: ToolPolicy,
    pub policy_note: &'static str,
    pub arguments: Vec<String>,
    pub required: Vec<String>,
}

fn tool_view(t: &ToolDecl) -> ToolView {
    ToolView {
        name: t.name.clone(),
        description: t.description.clone(),
        policy: t.policy,
        policy_note: policy_note(t.policy),
        arguments: t.input_schema.property_names(),
        required: t.input_schema.required.clone(),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ConnectionState {
    NotAttempted,
    Connected,
    AgentUserNeeded,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PermissionState {
    /// Nothing has asked Windows yet.
    Unknown,
    Granted,
    Denied,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SemanticSupport {
    NotAdvertised,
    /// The connector mentions semantic search, which Microsoft documents for Copilot+ PCs only.
    AdvertisedNeedsCopilotPlus,
    Available,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchCapabilities {
    pub available: bool,
    /// `declared` (registry) or `live` (read from the running connector).
    pub source: &'static str,
    pub arguments: Vec<String>,
    pub file_types: bool,
    pub start_date: bool,
    pub end_date: bool,
    pub semantic: SemanticSupport,
    pub fallback: &'static str,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileExplorerStatus {
    pub server_id: String,
    pub name: String,
    pub identity: ServerIdentity,
    pub connection_method: ConnectionMethod,
    pub connection: ConnectionState,
    pub connection_detail: Option<String>,
    pub protocol_version: Option<String>,
    pub declared_tools: Vec<ToolView>,
    pub live_tools: Option<Vec<ToolView>>,
    /// Do the live tools match what the registry declared?
    pub tools_match_declaration: Option<bool>,
    pub search: SearchCapabilities,
    pub permission: PermissionState,
    pub permission_detail: Option<String>,
    /// A tool call actually succeeded (not merely: the process started).
    pub verified_execution: bool,
    pub last_verified_ms: Option<i64>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OdrStatus {
    pub found: bool,
    pub path: Option<String>,
    pub trusted_location: Option<TrustedLocation>,
    pub windows_build: Option<u32>,
    pub build_supported: Option<bool>,
    pub registry_format: Option<RegistryFormat>,
    pub attempts: Vec<ListAttempt>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpStatus {
    pub odr: OdrStatus,
    pub servers: Vec<RegisteredServer>,
    pub file_explorer: Option<FileExplorerStatus>,
    pub notes: Vec<String>,
    /// What the integration will do when it is unavailable.
    pub fallback: &'static str,
}

pub const FALLBACK_TEXT: &str = "WinClean's own scanner and index always work without Windows MCP. When the connector is unavailable, file search uses the native index (file names and metadata only).";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum FailureKind {
    Unavailable,
    IdentityNotVerified,
    AgentUserNeeded,
    PermissionDenied,
    NotAuthorised,
    Cancelled,
    Failed,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpFailure {
    pub kind: FailureKind,
    pub message: String,
}

impl McpFailure {
    fn new(kind: FailureKind, message: impl Into<String>) -> Self {
        Self { kind, message: message.into() }
    }
}

impl From<McpError> for McpFailure {
    fn from(e: McpError) -> Self {
        let text = e.to_string();
        let kind = match &e {
            McpError::Cancelled => FailureKind::Cancelled,
            McpError::Permission(_) => FailureKind::PermissionDenied,
            _ if looks_like_agent_user_missing(&text) => FailureKind::AgentUserNeeded,
            _ => FailureKind::Failed,
        };
        McpFailure::new(kind, text)
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchOutcome {
    pub hits: Vec<SearchHit>,
    pub rejected: usize,
    pub truncated: bool,
    pub plan: SearchPlan,
    pub notes: Vec<String>,
}

// ---------------------------------------------------------------------------
// Manager
// ---------------------------------------------------------------------------

#[derive(Clone, Default)]
struct VerifyRecord {
    connection: Option<(ConnectionState, Option<String>)>,
    protocol_version: Option<String>,
    live_tools: Option<Vec<ToolDecl>>,
    permission: Option<(PermissionState, Option<String>)>,
    verified_execution: bool,
    at_ms: Option<i64>,
}

struct Cached {
    at: Instant,
    odr: Result<ResolvedOdr, String>,
    listing: Option<Listing>,
}

#[derive(Default)]
struct State {
    cached: Option<Cached>,
    verify: VerifyRecord,
}

pub struct McpManager {
    runner: Arc<dyn OdrRunner>,
    launcher: Arc<dyn Launcher>,
    env: OdrEnv,
    files: Arc<dyn FileProbe + Send + Sync>,
    state: Mutex<State>,
    now_ms: fn() -> i64,
}

impl McpManager {
    pub fn system() -> Self {
        Self::new(Arc::new(odr::SystemRunner), Arc::new(StdioLauncher), OdrEnv::from_process(), Arc::new(odr::RealFiles), super::super::ecoqos::now_ms)
    }

    pub fn new(runner: Arc<dyn OdrRunner>, launcher: Arc<dyn Launcher>, env: OdrEnv, files: Arc<dyn FileProbe + Send + Sync>, now_ms: fn() -> i64) -> Self {
        Self { runner, launcher, env, files, state: Mutex::new(State::default()), now_ms }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Resolves `odr.exe` and lists servers, reusing a recent result unless `force`.
    fn refresh(&self, windows_build: Option<u32>, force: bool, cancel: &Arc<AtomicBool>) -> (Result<ResolvedOdr, String>, Option<Listing>) {
        {
            let st = self.lock();
            if let (false, Some(c)) = (force, &st.cached) {
                if c.at.elapsed() < LISTING_TTL {
                    return (c.odr.clone(), c.listing.clone());
                }
            }
        }
        let resolved = if windows_build.map(|b| b < MIN_ODR_BUILD).unwrap_or(false) {
            Err(format!("Windows build {} is older than {MIN_ODR_BUILD}; the On-device Agent Registry (MCP on Windows preview) needs build {MIN_ODR_BUILD}.7262 or later.", windows_build.unwrap_or(0)))
        } else {
            odr::resolve_odr(&self.env, &*self.files)
        };
        let listing = resolved.as_ref().ok().map(|r| odr::list_servers(&*self.runner, &r.path, cancel));
        self.lock().cached = Some(Cached { at: Instant::now(), odr: resolved.clone(), listing: listing.clone() });
        (resolved, listing)
    }

    /// Current status. Discovery only: nothing is connected or called.
    pub fn status(&self, windows_build: Option<u32>, copilot_plus: bool, force: bool, cancel: &Arc<AtomicBool>) -> McpStatus {
        let (resolved, listing) = self.refresh(windows_build, force, cancel);
        self.build_status(windows_build, copilot_plus, resolved, listing)
    }

    fn build_status(&self, windows_build: Option<u32>, copilot_plus: bool, resolved: Result<ResolvedOdr, String>, listing: Option<Listing>) -> McpStatus {
        let mut notes = Vec::new();
        let build_supported = windows_build.map(|b| b >= MIN_ODR_BUILD);
        let parsed = listing.as_ref().and_then(|l| l.parsed.clone());
        let mut odr_status = OdrStatus {
            found: resolved.is_ok(),
            path: resolved.as_ref().ok().map(|r| r.path.clone()),
            trusted_location: resolved.as_ref().ok().map(|r| r.location),
            windows_build,
            build_supported,
            registry_format: parsed.as_ref().map(|p| p.format),
            attempts: listing.as_ref().map(|l| l.attempts.clone()).unwrap_or_default(),
            error: resolved.as_ref().err().cloned(),
        };
        if let Some(l) = &listing {
            if l.parsed.is_none() {
                odr_status.error = Some("odr.exe was found, but its server listing could not be read. See the attempts below.".into());
            }
        }
        let servers = parsed.as_ref().map(|p| p.servers.clone()).unwrap_or_default();
        if let Some(p) = &parsed {
            notes.extend(p.notes.clone());
            if p.servers.is_empty() {
                notes.push("odr.exe reports no registered MCP servers.".into());
            }
        }
        if servers.iter().any(|s| s.impersonation_warning) {
            notes.push("A registered server is named like File Explorer but is not proven to be Microsoft's connector. WinClean ignores it.".into());
        }
        if servers.iter().any(|s| s.connection == ConnectionMethod::ManifestCommandOnly) {
            notes.push("Some servers can only be started by running a command from their manifest. WinClean never runs commands taken from registry data.".into());
        }
        let fe = servers.iter().find(|s| s.identity == ServerIdentity::MicrosoftFileExplorer).map(|s| self.file_explorer_status(s, copilot_plus));
        if fe.is_none() && parsed.is_some() {
            notes.push("The Microsoft File Explorer connector was not found in the registry.".into());
        }
        McpStatus { odr: odr_status, servers, file_explorer: fe, notes, fallback: FALLBACK_TEXT }
    }

    fn file_explorer_status(&self, s: &RegisteredServer, copilot_plus: bool) -> FileExplorerStatus {
        let v = self.lock().verify.clone();
        let live = v.live_tools.as_ref();
        let tools_for_search = live.unwrap_or(&s.declared_tools);
        let search_tool = tools_for_search.iter().find(|t| t.name == "search_files");
        let semantic_text = search_tool.and_then(|t| t.description.as_ref()).map(|d| d.to_lowercase()).map(|d| d.contains("semantic") || d.contains("natural language")).unwrap_or(false);
        let search = SearchCapabilities {
            available: search_tool.is_some(),
            source: if live.is_some() { "live" } else { "declared" },
            arguments: search_tool.map(|t| t.input_schema.property_names()).unwrap_or_default(),
            file_types: search_tool.map(|t| t.input_schema.has("specifiedFileTypes")).unwrap_or(false),
            start_date: search_tool.map(|t| t.input_schema.has("startDate")).unwrap_or(false),
            end_date: search_tool.map(|t| t.input_schema.has("endDate")).unwrap_or(false),
            semantic: match (semantic_text, copilot_plus) {
                (false, _) => SemanticSupport::NotAdvertised,
                (true, false) => SemanticSupport::AdvertisedNeedsCopilotPlus,
                (true, true) => SemanticSupport::Available,
            },
            fallback: "Native file-name and metadata search over WinClean's own index.",
        };
        let names = |tools: &[ToolDecl]| {
            let mut n: Vec<_> = tools.iter().map(|t| (t.name.clone(), t.input_schema.property_names())).collect();
            n.sort();
            n
        };
        FileExplorerStatus {
            server_id: s.id.clone(),
            name: s.name.clone(),
            identity: s.identity,
            connection_method: s.connection,
            connection: v.connection.as_ref().map(|c| c.0).unwrap_or(ConnectionState::NotAttempted),
            connection_detail: v.connection.as_ref().and_then(|c| c.1.clone()),
            protocol_version: v.protocol_version.clone(),
            declared_tools: s.declared_tools.iter().map(tool_view).collect(),
            live_tools: live.map(|t| t.iter().map(tool_view).collect()),
            tools_match_declaration: live.filter(|_| !s.declared_tools.is_empty()).map(|l| names(l) == names(&s.declared_tools)),
            search,
            permission: v.permission.as_ref().map(|p| p.0).unwrap_or(PermissionState::Unknown),
            permission_detail: v.permission.as_ref().and_then(|p| p.1.clone()),
            verified_execution: v.verified_execution,
            last_verified_ms: v.at_ms,
        }
    }

    /// The verified Microsoft File Explorer connector from the latest listing.
    fn file_explorer(&self, windows_build: Option<u32>, cancel: &Arc<AtomicBool>) -> Result<(ResolvedOdr, RegisteredServer), McpFailure> {
        let (resolved, listing) = self.refresh(windows_build, false, cancel);
        let odr = resolved.map_err(|e| McpFailure::new(FailureKind::Unavailable, e))?;
        let servers = listing.and_then(|l| l.parsed).map(|p| p.servers).unwrap_or_default();
        let server = servers
            .into_iter()
            .find(|s| s.identity == ServerIdentity::MicrosoftFileExplorer)
            .ok_or_else(|| McpFailure::new(FailureKind::IdentityNotVerified, "WinClean found no registry entry that is verifiably Microsoft's File Explorer connector, so it will not connect to any server."))?;
        if server.connection != ConnectionMethod::OdrProxy {
            return Err(McpFailure::new(FailureKind::Unavailable, "The registry entry offers no package identifier to connect through odr.exe."));
        }
        Ok((odr, server))
    }

    fn open(&self, windows_build: Option<u32>, cancel: &Arc<AtomicBool>) -> Result<(McpClient<Box<dyn McpTransport>>, RegisteredServer, super::client::Handshake), McpFailure> {
        let (odr, server) = self.file_explorer(windows_build, cancel)?;
        let args = odr::proxy_args(server.identifier.as_deref().unwrap_or("")).map_err(|e| McpFailure::new(FailureKind::Unavailable, e))?;
        let transport = self.launcher.connect(&odr.path, &args).map_err(|e| McpFailure::new(FailureKind::Failed, e))?;
        let mut client = McpClient::new(transport, SESSION_TIMEOUT, cancel.clone());
        match client.initialize() {
            Ok(h) => Ok((client, server, h)),
            Err(e) => {
                client.close();
                Err(e.into())
            }
        }
    }

    fn record(&self, f: impl FnOnce(&mut VerifyRecord)) {
        let mut st = self.lock();
        f(&mut st.verify);
        st.verify.at_ms = Some((self.now_ms)());
    }

    /// Connects, performs the MCP handshake and reads the live tool list with
    /// each tool's real input schema. No tool is called, so Windows has nothing
    /// to ask the user about. Returns the updated status.
    pub fn verify_connection(&self, windows_build: Option<u32>, copilot_plus: bool, cancel: &Arc<AtomicBool>) -> McpStatus {
        self.refresh(windows_build, true, cancel);
        match self.open(windows_build, cancel) {
            Ok((mut client, _server, handshake)) => {
                let tools = client.list_tools();
                client.close();
                match tools {
                    Ok(live) => self.record(|v| {
                        v.connection = Some((ConnectionState::Connected, None));
                        v.protocol_version = Some(handshake.protocol_version.clone());
                        v.live_tools = Some(live.tools);
                    }),
                    Err(e) => self.record(|v| {
                        v.connection = Some((ConnectionState::Failed, Some(format!("connected, but listing tools failed: {e}"))));
                        v.protocol_version = Some(handshake.protocol_version.clone());
                    }),
                }
            }
            Err(f) => self.record(|v| {
                let state = if f.kind == FailureKind::AgentUserNeeded { ConnectionState::AgentUserNeeded } else { ConnectionState::Failed };
                v.connection = Some((state, Some(f.message)));
            }),
        }
        self.status(windows_build, copilot_plus, false, cancel)
    }

    /// `odr.exe mcp provision-agent-user`. Only ever called from an explicit user action.
    pub fn provision_agent_user(&self, windows_build: Option<u32>, cancel: &Arc<AtomicBool>) -> Result<String, McpFailure> {
        let (resolved, _) = self.refresh(windows_build, false, cancel);
        let odr = resolved.map_err(|e| McpFailure::new(FailureKind::Unavailable, e))?;
        odr::provision_agent_user(&*self.runner, &odr.path, cancel).map_err(|e| McpFailure::new(if e.contains("cancelled") { FailureKind::Cancelled } else { FailureKind::Failed }, e))
    }

    fn live_schema(live: &[ToolDecl], name: &str) -> Result<(InputSchema, Option<String>), McpFailure> {
        let tool = live.iter().find(|t| t.name == name).ok_or_else(|| McpFailure::new(FailureKind::Unavailable, format!("the connector does not offer '{name}'")))?;
        if classify_tool(name) == ToolPolicy::Unknown {
            return Err(McpFailure::new(FailureKind::Unavailable, "unrecognised tool"));
        }
        Ok((tool.input_schema.clone(), tool.description.clone()))
    }

    fn denied(&self, f: &McpFailure) {
        if f.kind == FailureKind::PermissionDenied {
            self.record(|v| v.permission = Some((PermissionState::Denied, Some(f.message.clone()))));
        }
    }

    /// Asks the connector to list one authorised folder. This is how WinClean
    /// learns whether Windows has granted access, and it is the proof that a
    /// tool actually executed.
    pub fn verify_permission(&self, windows_build: Option<u32>, copilot_plus: bool, folder: &str, roots: &[String], cancel: &Arc<AtomicBool>) -> Result<McpStatus, McpFailure> {
        let result = (|| {
            let (mut client, _s, _h) = self.open(windows_build, cancel)?;
            let outcome = (|| {
                let live = client.list_tools()?;
                let (schema, _) = Self::live_schema(&live.tools, "get_directories")?;
                let call = approve_metadata_call("get_directories", folder, roots, &schema).map_err(|e| McpFailure::new(FailureKind::NotAuthorised, e))?;
                let out = client.call(&call)?;
                self.record(|v| v.live_tools = Some(live.tools));
                Ok::<_, McpFailure>(parse_directories(&out, folder).len())
            })();
            client.close();
            outcome
        })();
        match &result {
            Ok(n) => self.record(|v| {
                v.permission = Some((PermissionState::Granted, Some(format!("Windows allowed the connector to list the folder ({n} sub-folder(s)).")))); 
                v.verified_execution = true;
                v.connection = Some((ConnectionState::Connected, None));
            }),
            Err(f) => self.denied(f),
        }
        result.map(|_| self.status(windows_build, copilot_plus, false, cancel))
    }

    /// A user-initiated search in an authorised location. The arguments are
    /// built from the connector's live schema; the answer is sanitised.
    pub fn search(&self, windows_build: Option<u32>, copilot_plus: bool, req: &SearchRequest, roots: &[String], include_snippets: bool, limit: usize, cancel: &Arc<AtomicBool>) -> Result<SearchOutcome, McpFailure> {
        let (mut client, _server, _h) = self.open(windows_build, cancel)?;
        let result = (|| {
            let live = client.list_tools()?;
            let (schema, description) = Self::live_schema(&live.tools, "search_files")?;
            let (call, plan) = approve_search_call(req, roots, &schema, description.as_deref(), copilot_plus).map_err(|e| McpFailure::new(FailureKind::NotAuthorised, e))?;
            let out = client.call(&call)?;
            self.record(|v| v.live_tools = Some(live.tools));
            let parsed = parse_hits(&out, &req.starting_dir, include_snippets, limit);
            let mut notes = Vec::new();
            if parsed.unrecognised_format {
                notes.push("The connector answered in a format WinClean does not recognise, so no results are shown.".to_string());
            }
            if !plan.unsupported_filters.is_empty() {
                notes.push(format!("This connector does not support: {}.", plan.unsupported_filters.join(", ")));
            }
            Ok(SearchOutcome { rejected: parsed.rejected.len(), truncated: parsed.truncated, hits: parsed.hits, plan, notes })
        })();
        client.close();
        match &result {
            Ok(_) => self.record(|v| {
                v.permission = Some((PermissionState::Granted, None));
                v.verified_execution = true;
            }),
            Err(f) => self.denied(f),
        }
        result
    }

    /// Metadata for one file inside an authorised location.
    pub fn file_details(&self, windows_build: Option<u32>, path: &str, roots: &[String], cancel: &Arc<AtomicBool>) -> Result<Value, McpFailure> {
        let (mut client, _s, _h) = self.open(windows_build, cancel)?;
        let result = (|| {
            let live = client.list_tools()?;
            let (schema, _) = Self::live_schema(&live.tools, "get_file_details")?;
            let call = approve_metadata_call("get_file_details", path, roots, &schema).map_err(|e| McpFailure::new(FailureKind::NotAuthorised, e))?;
            let out = client.call(&call)?;
            Ok(out.structured.or_else(|| out.texts.iter().find_map(|t| serde_json::from_str::<Value>(t).ok())).unwrap_or(Value::Null))
        })();
        client.close();
        if let Err(f) = &result {
            self.denied(f);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::optimizer::mcp::odr::fakes::{ok, Files, ScriptedRunner};
    use crate::optimizer::mcp::registry::fixtures::{structured_content, FILE_EXPLORER_PFN};
    use crate::optimizer::mcp::transport::scripted::ScriptedTransport;
    use serde_json::json;
    use std::sync::atomic::Ordering;

    struct ScriptedLauncher {
        launches: Mutex<Vec<(String, Vec<String>)>>,
        make: Mutex<Option<Box<dyn FnMut() -> Result<ScriptedTransport, String> + Send>>>,
        closed: Arc<AtomicBool>,
    }
    impl Launcher for ScriptedLauncher {
        fn connect(&self, odr_path: &str, args: &[String]) -> Result<Box<dyn McpTransport>, String> {
            self.launches.lock().unwrap().push((odr_path.to_string(), args.to_vec()));
            let t = (self.make.lock().unwrap().as_mut().expect("launcher script"))()?;
            self.closed.store(false, Ordering::SeqCst);
            Ok(Box::new(Tracked { inner: t, closed: self.closed.clone() }))
        }
    }
    struct Tracked {
        inner: ScriptedTransport,
        closed: Arc<AtomicBool>,
    }
    impl McpTransport for Tracked {
        fn send_line(&mut self, l: &str) -> Result<(), String> {
            self.inner.send_line(l)
        }
        fn recv_line(&mut self, w: Duration) -> crate::optimizer::mcp::transport::Recv {
            self.inner.recv_line(w)
        }
        fn close(&mut self) {
            self.closed.store(true, Ordering::SeqCst);
            self.inner.close()
        }
    }

    fn reply(id: &Value, result: Value) -> String {
        json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string()
    }

    /// A File Explorer connector. `deny` makes tool calls fail with a consent error.
    fn explorer_server(deny: bool, search_result: Value) -> ScriptedTransport {
        ScriptedTransport::new(move |m| match m["method"].as_str() {
            Some("initialize") => vec![reply(&m["id"], json!({ "protocolVersion": "2025-06-18", "serverInfo": { "name": "FileExplorer", "version": "1" }, "capabilities": { "tools": {} } }))],
            Some("tools/list") => vec![reply(&m["id"], json!({ "tools": [
                { "name": "get_file_details", "inputSchema": { "type": "object", "properties": { "path": { "type": "string" } }, "required": ["path"] } },
                { "name": "get_directories", "inputSchema": { "type": "object", "properties": { "path": { "type": "string" } }, "required": ["path"] } },
                { "name": "search_files", "description": "Search files. On Copilot+ PCs: semantic search with natural language.", "inputSchema": { "type": "object", "properties": { "startingDir": { "type": "string" }, "searchPatterns": { "type": "string" }, "specifiedFileTypes": { "type": "array", "items": { "type": "string" } } }, "required": ["startingDir", "searchPatterns"] } },
                { "name": "move_file", "inputSchema": { "type": "object", "properties": { "oldFullPath": { "type": "string" }, "newFullPath": { "type": "string" } } } } ] }))],
            Some("tools/call") if deny => vec![reply(&m["id"], json!({ "isError": true, "content": [ { "type": "text", "text": "Access is denied: the user has not granted this host access to Downloads." } ] }))],
            Some("tools/call") => match m["params"]["name"].as_str() {
                Some("get_directories") => vec![reply(&m["id"], json!({ "structuredContent": { "result": [ { "path": "C:\\Users\\me\\Downloads\\Old" } ] } }))],
                Some("search_files") => vec![reply(&m["id"], json!({ "structuredContent": search_result.clone() }))],
                _ => vec![reply(&m["id"], json!({ "structuredContent": { "size": 5 } }))],
            },
            _ => vec![],
        })
    }

    fn manager(server: impl FnMut() -> Result<ScriptedTransport, String> + Send + 'static, listing: &str) -> (McpManager, Arc<ScriptedLauncher>, Arc<ScriptedRunner>) {
        let runner = Arc::new(ScriptedRunner::default().reply(&["list"], ok(listing)).reply(&["list"], ok(listing)).reply(&["list"], ok(listing)));
        let launcher = Arc::new(ScriptedLauncher { launches: Default::default(), make: Mutex::new(Some(Box::new(server))), closed: Default::default() });
        let env = OdrEnv { system_root: Some("C:\\Windows".into()), path_dirs: vec![], program_files: vec![], local_app_data: None };
        let files = Arc::new(Files(["C:\\Windows\\System32\\odr.exe".to_string()].into_iter().collect()));
        (McpManager::new(runner.clone(), launcher.clone(), env, files, || 1_000), launcher, runner)
    }

    fn roots() -> Vec<String> {
        vec!["C:\\Users\\me\\Downloads".to_string()]
    }
    fn cancel() -> Arc<AtomicBool> {
        Arc::new(AtomicBool::new(false))
    }
    fn search_req() -> SearchRequest {
        SearchRequest { starting_dir: "C:\\Users\\me\\Downloads".into(), patterns: "*.iso".into(), file_types: vec![], start_date: None, end_date: None, semantic: crate::optimizer::mcp::policy::Semantic::Off }
    }

    #[test]
    fn discovery_alone_never_claims_the_connector_works() {
        let (m, launcher, _) = manager(|| Err("must not start".into()), &structured_content());
        let s = m.status(Some(26300), true, false, &cancel());
        assert!(s.odr.found && s.odr.registry_format == Some(RegistryFormat::StructuredContent));
        assert_eq!(s.odr.trusted_location, Some(TrustedLocation::System32));
        let fe = s.file_explorer.unwrap();
        assert_eq!(fe.identity, ServerIdentity::MicrosoftFileExplorer);
        assert_eq!(fe.connection, ConnectionState::NotAttempted);
        assert_eq!(fe.permission, PermissionState::Unknown);
        assert!(!fe.verified_execution && fe.live_tools.is_none());
        assert_eq!(fe.search.source, "declared");
        assert!(fe.search.available && fe.search.file_types && !fe.search.start_date);
        assert_eq!(fe.search.semantic, SemanticSupport::Available);
        assert!(launcher.launches.lock().unwrap().is_empty(), "status must not launch any server");
    }

    #[test]
    fn repeated_status_calls_reuse_the_cached_listing() {
        let (m, _, runner) = manager(|| Err("x".into()), &structured_content());
        m.status(Some(26300), false, false, &cancel());
        m.status(Some(26300), false, false, &cancel());
        m.status(Some(26300), false, false, &cancel());
        assert_eq!(runner.calls.lock().unwrap().len(), 1, "odr.exe is launched once, not per UI refresh");
        m.status(Some(26300), false, true, &cancel());
        assert_eq!(runner.calls.lock().unwrap().len(), 2, "force re-runs it");
    }

    #[test]
    fn old_windows_builds_do_not_even_try_to_run_odr() {
        let (m, _, runner) = manager(|| Err("x".into()), &structured_content());
        let s = m.status(Some(26100), false, false, &cancel());
        assert!(!s.odr.found && s.odr.build_supported == Some(false));
        assert!(s.odr.error.unwrap().contains("older than 26220"));
        assert!(runner.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn verification_runs_the_documented_proxy_command_lists_live_tools_and_cleans_up() {
        let (m, launcher, _) = manager(|| Ok(explorer_server(false, json!({ "result": [] }))), &structured_content());
        let s = m.verify_connection(Some(26300), true, &cancel());
        let fe = s.file_explorer.unwrap();
        assert_eq!(fe.connection, ConnectionState::Connected);
        assert_eq!(fe.protocol_version.as_deref(), Some("2025-06-18"));
        assert_eq!(fe.live_tools.as_ref().unwrap().len(), 4);
        assert_eq!(fe.tools_match_declaration, Some(false), "the live list differs from the registry's declaration here");
        assert_eq!(fe.search.source, "live");
        assert_eq!(fe.search.semantic, SemanticSupport::Available);
        assert_eq!(fe.permission, PermissionState::Unknown, "listing tools does not prove Windows granted file access");
        assert!(!fe.verified_execution);
        let l = launcher.launches.lock().unwrap();
        assert_eq!(l[0].0, "C:\\Windows\\System32\\odr.exe");
        assert_eq!(l[0].1, vec!["mcp", "run", "--proxy", FILE_EXPLORER_PFN]);
        assert!(launcher.closed.load(Ordering::SeqCst), "the session must be closed afterwards");
    }

    #[test]
    fn a_successful_permission_check_is_the_proof_of_execution() {
        let (m, launcher, _) = manager(|| Ok(explorer_server(false, json!({ "result": [] }))), &structured_content());
        let s = m.verify_permission(Some(26300), false, "C:\\Users\\me\\Downloads", &roots(), &cancel()).unwrap();
        let fe = s.file_explorer.unwrap();
        assert_eq!(fe.permission, PermissionState::Granted);
        assert!(fe.verified_execution && fe.last_verified_ms == Some(1_000));
        assert!(launcher.closed.load(Ordering::SeqCst));
    }

    #[test]
    fn windows_consent_failures_are_reported_as_such_and_never_worked_around() {
        let (m, launcher, _) = manager(|| Ok(explorer_server(true, json!({}))), &structured_content());
        let err = m.verify_permission(Some(26300), false, "C:\\Users\\me\\Downloads", &roots(), &cancel()).unwrap_err();
        assert_eq!(err.kind, FailureKind::PermissionDenied);
        let fe = m.status(Some(26300), false, false, &cancel()).file_explorer.unwrap();
        assert_eq!(fe.permission, PermissionState::Denied);
        assert!(fe.permission_detail.unwrap().contains("not granted"));
        assert!(!fe.verified_execution);
        assert_eq!(launcher.launches.lock().unwrap().len(), 1, "no retry");
        let err = m.search(Some(26300), false, &search_req(), &roots(), false, 50, &cancel()).unwrap_err();
        assert_eq!(err.kind, FailureKind::PermissionDenied);
    }

    #[test]
    fn search_returns_sanitised_hits_and_reports_unsupported_filters() {
        let result = json!({ "result": [
            { "name": "win11.iso", "path": "C:\\Users\\me\\Downloads\\win11.iso", "dateCreated": "2024-01-01", "snippet": "private" },
            { "path": "C:\\Users\\me\\Documents\\taxes.pdf" } ] });
        let (m, launcher, _) = manager(move || Ok(explorer_server(false, result.clone())), &structured_content());
        let mut req = search_req();
        req.start_date = Some("2024-01-01".into());
        let out = m.search(Some(26300), true, &req, &roots(), false, 50, &cancel()).unwrap();
        assert_eq!(out.hits.len(), 1, "the entry outside the searched folder is dropped");
        assert_eq!(out.rejected, 1);
        assert_eq!(out.hits[0].snippet, None);
        assert!(out.notes.iter().any(|n| n.contains("does not support: start date")));
        let fe = m.status(Some(26300), true, false, &cancel()).file_explorer.unwrap();
        assert!(fe.verified_execution && fe.permission == PermissionState::Granted);
        assert!(launcher.closed.load(Ordering::SeqCst));
    }

    #[test]
    fn searching_outside_an_authorised_location_never_reaches_the_connector() {
        let (m, launcher, _) = manager(|| Ok(explorer_server(false, json!({ "result": [] }))), &structured_content());
        let mut req = search_req();
        req.starting_dir = "C:\\Users\\me\\Documents".into();
        let err = m.search(Some(26300), false, &req, &roots(), false, 50, &cancel()).unwrap_err();
        assert_eq!(err.kind, FailureKind::NotAuthorised);
        let calls = launcher.launches.lock().unwrap().len();
        assert_eq!(calls, 1, "the session opens, but no tools/call is sent");
        let err = m.search(Some(26300), false, &search_req(), &[], false, 50, &cancel()).unwrap_err();
        assert_eq!(err.kind, FailureKind::NotAuthorised);
    }

    #[test]
    fn untrusted_servers_are_never_launched() {
        let fake = r#"[{"server":{"name":"File Explorer","packages":[{"identifier":"Evil.FileExplorer_8wekyb3d8bbwe"}]}}]"#;
        let (m, launcher, _) = manager(|| Err("must not start".into()), fake);
        let err = m.search(Some(26300), false, &search_req(), &roots(), false, 50, &cancel()).unwrap_err();
        assert_eq!(err.kind, FailureKind::IdentityNotVerified);
        assert!(launcher.launches.lock().unwrap().is_empty());
        let s = m.status(Some(26300), false, false, &cancel());
        assert!(s.file_explorer.is_none());
        assert!(s.notes.iter().any(|n| n.contains("not proven to be Microsoft's connector")));
    }

    #[test]
    fn a_missing_agent_user_is_a_distinct_state_with_an_explicit_remedy() {
        let (m, _, _) = manager(|| Ok(ScriptedTransport::new(|m| vec![json!({ "jsonrpc": "2.0", "id": m["id"], "error": { "code": -32000, "message": "The agent user has not been provisioned" } }).to_string()])), &structured_content());
        let s = m.verify_connection(Some(26300), false, &cancel());
        let fe = s.file_explorer.unwrap();
        assert_eq!(fe.connection, ConnectionState::AgentUserNeeded);
        assert!(fe.connection_detail.unwrap().contains("agent user"));
    }

    #[test]
    fn provisioning_is_a_separate_explicit_operation() {
        let runner = Arc::new(ScriptedRunner::default().reply(&["list"], ok(&structured_content())).reply(&["mcp", "provision-agent-user"], ok("provisioned")));
        let launcher = Arc::new(ScriptedLauncher { launches: Default::default(), make: Mutex::new(None), closed: Default::default() });
        let env = OdrEnv { system_root: Some("C:\\Windows".into()), ..Default::default() };
        let files = Arc::new(Files(["C:\\Windows\\System32\\odr.exe".to_string()].into_iter().collect()));
        let m = McpManager::new(runner.clone(), launcher, env, files, || 1);
        m.status(Some(26300), false, false, &cancel());
        assert!(runner.calls.lock().unwrap().iter().all(|c| !c.contains("provision")), "never provisioned implicitly");
        assert_eq!(m.provision_agent_user(Some(26300), &cancel()).unwrap(), "provisioned");
    }

    #[test]
    fn file_details_use_the_live_schema_and_an_authorised_path() {
        let (m, _, _) = manager(|| Ok(explorer_server(false, json!({}))), &structured_content());
        let v = m.file_details(Some(26300), "C:\\Users\\me\\Downloads\\a.iso", &roots(), &cancel()).unwrap();
        assert_eq!(v["size"], 5);
        assert_eq!(m.file_details(Some(26300), "C:\\Windows\\notepad.exe", &roots(), &cancel()).unwrap_err().kind, FailureKind::NotAuthorised);
    }
}
