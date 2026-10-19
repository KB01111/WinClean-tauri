//! WinClean as a **read-only** MCP server (stdio), for agents such as Lumen.
//!
//! Start it with `winclean.exe --mcp-stdio`. It exposes diagnostics only —
//! services, processes, battery, hardware, capabilities, workload — and has
//! no tool that changes anything: no service change, no EcoQoS, no profile
//! switch, no file access. Mutations stay behind WinClean's own approval flow
//! (policy validation, explicit user approval, snapshot, rollback), which an
//! MCP client cannot reach. Every tool is annotated `readOnlyHint: true`, and
//! tests assert that no registered tool is anything else.

use std::io::{BufRead, Write};

use serde_json::{json, Value};

pub const PROTOCOL_VERSION: &str = "2025-06-18";

/// `(name, description)` of every tool. All are read-only.
pub const TOOLS: &[(&str, &str)] = &[
    ("winclean_services", "List Windows services with start type, state, dependencies and WinClean's risk classification. Read-only."),
    ("winclean_service_recommendations", "Deterministic start-type recommendations (never for protected services). Read-only; nothing is applied."),
    ("winclean_processes", "Top processes by memory with CPU share, foreground flag and EcoQoS eligibility. Read-only."),
    ("winclean_battery", "Battery telemetry: real discharge watts, health, cycle count, AC/DC state. Read-only."),
    ("winclean_hardware", "CPU topology, GPUs (registry only, never wakes the RTX 4070), NPU, G-Helper presence. Read-only."),
    ("winclean_capabilities", "Windows ML execution-provider assessment and hardware-aware routing. Read-only; no inference is run."),
    ("winclean_workload", "Classifies the current workload (coding, gaming, AI inference…). Read-only."),
];

/// Executes a tool and returns its JSON result.
pub trait ToolHost {
    fn call(&self, tool: &str, args: &Value) -> Result<Value, String>;
}

fn limit_arg(args: &Value, default: usize) -> Result<usize, String> {
    let obj = args.as_object().ok_or("arguments must be an object")?;
    for k in obj.keys() {
        if k != "limit" {
            return Err(format!("unexpected argument '{k}'"));
        }
    }
    match obj.get("limit") {
        None => Ok(default),
        Some(v) => v.as_u64().filter(|n| (1..=200).contains(n)).map(|n| n as usize).ok_or_else(|| "limit must be an integer from 1 to 200".to_string()),
    }
}

fn no_args(args: &Value) -> Result<(), String> {
    match args.as_object() {
        Some(o) if o.is_empty() => Ok(()),
        Some(o) => Err(format!("unexpected argument '{}'", o.keys().next().unwrap())),
        None if args.is_null() => Ok(()),
        None => Err("arguments must be an object".to_string()),
    }
}

fn tool_schema(name: &str) -> Value {
    let takes_limit = matches!(name, "winclean_services" | "winclean_processes");
    json!({
        "type": "object",
        "properties": if takes_limit { json!({ "limit": { "type": "integer", "minimum": 1, "maximum": 200 } }) } else { json!({}) },
        "additionalProperties": false
    })
}

/// Handles one JSON-RPC line; returns the response line, or `None` for notifications.
pub fn handle_message(host: &dyn ToolHost, line: &str) -> Option<String> {
    let msg: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(_) => return Some(error_response(Value::Null, -32700, "parse error")),
    };
    let id = msg.get("id").cloned();
    let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");
    let Some(id) = id else {
        return None; // notifications (initialized, cancelled, …) need no reply
    };
    let response = match method {
        "initialize" => ok_response(
            id,
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": { "tools": { "listChanged": false } },
                "serverInfo": { "name": "winclean", "version": env!("CARGO_PKG_VERSION") },
                "instructions": "WinClean read-only diagnostics. No tool changes the system; changes require the user's approval inside WinClean."
            }),
        ),
        "ping" => ok_response(id, json!({})),
        "tools/list" => ok_response(
            id,
            json!({ "tools": TOOLS.iter().map(|(name, description)| json!({
                "name": name,
                "description": description,
                "inputSchema": tool_schema(name),
                "annotations": { "readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false }
            })).collect::<Vec<_>>() }),
        ),
        "tools/call" => {
            let params = msg.get("params").cloned().unwrap_or(Value::Null);
            let name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
            if !TOOLS.iter().any(|(t, _)| *t == name) {
                return Some(error_response(id, -32602, &format!("unknown tool '{name}'")));
            }
            let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
            let validated = match name {
                "winclean_services" | "winclean_processes" => limit_arg(&args, 100).map(|_| ()),
                _ => no_args(&args),
            };
            match validated.and_then(|_| host.call(name, &args)) {
                Ok(v) => ok_response(id, json!({ "content": [ { "type": "text", "text": v.to_string() } ], "isError": false })),
                Err(e) => ok_response(id, json!({ "content": [ { "type": "text", "text": e } ], "isError": true })),
            }
        }
        other => error_response(id, -32601, &format!("method not found: {other}")),
    };
    Some(response)
}

fn ok_response(id: Value, result: Value) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string()
}

fn error_response(id: Value, code: i64, message: &str) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }).to_string()
}

/// The real host: native, read-only Windows diagnostics.
pub struct SystemHost;

impl ToolHost for SystemHost {
    fn call(&self, tool: &str, args: &Value) -> Result<Value, String> {
        let to_json = |v: Result<Value, serde_json::Error>| v.map_err(|e| e.to_string());
        match tool {
            "winclean_services" => {
                let limit = limit_arg(args, 100)?;
                let services = super::services::list_services()?;
                to_json(serde_json::to_value(services.into_iter().take(limit).map(|s| json!({
                    "name": s.name, "displayName": s.display_name, "state": s.state, "startType": s.start_type,
                    "risk": s.classification.risk, "category": s.classification.category, "dependents": s.dependents.len(), "memoryMb": s.memory_mb
                })).collect::<Vec<_>>()))
            }
            "winclean_service_recommendations" => {
                let services = super::services::list_services()?;
                to_json(serde_json::to_value(super::service_plan::recommend_service_changes(&services)))
            }
            "winclean_processes" => {
                let limit = limit_arg(args, 60)?;
                let procs = super::processes::list_processes()?;
                to_json(serde_json::to_value(procs.into_iter().take(limit).map(|p| json!({
                    "pid": p.pid, "name": p.name, "workingSetMb": p.working_set_mb, "cpuPercent": p.cpu_percent_recent, "foreground": p.is_foreground, "ecoDecision": p.eco_decision
                })).collect::<Vec<_>>()))
            }
            "winclean_battery" => {
                let mut source = super::battery::system_source();
                let info = source.info().ok();
                let reading = source.read().ok();
                to_json(serde_json::to_value(super::battery::summarize(info, reading, &[])))
            }
            "winclean_hardware" => to_json(serde_json::to_value(super::hardware::hardware_info()?)),
            "winclean_capabilities" => to_json(serde_json::to_value(super::capabilities::capability_report(None)?)),
            "winclean_workload" => {
                let mut tracker = super::workload::CpuTracker::default();
                let _ = super::workload::sample(&mut tracker);
                std::thread::sleep(std::time::Duration::from_millis(400));
                to_json(serde_json::to_value(super::workload::sample(&mut tracker)))
            }
            other => Err(format!("unknown tool '{other}'")),
        }
    }
}

/// Runs the stdio server until stdin closes. Returns the process exit code.
pub fn run_stdio() -> i32 {
    let host = SystemHost;
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    for line in stdin.lock().lines().map_while(Result::ok) {
        if line.trim().is_empty() {
            continue;
        }
        if let Some(reply) = handle_message(&host, &line) {
            if writeln!(out, "{reply}").and_then(|_| out.flush()).is_err() {
                return 1;
            }
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Fake {
        calls: Mutex<Vec<String>>,
    }
    impl ToolHost for Fake {
        fn call(&self, tool: &str, _args: &Value) -> Result<Value, String> {
            self.calls.lock().unwrap().push(tool.to_string());
            if tool == "winclean_battery" {
                return Err("no battery".into());
            }
            Ok(json!({ "tool": tool }))
        }
    }

    fn send(host: &Fake, v: Value) -> Value {
        serde_json::from_str(&handle_message(host, &v.to_string()).expect("a response")).unwrap()
    }

    #[test]
    fn initialize_advertises_tools_only() {
        let r = send(&Fake::default(), json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {} }));
        assert_eq!(r["result"]["serverInfo"]["name"], "winclean");
        let caps = r["result"]["capabilities"].as_object().unwrap();
        assert!(caps.contains_key("tools"));
        for forbidden in ["resources", "prompts", "sampling", "roots", "logging"] {
            assert!(!caps.contains_key(forbidden), "{forbidden}");
        }
    }

    #[test]
    fn every_tool_is_annotated_read_only_and_has_a_closed_schema() {
        let r = send(&Fake::default(), json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }));
        let tools = r["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), TOOLS.len());
        for t in tools {
            assert_eq!(t["annotations"]["readOnlyHint"], true, "{}", t["name"]);
            assert_eq!(t["annotations"]["destructiveHint"], false);
            assert_eq!(t["inputSchema"]["additionalProperties"], false);
            let name = t["name"].as_str().unwrap();
            for word in ["apply", "set_", "delete", "write", "rollback", "disable", "enable", "kill", "terminate", "throttle"] {
                assert!(!name.contains(word), "tool name '{name}' suggests a mutation ({word})");
            }
        }
    }

    #[test]
    fn mutating_or_unknown_tools_are_not_callable_and_never_reach_the_host() {
        let host = Fake::default();
        for name in ["winclean_apply_profile", "winclean_set_service_start_type", "winclean_rollback", "winclean_ecoqos", "run_command", ""] {
            let r = send(&host, json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": { "name": name, "arguments": {} } }));
            assert_eq!(r["error"]["code"], -32602, "{name}");
        }
        assert!(host.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn arguments_are_validated_before_the_host_runs() {
        let host = Fake::default();
        for args in [json!({ "limit": 0 }), json!({ "limit": 201 }), json!({ "limit": "5" }), json!({ "path": "C:\\" }), json!([1])] {
            let r = send(&host, json!({ "jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": { "name": "winclean_services", "arguments": args } }));
            assert_eq!(r["result"]["isError"], true, "{args}");
        }
        let r = send(&host, json!({ "jsonrpc": "2.0", "id": 5, "method": "tools/call", "params": { "name": "winclean_hardware", "arguments": { "x": 1 } } }));
        assert_eq!(r["result"]["isError"], true, "tools without parameters take none");
        assert!(host.calls.lock().unwrap().is_empty());
        let ok = send(&host, json!({ "jsonrpc": "2.0", "id": 6, "method": "tools/call", "params": { "name": "winclean_services", "arguments": { "limit": 5 } } }));
        assert_eq!(ok["result"]["isError"], false);
        assert_eq!(host.calls.lock().unwrap().clone(), vec!["winclean_services"]);
    }

    #[test]
    fn host_errors_become_tool_errors_not_protocol_errors() {
        let r = send(&Fake::default(), json!({ "jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": { "name": "winclean_battery" } }));
        assert_eq!(r["result"]["isError"], true);
        assert_eq!(r["result"]["content"][0]["text"], "no battery");
    }

    #[test]
    fn notifications_get_no_reply_and_garbage_gets_a_parse_error() {
        let host = Fake::default();
        assert!(handle_message(&host, r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#).is_none());
        let bad: Value = serde_json::from_str(&handle_message(&host, "not json").unwrap()).unwrap();
        assert_eq!(bad["error"]["code"], -32700);
        let unknown = send(&host, json!({ "jsonrpc": "2.0", "id": 9, "method": "resources/list" }));
        assert_eq!(unknown["error"]["code"], -32601);
        assert_eq!(send(&host, json!({ "jsonrpc": "2.0", "id": 10, "method": "ping" }))["result"], json!({}));
    }

    #[test]
    fn the_server_speaks_to_winclean_s_own_client() {
        // Round trip through the real client policy: the server's tool names must be unknown
        // to the *client-side* File Explorer allow-list (they are a different namespace).
        for (name, _) in TOOLS {
            assert_eq!(super::super::mcp::classify_tool(name), super::super::mcp::ToolPolicy::Unknown);
        }
    }
}
