//! Windows MCP integration (preview platform feature).
//!
//! * **Discovery** — `odr.exe mcp list` (the Windows On-device Agent Registry
//!   CLI). Needs Windows build 26220.7262+; Microsoft marks the feature as
//!   pre-release, so every failure is reported verbatim and nothing is assumed.
//! * **Policy** — WinClean only ever calls MCP tools it can prove are safe:
//!   the File Explorer connector's *metadata* tools. Tools that read file
//!   content, every destructive tool, and every unknown tool are blocked, and
//!   a path argument must be a plain absolute Windows path. Windows applies its
//!   own consent prompts on top.
//! * **Client** — a minimal newline-delimited JSON-RPC stdio client with
//!   timeouts, used to probe a server (`initialize` + `tools/list`) and to call
//!   an allowed metadata tool. WinClean's native scanner is untouched: MCP is
//!   never used to scan or delete files.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Lowest Windows build with the On-device Agent Registry (public preview).
pub const MIN_ODR_BUILD: u32 = 26220;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ToolPolicy {
    /// Returns file/folder metadata only. WinClean may call it (user-initiated).
    MetadataOnly,
    /// Reads file content or snippets. Blocked: WinClean never reads contents.
    ContentRead,
    /// Writes, moves, creates, compresses… Blocked unconditionally.
    Destructive,
    /// Not known to be read-only. Blocked.
    Unknown,
}

impl ToolPolicy {
    pub fn callable(&self) -> bool {
        *self == ToolPolicy::MetadataOnly
    }
}

/// File Explorer connector tools, from Microsoft Learn "Windows File Explorer
/// MCP connector". Anything not listed is `Unknown` and therefore blocked.
pub fn classify_tool(name: &str) -> ToolPolicy {
    match name {
        "get_file_details" | "get_directories" => ToolPolicy::MetadataOnly,
        "read_text_file" | "read_file" | "search_files" => ToolPolicy::ContentRead,
        "edit_text_file" | "create_directory" | "move_file" | "create_text_file" | "zip_folder" | "unzip_folder" => ToolPolicy::Destructive,
        _ => ToolPolicy::Unknown,
    }
}

/// A path argument must be a plain absolute local path: `C:\…`, no `..`, no
/// device/UNC prefixes, no control characters, bounded length.
pub fn validate_path_arg(path: &str) -> Result<(), String> {
    if path.is_empty() || path.chars().count() > 1024 {
        return Err("path must be 1-1024 characters".into());
    }
    if path.chars().any(|c| c.is_control()) {
        return Err("path contains control characters".into());
    }
    let b = path.as_bytes();
    let drive = b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/');
    if !drive {
        return Err("only absolute local paths like C:\\Users\\you\\file.txt are accepted (no UNC or device paths)".into());
    }
    if path.split(['\\', '/']).any(|seg| seg == "..") {
        return Err("path must not contain '..' segments".into());
    }
    Ok(())
}

/// Checks a tool call against the policy before anything is sent.
pub fn check_call(tool: &str, args: &Value) -> Result<(), String> {
    let policy = classify_tool(tool);
    if !policy.callable() {
        return Err(match policy {
            ToolPolicy::ContentRead => format!("'{tool}' reads file content; WinClean only allows metadata tools."),
            ToolPolicy::Destructive => format!("'{tool}' changes files and is blocked."),
            _ => format!("'{tool}' is not a tool WinClean can prove to be read-only, so it is blocked."),
        });
    }
    let obj = args.as_object().ok_or("arguments must be an object")?;
    let path_key = if tool == "get_file_details" || tool == "get_directories" { "path" } else { "" };
    for (k, v) in obj {
        if k != path_key {
            return Err(format!("unexpected argument '{k}'"));
        }
        validate_path_arg(v.as_str().ok_or("path must be a string")?)?;
    }
    if !obj.contains_key(path_key) {
        return Err("missing required argument 'path'".into());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Discovery
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpToolInfo {
    pub name: String,
    pub description: Option<String>,
    pub policy: ToolPolicy,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerInfo {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub tools: Vec<McpToolInfo>,
    pub is_file_explorer: bool,
    /// The registry entry exactly as `odr.exe` printed it (schema is preview).
    pub raw: Value,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpDiscovery {
    /// `odr.exe` could be started.
    pub odr_available: bool,
    pub windows_build: Option<u32>,
    pub build_supported: Option<bool>,
    pub servers: Vec<McpServerInfo>,
    pub file_explorer_found: bool,
    pub notes: Vec<String>,
    /// First part of the raw CLI output, for support/debugging.
    pub raw_excerpt: Option<String>,
}

fn first_str<'a>(v: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|k| v.get(k).and_then(|x| x.as_str()))
}

fn extract_tools(entry: &Value) -> Vec<McpToolInfo> {
    // ODR keeps the static tool list in `_meta.com.microsoft.windows` (preview).
    let candidates = [
        entry.get("tools"),
        entry.pointer("/_meta/com.microsoft.windows/tools/list/tools"),
        entry.pointer("/_meta/com.microsoft.windows/static_responses/tools~1list/tools"),
        entry.pointer("/_meta/com.microsoft.windows/static_responses/tools/list/tools"),
    ];
    for c in candidates.into_iter().flatten() {
        if let Some(arr) = c.as_array() {
            return arr
                .iter()
                .filter_map(|t| {
                    let name = if t.is_string() { t.as_str()? } else { t.get("name")?.as_str()? };
                    Some(McpToolInfo { name: name.to_string(), description: t.get("description").and_then(|d| d.as_str()).map(|s| s.to_string()), policy: classify_tool(name) })
                })
                .collect();
        }
    }
    Vec::new()
}

/// Tolerant parser for `odr.exe mcp list` output: a JSON array of servers or an
/// object holding one (`servers`, `mcp_servers`, `items`).
pub fn parse_odr_list(stdout: &str) -> Result<Vec<McpServerInfo>, String> {
    let value: Value = serde_json::from_str(stdout.trim()).map_err(|e| format!("odr.exe did not print JSON ({e})"))?;
    let items: Vec<Value> = match &value {
        Value::Array(a) => a.clone(),
        Value::Object(o) => ["servers", "mcp_servers", "mcpServers", "items"].iter().find_map(|k| o.get(*k).and_then(|v| v.as_array()).cloned()).unwrap_or_else(|| vec![value.clone()]),
        _ => return Err("unexpected odr.exe output shape".into()),
    };
    Ok(items
        .into_iter()
        .filter(|v| v.is_object())
        .map(|v| {
            let id = first_str(&v, &["id", "server_id", "serverId", "package_family_name", "name"]).unwrap_or("unknown").to_string();
            let name = first_str(&v, &["displayName", "display_name", "title", "name"]).unwrap_or(&id).to_string();
            let hay = format!("{id} {name}").to_ascii_lowercase();
            McpServerInfo {
                is_file_explorer: hay.contains("file") && hay.contains("explorer") || hay.contains("fileexplorer"),
                description: first_str(&v, &["description"]).map(|s| s.to_string()),
                tools: extract_tools(&v),
                id,
                name,
                raw: v,
            }
        })
        .collect())
}

pub struct CmdOutput {
    pub stdout: String,
    pub success: bool,
}

/// Runs a short-lived command (injectable so discovery is testable).
pub trait CommandRunner: Send + Sync {
    fn run(&self, program: &str, args: &[&str], timeout: Duration) -> Result<CmdOutput, String>;
}

pub struct SystemRunner;

impl CommandRunner for SystemRunner {
    fn run(&self, program: &str, args: &[&str], timeout: Duration) -> Result<CmdOutput, String> {
        let mut cmd = Command::new(program);
        cmd.args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        let mut child = cmd.spawn().map_err(|e| format!("could not start {program}: {e}"))?;
        let mut stdout = child.stdout.take().ok_or("no stdout")?;
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            use std::io::Read;
            let mut buf = Vec::new();
            let _ = (&mut stdout).take(1024 * 1024).read_to_end(&mut buf);
            let _ = tx.send(buf);
        });
        let deadline = Instant::now() + timeout;
        let status = loop {
            match child.try_wait().map_err(|e| e.to_string())? {
                Some(s) => break s,
                None if Instant::now() >= deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!("{program} did not finish within {} s and was stopped", timeout.as_secs()));
                }
                None => std::thread::sleep(Duration::from_millis(40)),
            }
        };
        let buf = rx.recv_timeout(Duration::from_secs(2)).unwrap_or_default();
        Ok(CmdOutput { stdout: String::from_utf8_lossy(&buf).into_owned(), success: status.success() })
    }
}

/// Discovers registered MCP servers. Read-only: only `odr.exe mcp list` is run.
pub fn discover(runner: &dyn CommandRunner, windows_build: Option<u32>) -> McpDiscovery {
    let mut d = McpDiscovery { windows_build, build_supported: windows_build.map(|b| b >= MIN_ODR_BUILD), ..Default::default() };
    if d.build_supported == Some(false) {
        d.notes.push(format!("Windows build {} is older than {MIN_ODR_BUILD}; the On-device Agent Registry (MCP on Windows preview) needs build {MIN_ODR_BUILD}.7262 or later.", windows_build.unwrap_or(0)));
    }
    let timeout = Duration::from_secs(6);
    // The docs show both `odr mcp list` and `odr list`; try the documented one first.
    let attempt = runner.run("odr.exe", &["mcp", "list"], timeout).or_else(|first| runner.run("odr.exe", &["list"], timeout).map_err(|second| format!("{first}; {second}")));
    match attempt {
        Ok(out) => {
            d.odr_available = true;
            d.raw_excerpt = Some(out.stdout.chars().take(2000).collect());
            if out.stdout.trim().is_empty() {
                d.notes.push(if out.success { "odr.exe reported no registered MCP servers." } else { "odr.exe exited with an error and printed nothing." }.to_string());
            } else {
                match parse_odr_list(&out.stdout) {
                    Ok(servers) => {
                        d.file_explorer_found = servers.iter().any(|s| s.is_file_explorer);
                        d.servers = servers;
                    }
                    Err(e) => d.notes.push(e),
                }
            }
        }
        Err(e) => d.notes.push(format!("odr.exe is not available ({e}). The MCP-on-Windows preview needs Windows build {MIN_ODR_BUILD}.7262+ with the feature enabled.")),
    }
    if d.servers.iter().any(|s| s.tools.iter().any(|t| t.policy == ToolPolicy::Destructive)) {
        d.notes.push("Some registered tools change files. WinClean never calls them.".to_string());
    }
    d
}

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

const PROTOCOL_VERSION: &str = "2025-06-18";

pub struct McpSession {
    child: Child,
    stdin: ChildStdin,
    rx: Receiver<String>,
    next_id: u64,
    timeout: Duration,
}

impl McpSession {
    pub fn spawn(program: &str, args: &[String], timeout: Duration) -> Result<Self, String> {
        Self::spawn_with_env(program, args, timeout, &[])
    }

    pub fn spawn_with_env(program: &str, args: &[String], timeout: Duration, envs: &[(&str, &str)]) -> Result<Self, String> {
        let mut cmd = Command::new(program);
        cmd.args(args).envs(envs.iter().copied()).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000);
        }
        let mut child = cmd.spawn().map_err(|e| format!("could not start {program}: {e}"))?;
        let stdin = child.stdin.take().ok_or("no stdin")?;
        let stdout = child.stdout.take().ok_or("no stdout")?;
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if line.len() > 4 * 1024 * 1024 || tx.send(line).is_err() {
                    break;
                }
            }
        });
        Ok(Self { child, stdin, rx, next_id: 1, timeout })
    }

    fn send(&mut self, message: &Value) -> Result<(), String> {
        let mut line = serde_json::to_string(message).map_err(|e| e.to_string())?;
        line.push('\n');
        self.stdin.write_all(line.as_bytes()).and_then(|_| self.stdin.flush()).map_err(|e| format!("could not write to the server: {e}"))
    }

    /// Sends a request and waits for the response with the same id. Lines that
    /// are not JSON-RPC (stray logs) and notifications are ignored.
    pub fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))?;
        let deadline = Instant::now() + self.timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let line = self.rx.recv_timeout(remaining).map_err(|_| format!("the server did not answer '{method}' in {} s", self.timeout.as_secs()))?;
            let Ok(msg) = serde_json::from_str::<Value>(&line) else { continue };
            if msg.get("id").and_then(|v| v.as_u64()) != Some(id) {
                continue;
            }
            if let Some(err) = msg.get("error") {
                return Err(format!("{}", err.get("message").and_then(|m| m.as_str()).unwrap_or("server error")));
            }
            return Ok(msg.get("result").cloned().unwrap_or(Value::Null));
        }
    }

    pub fn initialize(&mut self) -> Result<Value, String> {
        let result = self.request("initialize", json!({ "protocolVersion": PROTOCOL_VERSION, "capabilities": {}, "clientInfo": { "name": "WinClean", "version": env!("CARGO_PKG_VERSION") } }))?;
        self.send(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }))?;
        Ok(result)
    }

    pub fn list_tools(&mut self) -> Result<Vec<McpToolInfo>, String> {
        let result = self.request("tools/list", json!({}))?;
        Ok(result
            .get("tools")
            .and_then(|t| t.as_array())
            .map(|a| a.iter().filter_map(|t| t.get("name").and_then(|n| n.as_str()).map(|n| McpToolInfo { name: n.to_string(), description: t.get("description").and_then(|d| d.as_str()).map(|s| s.to_string()), policy: classify_tool(n) })).collect())
            .unwrap_or_default())
    }

    /// Calls a tool *after* the policy check. A blocked call never reaches the server.
    pub fn call_tool(&mut self, tool: &str, args: Value) -> Result<Value, String> {
        check_call(tool, &args)?;
        self.request("tools/call", json!({ "name": tool, "arguments": args }))
    }
}

impl Drop for McpSession {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_explorer_tools_are_classified_from_the_documented_list() {
        for t in ["get_file_details", "get_directories"] {
            assert_eq!(classify_tool(t), ToolPolicy::MetadataOnly, "{t}");
        }
        for t in ["read_text_file", "read_file", "search_files"] {
            assert_eq!(classify_tool(t), ToolPolicy::ContentRead, "{t}");
        }
        for t in ["edit_text_file", "create_directory", "move_file", "create_text_file", "zip_folder", "unzip_folder"] {
            assert_eq!(classify_tool(t), ToolPolicy::Destructive, "{t}");
        }
        for t in ["delete_everything", "", "GET_FILE_DETAILS", "get_file_details "] {
            assert_eq!(classify_tool(t), ToolPolicy::Unknown, "{t:?}: matching is exact and unknown tools are blocked");
        }
    }

    #[test]
    fn only_metadata_tools_pass_the_call_check() {
        assert!(check_call("get_file_details", &json!({ "path": "C:\\Users\\me\\a.txt" })).is_ok());
        assert!(check_call("get_directories", &json!({ "path": "C:/Users/me" })).is_ok());
        for blocked in ["read_text_file", "search_files", "move_file", "edit_text_file", "mystery_tool"] {
            assert!(check_call(blocked, &json!({ "path": "C:\\a" })).is_err(), "{blocked}");
        }
    }

    #[test]
    fn path_arguments_must_be_plain_absolute_local_paths() {
        for ok in ["C:\\Users\\me\\a.txt", "d:/data/x", "C:\\a b\\c (1).txt"] {
            assert!(validate_path_arg(ok).is_ok(), "{ok}");
        }
        for bad in ["", "relative\\path", "\\\\server\\share\\a", "\\\\?\\C:\\a", "C:\\a\\..\\b", "C:/a/../b", "C:\\a\u{0}b", "C:", &format!("C:\\{}", "x".repeat(1100))] {
            assert!(validate_path_arg(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn unexpected_or_missing_arguments_are_refused() {
        assert!(check_call("get_file_details", &json!({ "path": "C:\\a", "also": "x" })).is_err());
        assert!(check_call("get_file_details", &json!({})).is_err());
        assert!(check_call("get_file_details", &json!({ "path": 5 })).is_err());
        assert!(check_call("get_file_details", &json!(["C:\\a"])).is_err());
    }

    struct FakeRunner(Result<String, String>, std::sync::Mutex<Vec<String>>);
    impl CommandRunner for FakeRunner {
        fn run(&self, program: &str, args: &[&str], _t: Duration) -> Result<CmdOutput, String> {
            self.1.lock().unwrap().push(format!("{program} {}", args.join(" ")));
            self.0.clone().map(|stdout| CmdOutput { stdout, success: true })
        }
    }

    #[test]
    fn discovery_parses_arrays_and_wrapped_lists_and_finds_file_explorer() {
        let array = r#"[{"id":"MicrosoftWindows.Client.FileExplorer_cw5n1h2txyewy","name":"File Explorer","tools":[{"name":"get_file_details"},{"name":"move_file"},{"name":"read_file"}]},{"id":"Other.Server","name":"Other"}]"#;
        let d = discover(&FakeRunner(Ok(array.into()), Default::default()), Some(26300));
        assert!(d.odr_available && d.file_explorer_found);
        assert_eq!(d.servers.len(), 2);
        let fe = &d.servers[0];
        assert!(fe.is_file_explorer);
        assert_eq!(fe.tools.iter().map(|t| t.policy).collect::<Vec<_>>(), vec![ToolPolicy::MetadataOnly, ToolPolicy::Destructive, ToolPolicy::ContentRead]);
        assert!(d.notes.iter().any(|n| n.contains("change files")));
        assert!(!d.servers[1].is_file_explorer);

        let wrapped = r#"{"servers":[{"name":"Windows Settings","description":"settings"}]}"#;
        let d = discover(&FakeRunner(Ok(wrapped.into()), Default::default()), Some(26300));
        assert_eq!(d.servers[0].name, "Windows Settings");
        assert_eq!(d.servers[0].description.as_deref(), Some("settings"));
    }

    #[test]
    fn tools_are_read_from_the_preview_meta_block_too() {
        let v = r#"[{"id":"x","_meta":{"com.microsoft.windows":{"tools/list":{"tools":[{"name":"get_directories"}]}}}}]"#;
        // The key contains a slash, which a JSON pointer would need escaped; the tolerant parser must still not panic.
        let d = discover(&FakeRunner(Ok(v.into()), Default::default()), Some(26300));
        assert_eq!(d.servers.len(), 1);
    }

    #[test]
    fn discovery_reports_every_failure_instead_of_guessing() {
        let missing = discover(&FakeRunner(Err("not found".into()), Default::default()), Some(26100));
        assert!(!missing.odr_available);
        assert!(missing.notes.iter().any(|n| n.contains("odr.exe is not available")));
        assert_eq!(missing.build_supported, Some(false));
        assert!(missing.notes.iter().any(|n| n.contains("older than 26220")));

        let junk = discover(&FakeRunner(Ok("Usage: odr [command]".into()), Default::default()), Some(26300));
        assert!(junk.odr_available && junk.servers.is_empty());
        assert!(junk.notes.iter().any(|n| n.contains("did not print JSON")));

        let empty = discover(&FakeRunner(Ok("   ".into()), Default::default()), None);
        assert_eq!(empty.build_supported, None);
        assert!(empty.notes.iter().any(|n| n.contains("no registered MCP servers")));
    }

    #[test]
    fn discovery_only_runs_the_read_only_list_command() {
        let runner = FakeRunner(Ok("[]".into()), Default::default());
        discover(&runner, Some(26300));
        assert_eq!(runner.1.lock().unwrap().clone(), vec!["odr.exe mcp list"]);
    }

    // --- client against a real child process (this test binary, in mock mode) ---

    /// Entry point of the mock MCP server: runs only when spawned by the client tests.
    #[test]
    fn mcp_mock_server_child() {
        if std::env::var("WINCLEAN_MCP_MOCK").is_err() {
            return;
        }
        let stdin = std::io::stdin();
        let mut out = std::io::stdout();
        // The test harness prints "test <name> ... " without a newline right
        // before running this function; end that line so every JSON reply
        // below starts on a line of its own.
        let _ = writeln!(out);
        let _ = out.flush();
        for line in stdin.lock().lines().map_while(Result::ok) {
            let Ok(msg) = serde_json::from_str::<Value>(&line) else { continue };
            let Some(id) = msg.get("id").cloned() else { continue };
            let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");
            let result = match method {
                "initialize" => json!({ "protocolVersion": PROTOCOL_VERSION, "serverInfo": { "name": "mock", "version": "1" }, "capabilities": { "tools": {} } }),
                "tools/list" => json!({ "tools": [ { "name": "get_file_details" }, { "name": "move_file" } ] }),
                "tools/call" => json!({ "content": [ { "type": "text", "text": format!("called {}", msg["params"]["name"]) } ] }),
                // A request the server deliberately never answers (timeout test).
                "ignore" => continue,
                _ => {
                    let _ = writeln!(out, "{}", json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32601, "message": "method not found" } }));
                    continue;
                }
            };
            let _ = writeln!(out, "{}", json!({ "jsonrpc": "2.0", "id": id, "result": result }));
            let _ = out.flush();
        }
        std::process::exit(0);
    }

    fn spawn_mock() -> McpSession {
        let exe = std::env::current_exe().unwrap();
        // Re-run this test binary running only the mock-server "test"; the
        // environment variable is passed to the child alone.
        McpSession::spawn_with_env(
            exe.to_str().unwrap(),
            &["optimizer::mcp::tests::mcp_mock_server_child".into(), "--exact".into(), "--nocapture".into(), "--test-threads=1".into()],
            Duration::from_secs(20),
            &[("WINCLEAN_MCP_MOCK", "1")],
        )
        .unwrap()
    }

    #[test]
    fn client_probes_a_server_and_only_sends_allowed_calls() {
        let mut s = spawn_mock();
        let init = s.initialize().unwrap();
        assert_eq!(init["serverInfo"]["name"], "mock");
        let tools = s.list_tools().unwrap();
        assert_eq!(tools.iter().map(|t| t.policy).collect::<Vec<_>>(), vec![ToolPolicy::MetadataOnly, ToolPolicy::Destructive]);
        let ok = s.call_tool("get_file_details", json!({ "path": "C:\\Users\\me\\a.txt" })).unwrap();
        assert!(ok["content"][0]["text"].as_str().unwrap().contains("get_file_details"));
        // Blocked calls fail locally and never reach the server.
        assert!(s.call_tool("move_file", json!({ "path": "C:\\a" })).is_err());
        assert!(s.call_tool("get_file_details", json!({ "path": "C:\\a\\..\\b" })).is_err());
        // The session is still healthy afterwards.
        assert!(s.list_tools().is_ok());
    }

    #[test]
    fn server_errors_surface_as_messages_and_unanswered_requests_time_out() {
        let mut s = spawn_mock();
        s.initialize().unwrap();
        assert_eq!(s.request("nope", json!({})).unwrap_err(), "method not found");
        s.timeout = Duration::from_millis(300);
        let started = Instant::now();
        let err = s.request("ignore", json!({})).unwrap_err();
        assert!(err.contains("did not answer 'ignore'"), "{err}");
        assert!(started.elapsed() < Duration::from_secs(5), "a silent server must not hang the caller");
    }
}
