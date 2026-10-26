//! What WinClean may ask an MCP server to do.
//!
//! The rules are deliberately small and default-deny:
//!
//! * `get_file_details` and `get_directories` are metadata-only and callable
//!   for a path inside a location the user authorised.
//! * `search_files` can return snippets of file *content*, so it is its own
//!   class (`ConsentedSearch`): it is only reachable through the search
//!   adapter, for a user-initiated search, inside an authorised location.
//! * content readers, every mutating tool and every tool WinClean does not
//!   recognise are blocked. A server's own annotations are never trusted to
//!   upgrade a tool.
//!
//! A call can only be sent if it is wrapped in an [`ApprovedCall`], and the
//! only code that can build one lives in this file.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::schema::{validate_args, InputSchema};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ToolPolicy {
    /// Returns metadata only (names, sizes, dates). Callable for authorised paths.
    MetadataOnly,
    /// Searches and may return content snippets. Callable only through the
    /// consent-aware search adapter.
    ConsentedSearch,
    /// Reads file content. Blocked.
    ContentRead,
    /// Writes, moves, creates, compresses… Blocked unconditionally.
    Destructive,
    /// Not known to be read-only. Blocked.
    Unknown,
}

/// File Explorer connector tools, from Microsoft Learn "Windows File Explorer
/// MCP connector". Matching is exact; anything else is `Unknown` (blocked).
pub fn classify_tool(name: &str) -> ToolPolicy {
    match name {
        "get_file_details" | "get_directories" => ToolPolicy::MetadataOnly,
        "search_files" => ToolPolicy::ConsentedSearch,
        "read_text_file" | "read_file" => ToolPolicy::ContentRead,
        "edit_text_file" | "create_directory" | "move_file" | "create_text_file" | "zip_folder" | "unzip_folder" => ToolPolicy::Destructive,
        _ => ToolPolicy::Unknown,
    }
}

pub fn policy_note(policy: ToolPolicy) -> &'static str {
    match policy {
        ToolPolicy::MetadataOnly => "Metadata only. WinClean may call it for locations you authorised.",
        ToolPolicy::ConsentedSearch => "Search. May return content snippets, so WinClean only runs it when you start a search in a location you authorised, and never sends snippets to an AI model.",
        ToolPolicy::ContentRead => "Reads file contents. WinClean never calls it.",
        ToolPolicy::Destructive => "Changes files. WinClean never calls it.",
        ToolPolicy::Unknown => "Not recognised as read-only, so WinClean blocks it.",
    }
}

/// An absolute local path: `C:\…`, no `..`, no device/UNC prefixes, no alternate
/// data streams, no control or bidirectional-override characters, bounded length.
pub fn validate_path_arg(path: &str) -> Result<(), String> {
    if path.is_empty() || path.chars().count() > 1024 {
        return Err("path must be 1-1024 characters".into());
    }
    if path.chars().any(|c| c.is_control() || is_bidi_control(c)) {
        return Err("path contains control characters".into());
    }
    let b = path.as_bytes();
    let drive = b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/');
    if !drive {
        return Err("only absolute local paths like C:\\Users\\you\\file.txt are accepted (no UNC or device paths)".into());
    }
    for seg in path[3..].split(['\\', '/']) {
        if seg.contains(':') {
            return Err("path must not contain ':' (alternate data streams are not accepted)".into());
        }
        if !seg.is_empty() && seg.chars().all(|c| c == '.') {
            return Err("path must not contain '.' or '..' segments".into());
        }
        if seg.contains(['<', '>', '"', '|', '?', '*']) {
            return Err("path contains characters Windows does not allow".into());
        }
    }
    Ok(())
}

/// Bidirectional override/isolate characters can make a path read differently
/// from what it is; they are stripped from results and refused in arguments.
pub fn is_bidi_control(c: char) -> bool {
    matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{200E}' | '\u{200F}' | '\u{061C}')
}

pub use crate::pathutil::{is_within, norm_path};

/// A tool call that passed the policy. It cannot be built anywhere else, so
/// the client cannot be asked to send anything that skipped the checks.
#[derive(Clone, Debug)]
pub struct ApprovedCall {
    tool: &'static str,
    args: Value,
    kind: ApprovedKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApprovedKind {
    Metadata,
    Search,
}

impl ApprovedCall {
    pub fn tool(&self) -> &str {
        self.tool
    }
    pub fn args(&self) -> &Value {
        &self.args
    }
    pub fn kind(&self) -> ApprovedKind {
        self.kind
    }
}

fn require_authorised(roots: &[String], path: &str) -> Result<(), String> {
    validate_path_arg(path)?;
    if roots.iter().any(|r| is_within(r, path)) {
        Ok(())
    } else {
        Err("that location is not one you authorised for Windows MCP search. Add it in MCP Integrations first.".into())
    }
}

/// Approves `get_file_details` / `get_directories` for a path inside an
/// authorised location, against the connector's own (live) schema.
pub fn approve_metadata_call(tool: &str, path: &str, roots: &[String], schema: &InputSchema) -> Result<ApprovedCall, String> {
    let tool_name: &'static str = match tool {
        "get_file_details" => "get_file_details",
        "get_directories" => "get_directories",
        other => {
            return Err(match classify_tool(other) {
                ToolPolicy::ConsentedSearch => format!("'{other}' is only available through the consent-aware search."),
                ToolPolicy::ContentRead => format!("'{other}' reads file content; WinClean only allows metadata tools."),
                ToolPolicy::Destructive => format!("'{other}' changes files and is blocked."),
                _ => format!("'{other}' is not a tool WinClean can prove to be read-only, so it is blocked."),
            })
        }
    };
    require_authorised(roots, path)?;
    if !schema.has("path") {
        return Err(format!("the connector's '{tool_name}' does not take a 'path' argument; WinClean will not guess its arguments"));
    }
    let args = json!({ "path": path });
    validate_args(schema, &args)?;
    Ok(ApprovedCall { tool: tool_name, args, kind: ApprovedKind::Metadata })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Semantic {
    /// The user asked for a natural-language (semantic) search.
    Requested,
    Off,
}

#[derive(Clone, Debug)]
pub struct SearchRequest {
    pub starting_dir: String,
    pub patterns: String,
    pub file_types: Vec<String>,
    pub start_date: Option<String>,
    pub end_date: Option<String>,
    pub semantic: Semantic,
}

/// How a search request was mapped onto the connector's real schema.
#[derive(Clone, Debug, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SearchPlan {
    pub used_arguments: Vec<String>,
    /// Filters the user asked for that this connector's schema cannot express.
    pub unsupported_filters: Vec<String>,
    pub semantic: bool,
}

fn good_filter_text(s: &str, max: usize) -> bool {
    !s.is_empty() && s.chars().count() <= max && !s.chars().any(|c| c.is_control() || is_bidi_control(c))
}

/// Approves a user-initiated `search_files` call. The arguments are built from
/// the schema the connector reported (`searchPatterns`, `startingDir` and any
/// file-type / date filters it declares) — never from assumptions.
pub fn approve_search_call(req: &SearchRequest, roots: &[String], schema: &InputSchema, tool_description: Option<&str>, semantic_supported: bool) -> Result<(ApprovedCall, SearchPlan), String> {
    require_authorised(roots, &req.starting_dir)?;
    if !good_filter_text(&req.patterns, 256) {
        return Err("enter what to search for (up to 256 characters, no control characters)".into());
    }
    for key in ["startingDir", "searchPatterns"] {
        if !schema.has(key) {
            return Err(format!("the connector's search_files does not declare '{key}', so WinClean cannot build a valid call"));
        }
    }
    let mut plan = SearchPlan::default();
    let mut args = serde_json::Map::new();
    args.insert("startingDir".into(), json!(req.starting_dir));
    plan.used_arguments.push("startingDir".into());

    let patterns_prop = schema.property("searchPatterns").expect("checked above");
    // The documentation lists `searchPatterns` as a string; honour an array schema if that is what the connector declares.
    let pattern_value = if patterns_prop.accepts("string") { json!(req.patterns) } else if patterns_prop.accepts("array") { json!([req.patterns]) } else { json!(req.patterns) };
    args.insert("searchPatterns".into(), pattern_value);
    plan.used_arguments.push("searchPatterns".into());

    if !req.file_types.is_empty() {
        let types: Vec<String> = req.file_types.iter().map(|t| t.trim().trim_start_matches('.').to_string()).filter(|t| good_filter_text(t, 16)).take(16).collect();
        match schema.property("specifiedFileTypes") {
            Some(p) if !types.is_empty() && p.accepts("array") => {
                args.insert("specifiedFileTypes".into(), json!(types));
                plan.used_arguments.push("specifiedFileTypes".into());
            }
            Some(p) if !types.is_empty() && p.accepts("string") => {
                args.insert("specifiedFileTypes".into(), json!(types.join(",")));
                plan.used_arguments.push("specifiedFileTypes".into());
            }
            _ => plan.unsupported_filters.push("file types".into()),
        }
    }
    for (key, value, label) in [("startDate", &req.start_date, "start date"), ("endDate", &req.end_date, "end date")] {
        if let Some(v) = value {
            if schema.has(key) && good_filter_text(v, 40) {
                args.insert(key.into(), json!(v));
                plan.used_arguments.push(key.into());
            } else {
                plan.unsupported_filters.push(label.into());
            }
        }
    }
    // Semantic search has no argument of its own: it is the same call with a
    // natural-language pattern, only meaningful where the connector offers it.
    plan.semantic = req.semantic == Semantic::Requested && semantic_supported && tool_description.map(|d| d.to_lowercase().contains("semantic") || d.to_lowercase().contains("natural language")).unwrap_or(false);
    if req.semantic == Semantic::Requested && !plan.semantic {
        plan.unsupported_filters.push("semantic search".into());
    }
    let args = Value::Object(args);
    validate_args(schema, &args)?;
    Ok((ApprovedCall { tool: "search_files", args, kind: ApprovedKind::Search }, plan))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema(v: Value) -> InputSchema {
        InputSchema::from_value(&v)
    }
    fn path_schema() -> InputSchema {
        schema(json!({ "type": "object", "properties": { "path": { "type": "string" } }, "required": ["path"] }))
    }
    fn search_schema() -> InputSchema {
        schema(json!({ "type": "object", "properties": {
            "startingDir": { "type": "string" }, "searchPatterns": { "type": "string" },
            "specifiedFileTypes": { "type": "array", "items": { "type": "string" } }, "startDate": { "type": "string" }, "endDate": { "type": "string" }
        }, "required": ["startingDir", "searchPatterns"] }))
    }
    fn roots() -> Vec<String> {
        vec!["C:\\Users\\me\\Downloads".to_string()]
    }
    fn req(dir: &str) -> SearchRequest {
        SearchRequest { starting_dir: dir.into(), patterns: "*.iso".into(), file_types: vec![], start_date: None, end_date: None, semantic: Semantic::Off }
    }

    #[test]
    fn tools_are_classified_from_the_documented_list_and_search_is_its_own_class() {
        for t in ["get_file_details", "get_directories"] {
            assert_eq!(classify_tool(t), ToolPolicy::MetadataOnly, "{t}");
        }
        assert_eq!(classify_tool("search_files"), ToolPolicy::ConsentedSearch);
        for t in ["read_text_file", "read_file"] {
            assert_eq!(classify_tool(t), ToolPolicy::ContentRead, "{t}");
        }
        for t in ["edit_text_file", "create_directory", "move_file", "create_text_file", "zip_folder", "unzip_folder"] {
            assert_eq!(classify_tool(t), ToolPolicy::Destructive, "{t}");
        }
        for t in ["delete_everything", "", "GET_FILE_DETAILS", "get_file_details ", "search_files2"] {
            assert_eq!(classify_tool(t), ToolPolicy::Unknown, "{t:?}: matching is exact");
        }
    }

    #[test]
    fn path_arguments_must_be_plain_absolute_local_paths() {
        for ok in ["C:\\Users\\me\\a.txt", "d:/data/x", "C:\\a b\\c (1).txt", "C:\\a\\...b"] {
            assert!(validate_path_arg(ok).is_ok(), "{ok}");
        }
        for bad in [
            "", "relative\\path", "\\\\server\\share\\a", "\\\\?\\C:\\a", "C:\\a\\..\\b", "C:/a/../b", "C:\\a\u{0}b", "C:", "C:\\a\\.\\b",
            "C:\\a\\...", "C:\\a:stream", "C:\\a\\b:$DATA", "C:\\a\\<b>", "C:\\a\\b?", "C:\\a\u{202E}gnp.exe", &format!("C:\\{}", "x".repeat(1100)),
        ] {
            assert!(validate_path_arg(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn locations_are_compared_by_component_not_by_prefix() {
        assert!(is_within("C:\\Users\\me\\Downloads", "c:/users/ME/downloads/a/b.iso"));
        assert!(is_within("C:\\Users\\me\\Downloads\\", "C:\\Users\\me\\Downloads"));
        assert!(!is_within("C:\\Users\\me\\Downloads", "C:\\Users\\me\\Downloads2\\a"));
        assert!(!is_within("C:\\Users\\me\\Downloads", "C:\\Users\\me"));
        assert!(!is_within("", "C:\\a"));
    }

    #[test]
    fn metadata_calls_need_an_authorised_location_and_the_live_schema() {
        let ok = approve_metadata_call("get_file_details", "C:\\Users\\me\\Downloads\\a.iso", &roots(), &path_schema()).unwrap();
        assert_eq!((ok.tool(), ok.kind()), ("get_file_details", ApprovedKind::Metadata));
        assert_eq!(ok.args(), &json!({ "path": "C:\\Users\\me\\Downloads\\a.iso" }));
        assert!(approve_metadata_call("get_directories", "C:\\Users\\me\\Documents", &roots(), &path_schema()).unwrap_err().contains("not one you authorised"));
        assert!(approve_metadata_call("get_file_details", "C:\\Users\\me\\Downloads\\..\\Documents\\a", &roots(), &path_schema()).is_err());
        assert!(approve_metadata_call("get_file_details", "C:\\Users\\me\\Downloads\\a", &[], &path_schema()).is_err(), "no authorised location, no call");
        let other = schema(json!({ "type": "object", "properties": { "filePath": { "type": "string" } } }));
        assert!(approve_metadata_call("get_file_details", "C:\\Users\\me\\Downloads\\a", &roots(), &other).unwrap_err().contains("does not take a 'path'"));
    }

    #[test]
    fn nothing_else_can_be_approved_as_a_direct_call() {
        for tool in ["search_files", "read_file", "read_text_file", "move_file", "edit_text_file", "mystery"] {
            assert!(approve_metadata_call(tool, "C:\\Users\\me\\Downloads\\a", &roots(), &path_schema()).is_err(), "{tool}");
        }
    }

    #[test]
    fn search_arguments_follow_the_connectors_schema_and_stay_inside_the_authorised_location() {
        let mut r = req("C:\\Users\\me\\Downloads");
        r.file_types = vec![".iso".into(), "zip".into()];
        r.start_date = Some("2024-01-01".into());
        r.end_date = Some("2025-01-01".into());
        let (call, plan) = approve_search_call(&r, &roots(), &search_schema(), None, false).unwrap();
        assert_eq!(call.tool(), "search_files");
        assert_eq!(call.args()["startingDir"], "C:\\Users\\me\\Downloads");
        assert_eq!(call.args()["searchPatterns"], "*.iso");
        assert_eq!(call.args()["specifiedFileTypes"], json!(["iso", "zip"]));
        assert_eq!(plan.used_arguments.len(), 5);
        assert!(plan.unsupported_filters.is_empty());

        assert!(approve_search_call(&req("C:\\Users\\me\\Documents"), &roots(), &search_schema(), None, false).is_err());
        assert!(approve_search_call(&req("C:\\"), &roots(), &search_schema(), None, false).is_err());
        let mut bad = req("C:\\Users\\me\\Downloads");
        bad.patterns = "a\u{0}b".into();
        assert!(approve_search_call(&bad, &roots(), &search_schema(), None, false).is_err());
    }

    #[test]
    fn filters_the_connector_does_not_declare_are_reported_not_sent() {
        let minimal = schema(json!({ "type": "object", "properties": { "startingDir": { "type": "string" }, "searchPatterns": { "type": "string" } }, "required": ["startingDir", "searchPatterns"] }));
        let mut r = req("C:\\Users\\me\\Downloads");
        r.file_types = vec!["iso".into()];
        r.start_date = Some("2024-01-01".into());
        let (call, plan) = approve_search_call(&r, &roots(), &minimal, None, false).unwrap();
        assert_eq!(call.args().as_object().unwrap().len(), 2);
        assert_eq!(plan.unsupported_filters, vec!["file types", "start date"]);
        let no_dir = schema(json!({ "type": "object", "properties": { "searchPatterns": { "type": "string" } } }));
        assert!(approve_search_call(&r, &roots(), &no_dir, None, false).unwrap_err().contains("startingDir"));
        let array_patterns = schema(json!({ "type": "object", "properties": { "startingDir": { "type": "string" }, "searchPatterns": { "type": "array", "items": { "type": "string" } } } }));
        assert_eq!(approve_search_call(&req("C:\\Users\\me\\Downloads"), &roots(), &array_patterns, None, false).unwrap().0.args()["searchPatterns"], json!(["*.iso"]));
    }

    #[test]
    fn semantic_search_is_only_used_when_the_connector_offers_it() {
        let mut r = req("C:\\Users\\me\\Downloads");
        r.semantic = Semantic::Requested;
        let (_, plan) = approve_search_call(&r, &roots(), &search_schema(), Some("Searches files by name. Supports semantic search with natural language."), true).unwrap();
        assert!(plan.semantic && plan.unsupported_filters.is_empty());
        let (_, plan) = approve_search_call(&r, &roots(), &search_schema(), Some("Searches files by name."), true).unwrap();
        assert!(!plan.semantic && plan.unsupported_filters == vec!["semantic search"]);
        let (_, plan) = approve_search_call(&r, &roots(), &search_schema(), Some("semantic"), false).unwrap();
        assert!(!plan.semantic, "not on a PC without the capability");
    }
}
