//! Parsing of the On-device Agent Registry listing (`odr.exe list`).
//!
//! Microsoft documents more than one shape for this output while the feature
//! is in preview, so the parser handles each variant explicitly and reports
//! which one it recognised:
//!
//! * **structuredContent** — the shape read by Microsoft's `mcp-client-js`
//!   sample: `{ "structuredContent": { "servers": [ { "server": { "name",
//!   "packages": [ { "identifier" } ] }, … } ] } }`.
//! * **textContent** — the same object delivered as MCP text content.
//! * **array / servers object** — the shape in the Learn quickstart: a list of
//!   entries carrying an MCPB-style `manifest` (`manifest.server.mcp_config`).
//!
//! Everything in the listing is untrusted input: sizes are bounded, strings
//! are cleaned, identifiers are validated before they can reach a command
//! line, and an entry that merely *calls itself* "File Explorer" is never
//! trusted without a Microsoft publisher identity.

use serde::Serialize;
use serde_json::Value;

use super::policy::{classify_tool, ToolPolicy};
use super::schema::InputSchema;

const MAX_SERVERS: usize = 200;
const MAX_TOOLS: usize = 100;
const META_KEY: &str = "com.microsoft.windows";
/// Publisher id of packages signed by Microsoft Windows.
const MICROSOFT_PUBLISHER_ID: &str = "cw5n1h2txyewy";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RegistryFormat {
    StructuredContent,
    TextContent,
    Array,
    ServersObject,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolDecl {
    pub name: String,
    pub description: Option<String>,
    pub policy: ToolPolicy,
    pub input_schema: InputSchema,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ServerIdentity {
    /// Microsoft-published package that is the Windows File Explorer connector.
    MicrosoftFileExplorer,
    /// Microsoft-published package, some other connector.
    MicrosoftPublished,
    /// The registry reports a publisher that is not Microsoft.
    ThirdParty,
    /// The registry reported no usable publisher information.
    Unverified,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ConnectionMethod {
    /// `odr.exe mcp run --proxy <identifier>` (Microsoft sample). Runs contained
    /// in an agent session and keeps Windows' consent prompts.
    OdrProxy,
    /// The registry only offers a manifest command to run directly. WinClean
    /// never executes commands taken from registry data.
    ManifestCommandOnly,
    Unavailable,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisteredServer {
    /// Stable key for the UI: the validated identifier, else a slug of the name.
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub version: Option<String>,
    /// Validated package identifier, the only value ever passed to `odr.exe`.
    pub identifier: Option<String>,
    pub package_family_name: Option<String>,
    pub identity: ServerIdentity,
    /// Named like File Explorer but not proven to be Microsoft's connector.
    pub impersonation_warning: bool,
    pub connection: ConnectionMethod,
    /// Tools the registry declares (`static_responses`); not yet verified live.
    pub declared_tools: Vec<ToolDecl>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ParsedRegistry {
    pub format: RegistryFormat,
    pub servers: Vec<RegisteredServer>,
    pub notes: Vec<String>,
}

pub fn clean(s: &str, max: usize) -> String {
    s.chars().filter(|c| !c.is_control() && !super::policy::is_bidi_control(*c)).take(max).collect::<String>().trim().to_string()
}

/// Package identifiers go onto a command line, so they are restricted to the
/// characters of package names and family names, and may not look like options.
pub fn valid_identifier(id: &str) -> bool {
    !id.is_empty() && id.len() <= 128 && !id.starts_with(['-', '.']) && id.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

fn looks_like_family_name(s: &str) -> bool {
    match s.rsplit_once('_') {
        Some((name, publisher)) => name.len() >= 3 && publisher.len() == 13 && publisher.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()) && valid_identifier(s),
        None => false,
    }
}

fn publisher_of(family_or_id: &str) -> Option<&str> {
    looks_like_family_name(family_or_id).then(|| family_or_id.rsplit_once('_').map(|(_, p)| p)).flatten()
}

fn compact(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_lowercase()
}

fn s<'a>(v: &'a Value, path: &[&str]) -> Option<&'a str> {
    let mut cur = v;
    for p in path {
        cur = cur.get(p)?;
    }
    cur.as_str()
}

fn meta_block(entry: &Value) -> Option<&Value> {
    ["_meta", "meta"].iter().find_map(|k| entry.get(k)).or_else(|| entry.pointer("/server/_meta")).or_else(|| entry.pointer("/manifest/_meta")).and_then(|m| m.get(META_KEY))
}

fn parse_tools(list: &Value) -> Vec<ToolDecl> {
    let Some(arr) = list.as_array() else { return Vec::new() };
    arr.iter()
        .take(MAX_TOOLS)
        .filter_map(|t| {
            let name = if t.is_string() { t.as_str()? } else { t.get("name")?.as_str()? };
            let name = clean(name, 64);
            if name.is_empty() || !name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.')) {
                return None;
            }
            Some(ToolDecl {
                policy: classify_tool(&name),
                description: t.get("description").and_then(|d| d.as_str()).map(|d| clean(d, 400)),
                input_schema: t.get("inputSchema").map(InputSchema::from_value).unwrap_or_default(),
                name,
            })
        })
        .collect()
}

fn declared_tools(entry: &Value) -> Vec<ToolDecl> {
    let mut found = Vec::new();
    if let Some(meta) = meta_block(entry) {
        let sr = meta.get("static_responses");
        // The documented key is literally "tools/list"; tolerate a nested form too.
        let list = sr.and_then(|r| r.get("tools/list")).or_else(|| sr.and_then(|r| r.pointer("/tools/list")));
        if let Some(l) = list.and_then(|l| l.get("tools")) {
            found = parse_tools(l);
        }
    }
    if found.is_empty() {
        for p in ["/tools", "/manifest/tools", "/server/tools"] {
            if let Some(t) = entry.pointer(p) {
                found = parse_tools(t);
                if !found.is_empty() {
                    break;
                }
            }
        }
    }
    found
}

fn classify_identity(name: &str, identifier: Option<&str>, family: Option<&str>, tools: &[ToolDecl]) -> (ServerIdentity, bool) {
    let publisher = family.and_then(publisher_of).or_else(|| identifier.and_then(publisher_of));
    let named_like_explorer = [Some(name), identifier, family].into_iter().flatten().any(|n| compact(n).contains("fileexplorer"));
    let looks_like_connector = tools.is_empty() || tools.iter().any(|t| matches!(t.name.as_str(), "get_file_details" | "get_directories" | "search_files"));
    match publisher {
        Some(MICROSOFT_PUBLISHER_ID) if named_like_explorer && looks_like_connector => (ServerIdentity::MicrosoftFileExplorer, false),
        Some(MICROSOFT_PUBLISHER_ID) => (ServerIdentity::MicrosoftPublished, false),
        Some(_) => (ServerIdentity::ThirdParty, named_like_explorer),
        None => (ServerIdentity::Unverified, named_like_explorer),
    }
}

fn parse_entry(v: &Value) -> Option<RegisteredServer> {
    if !v.is_object() {
        return None;
    }
    let name = ["/server/name", "/manifest/name", "/name", "/displayName", "/display_name", "/title"].iter().find_map(|p| v.pointer(p).and_then(|x| x.as_str())).map(|n| clean(n, 128)).filter(|n| !n.is_empty())?;
    let family = meta_block(v).and_then(|m| m.get("package_family_name")).and_then(|x| x.as_str()).or_else(|| s(v, &["package_family_name"])).filter(|x| valid_identifier(x)).map(str::to_string);
    let identifier = ["/server/packages/0/identifier", "/packages/0/identifier", "/manifest/server/packages/0/identifier", "/identifier", "/id", "/server_id", "/serverId"]
        .iter()
        .find_map(|p| v.pointer(p).and_then(|x| x.as_str()))
        .filter(|x| valid_identifier(x))
        .map(str::to_string);
    let tools = declared_tools(v);
    let (identity, impersonation_warning) = classify_identity(&name, identifier.as_deref(), family.as_deref(), &tools);
    let manifest_command = v.pointer("/manifest/server/mcp_config/command").or_else(|| v.pointer("/server/mcp_config/command")).and_then(|c| c.as_str()).is_some();
    let connection = if identifier.is_some() {
        ConnectionMethod::OdrProxy
    } else if manifest_command {
        ConnectionMethod::ManifestCommandOnly
    } else {
        ConnectionMethod::Unavailable
    };
    let slug: String = name.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect();
    Some(RegisteredServer {
        id: identifier.clone().unwrap_or(slug),
        description: ["/server/description", "/manifest/description", "/description"].iter().find_map(|p| v.pointer(p).and_then(|x| x.as_str())).map(|d| clean(d, 400)),
        version: ["/server/version", "/manifest/version", "/version"].iter().find_map(|p| v.pointer(p).and_then(|x| x.as_str())).map(|d| clean(d, 40)),
        package_family_name: family,
        identifier,
        identity,
        impersonation_warning,
        connection,
        declared_tools: tools,
        name,
    })
}

fn locate(value: &Value, depth: u8) -> Result<(RegistryFormat, Vec<Value>), String> {
    if let Some(servers) = value.pointer("/structuredContent/servers").and_then(|s| s.as_array()) {
        return Ok((RegistryFormat::StructuredContent, servers.clone()));
    }
    if value.get("isError").and_then(|e| e.as_bool()) == Some(true) {
        let text = value.pointer("/content/0/text").and_then(|t| t.as_str()).unwrap_or("odr.exe reported an error");
        return Err(format!("odr.exe reported an error: {}", clean(text, 200)));
    }
    if depth == 0 {
        if let Some(parts) = value.get("content").and_then(|c| c.as_array()) {
            for part in parts.iter().take(4) {
                if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                    if let Ok(inner) = serde_json::from_str::<Value>(text) {
                        if let Ok((_, servers)) = locate(&inner, 1) {
                            return Ok((RegistryFormat::TextContent, servers));
                        }
                    }
                }
            }
        }
    }
    match value {
        Value::Array(a) => Ok((RegistryFormat::Array, a.clone())),
        Value::Object(o) => ["servers", "mcp_servers", "mcpServers", "items"]
            .iter()
            .find_map(|k| o.get(*k).and_then(|v| v.as_array()))
            .map(|a| (RegistryFormat::ServersObject, a.clone()))
            .ok_or_else(|| format!("unrecognised odr.exe output (top-level keys: {})", o.keys().take(8).map(|k| clean(k, 32)).collect::<Vec<_>>().join(", "))),
        _ => Err("unrecognised odr.exe output (not a JSON object or array)".into()),
    }
}

/// Parses `odr.exe list` / `odr.exe mcp list` output.
pub fn parse_registry(stdout: &str) -> Result<ParsedRegistry, String> {
    let trimmed = stdout.trim().trim_start_matches('\u{feff}');
    let value: Value = serde_json::from_str(trimmed).map_err(|e| format!("odr.exe did not print JSON ({e})"))?;
    let (format, entries) = locate(&value, 0)?;
    let mut notes = Vec::new();
    if entries.len() > MAX_SERVERS {
        notes.push(format!("The registry lists {} servers; only the first {MAX_SERVERS} are shown.", entries.len()));
    }
    let mut servers: Vec<RegisteredServer> = entries.iter().take(MAX_SERVERS).filter_map(parse_entry).collect();
    if servers.len() < entries.len().min(MAX_SERVERS) {
        notes.push("Some registry entries had no usable name and were skipped.".into());
    }
    // Two entries with the same id are a spoofing signal: trust neither.
    let mut seen = std::collections::HashMap::<String, usize>::new();
    for sv in &servers {
        *seen.entry(sv.id.clone()).or_default() += 1;
    }
    for sv in servers.iter_mut().filter(|sv| seen[&sv.id] > 1) {
        sv.identity = ServerIdentity::Unverified;
        sv.impersonation_warning = true;
        sv.connection = ConnectionMethod::Unavailable;
    }
    if seen.values().any(|n| *n > 1) {
        notes.push("Several registry entries share the same identifier; WinClean trusts none of them.".into());
    }
    Ok(ParsedRegistry { format, servers, notes })
}

#[cfg(test)]
pub(crate) mod fixtures {
    pub const FILE_EXPLORER_PFN: &str = "MicrosoftWindows.Client.FileExplorer_cw5n1h2txyewy";

    /// Shape read by Microsoft's mcp-client-js sample (`structuredContent.servers`).
    pub fn structured_content() -> String {
        format!(
            r#"{{"structuredContent":{{"servers":[
              {{"server":{{"name":"Windows File Explorer","description":"Access files","version":"1.0.0","packages":[{{"registryType":"msix","identifier":"{FILE_EXPLORER_PFN}","transport":{{"type":"stdio"}}}}]}},
                "_meta":{{"com.microsoft.windows":{{"package_family_name":"{FILE_EXPLORER_PFN}","static_responses":{{
                  "initialize":{{"protocolVersion":"2025-06-18","serverInfo":{{"name":"FileExplorer","version":"1"}}}},
                  "tools/list":{{"tools":[
                    {{"name":"get_file_details","description":"Metadata","inputSchema":{{"type":"object","properties":{{"path":{{"type":"string"}}}},"required":["path"]}}}},
                    {{"name":"get_directories","inputSchema":{{"type":"object","properties":{{"path":{{"type":"string"}}}}}}}},
                    {{"name":"search_files","description":"Searches files; semantic search on Copilot+ PCs","inputSchema":{{"type":"object","properties":{{"startingDir":{{"type":"string"}},"searchPatterns":{{"type":"string"}},"specifiedFileTypes":{{"type":"array","items":{{"type":"string"}}}}}},"required":["startingDir","searchPatterns"]}}}},
                    {{"name":"move_file"}},{{"name":"read_file"}}]}}}}}}}}}},
              {{"server":{{"name":"Sample App","packages":[{{"identifier":"Contoso.Sample_8wekyb3d8bbwe"}}]}}}}
            ]}}}}"#
        )
    }

    /// Shape in the Learn quickstart: a list of manifest-based entries.
    pub fn manifest_array() -> String {
        r#"[{"manifest":{"manifest_version":"0.1","name":"MCP-Server-Sample-App","version":"1.0.0","description":"A Sample App MCP Server","author":{"name":"Microsoft"},
            "server":{"type":"binary","entry_point":"S.exe","mcp_config":{"command":"S.exe","args":[]}},"tools":[{"name":"get_weather"}],
            "_meta":{"com.microsoft.windows":{"package_family_name":"Contoso.Sample_8wekyb3d8bbwe","static_responses":{"tools/list":{"tools":[{"name":"get_weather","inputSchema":{"type":"object","properties":{"city":{"type":"string"}}}}]}}}}}}]"#
            .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::*;
    use super::*;

    #[test]
    fn the_structured_content_shape_of_the_official_sample_is_parsed_with_nested_metadata() {
        let r = parse_registry(&structured_content()).unwrap();
        assert_eq!(r.format, RegistryFormat::StructuredContent);
        assert_eq!(r.servers.len(), 2);
        let fe = &r.servers[0];
        assert_eq!(fe.name, "Windows File Explorer");
        assert_eq!(fe.identifier.as_deref(), Some(FILE_EXPLORER_PFN));
        assert_eq!(fe.id, FILE_EXPLORER_PFN);
        assert_eq!(fe.identity, ServerIdentity::MicrosoftFileExplorer);
        assert_eq!(fe.connection, ConnectionMethod::OdrProxy);
        assert_eq!(fe.version.as_deref(), Some("1.0.0"));
        let names: Vec<_> = fe.declared_tools.iter().map(|t| (t.name.as_str(), t.policy)).collect();
        assert_eq!(
            names,
            vec![
                ("get_file_details", ToolPolicy::MetadataOnly),
                ("get_directories", ToolPolicy::MetadataOnly),
                ("search_files", ToolPolicy::ConsentedSearch),
                ("move_file", ToolPolicy::Destructive),
                ("read_file", ToolPolicy::ContentRead)
            ]
        );
        assert!(fe.declared_tools[2].input_schema.has("startingDir") && fe.declared_tools[2].input_schema.required.len() == 2);
        assert_eq!(r.servers[1].identity, ServerIdentity::ThirdParty);
        assert!(!r.servers[1].impersonation_warning);
    }

    #[test]
    fn the_learn_quickstart_manifest_shape_is_parsed_and_never_becomes_a_runnable_command() {
        let r = parse_registry(&manifest_array()).unwrap();
        assert_eq!(r.format, RegistryFormat::Array);
        let s = &r.servers[0];
        assert_eq!(s.name, "MCP-Server-Sample-App");
        assert_eq!(s.package_family_name.as_deref(), Some("Contoso.Sample_8wekyb3d8bbwe"));
        assert_eq!(s.declared_tools[0].name, "get_weather");
        assert!(s.declared_tools[0].input_schema.has("city"));
        // No package identifier: the only way in would be the manifest command, which WinClean refuses to run.
        assert!(s.identifier.is_none());
        assert_eq!(s.connection, ConnectionMethod::ManifestCommandOnly);
    }

    #[test]
    fn text_content_and_wrapped_objects_are_recognised() {
        let inner = structured_content();
        let wrapped = serde_json::json!({ "content": [ { "type": "text", "text": inner } ] }).to_string();
        let r = parse_registry(&wrapped).unwrap();
        assert_eq!(r.format, RegistryFormat::TextContent);
        assert_eq!(r.servers.len(), 2);
        let obj = r#"{"servers":[{"name":"Windows Settings","description":"settings"}]}"#;
        let r = parse_registry(obj).unwrap();
        assert_eq!(r.format, RegistryFormat::ServersObject);
        assert_eq!(r.servers[0].connection, ConnectionMethod::Unavailable);
        assert_eq!(r.servers[0].identity, ServerIdentity::Unverified);
    }

    #[test]
    fn a_server_that_only_calls_itself_file_explorer_is_not_trusted() {
        let fake = r#"[{"server":{"name":"File Explorer","packages":[{"identifier":"Evil.FileExplorer_8wekyb3d8bbwe"}]}},
                       {"server":{"name":"Windows File Explorer"}},
                       {"server":{"name":"File Explorer","packages":[{"identifier":"Fake.Thing_cw5n1h2txyew"}]}}]"#;
        let r = parse_registry(fake).unwrap();
        assert_eq!(r.servers[0].identity, ServerIdentity::ThirdParty);
        assert!(r.servers[0].impersonation_warning);
        assert_eq!(r.servers[1].identity, ServerIdentity::Unverified, "no publisher information at all");
        assert!(r.servers[1].impersonation_warning);
        assert_eq!(r.servers[2].identity, ServerIdentity::Unverified, "a malformed publisher id is not a publisher");
        assert!(r.servers.iter().all(|s| s.identity != ServerIdentity::MicrosoftFileExplorer));
    }

    #[test]
    fn a_microsoft_publisher_with_the_wrong_tools_is_not_the_file_explorer_connector() {
        let v = r#"[{"server":{"name":"Windows File Explorer","packages":[{"identifier":"X.FileExplorer_cw5n1h2txyewy"}]},
                     "_meta":{"com.microsoft.windows":{"static_responses":{"tools/list":{"tools":[{"name":"run_anything"}]}}}}}]"#;
        assert_eq!(parse_registry(v).unwrap().servers[0].identity, ServerIdentity::MicrosoftPublished);
    }

    #[test]
    fn duplicated_ids_are_a_spoofing_signal_and_lose_all_trust() {
        let dup = format!(
            r#"[{{"server":{{"name":"Windows File Explorer","packages":[{{"identifier":"{FILE_EXPLORER_PFN}"}}]}}}},{{"server":{{"name":"Windows File Explorer","packages":[{{"identifier":"{FILE_EXPLORER_PFN}"}}]}}}}]"#
        );
        let r = parse_registry(&dup).unwrap();
        assert!(r.servers.iter().all(|s| s.identity == ServerIdentity::Unverified && s.connection == ConnectionMethod::Unavailable));
        assert!(r.notes.iter().any(|n| n.contains("share the same identifier")));
    }

    #[test]
    fn identifiers_that_could_act_as_command_line_options_are_dropped() {
        for bad in ["--proxy", "-x", "a b", "a;b", "a&b", "$(x)", "a\"b", "", &"a".repeat(200), ".hidden", "a/b", "a\\b"] {
            assert!(!valid_identifier(bad), "{bad:?}");
            let v = serde_json::json!([{ "server": { "name": "S", "packages": [ { "identifier": bad } ] } }]).to_string();
            assert_eq!(parse_registry(&v).unwrap().servers[0].identifier, None, "{bad:?}");
        }
        assert!(valid_identifier("Microsoft.Windows.Foo_1.0_cw5n1h2txyewy"));
    }

    #[test]
    fn invalid_and_malicious_output_is_reported_not_guessed() {
        assert!(parse_registry("Usage: odr [command]").unwrap_err().contains("did not print JSON"));
        assert!(parse_registry("").unwrap_err().contains("did not print JSON"));
        assert!(parse_registry("42").unwrap_err().contains("unrecognised"));
        assert!(parse_registry(r#"{"unexpected":{}}"#).unwrap_err().contains("top-level keys: unexpected"));
        assert!(parse_registry(r#"{"isError":true,"content":[{"type":"text","text":"agent user missing"}]}"#).unwrap_err().contains("agent user missing"));
        let deep = format!("{}{}", "[".repeat(500), "]".repeat(500));
        assert!(parse_registry(&deep).is_err(), "pathological nesting is an error, not a stack overflow");
        assert_eq!(parse_registry("\u{feff}[]").unwrap().servers.len(), 0);
        assert_eq!(parse_registry(r#"[1,"x",null,{"nothing":true}]"#).unwrap().servers.len(), 0);
    }

    #[test]
    fn sizes_and_text_are_bounded_and_cleaned() {
        let many: Vec<_> = (0..500).map(|i| serde_json::json!({ "server": { "name": format!("S{i}") } })).collect();
        let r = parse_registry(&serde_json::to_string(&many).unwrap()).unwrap();
        assert_eq!(r.servers.len(), 200);
        assert!(r.notes.iter().any(|n| n.contains("only the first 200")));
        let nasty = serde_json::json!([{ "server": { "name": "Fi\u{202E}le\u{0}\nExplorer", "description": "d".repeat(5000) } }]).to_string();
        let s = &parse_registry(&nasty).unwrap().servers[0];
        assert_eq!(s.name, "FileExplorer");
        assert_eq!(s.description.as_ref().unwrap().len(), 400);
        let tools: Vec<_> = (0..300).map(|i| serde_json::json!({ "name": format!("t{i}") })).collect();
        let t = serde_json::json!([{ "tools": tools, "name": "x" }]).to_string();
        assert_eq!(parse_registry(&t).unwrap().servers[0].declared_tools.len(), 100);
        let bad_tool = serde_json::json!([{ "name": "x", "tools": [ { "name": "a b" }, { "name": "ok_tool" }, { "name": "$(x)" } ] }]).to_string();
        assert_eq!(parse_registry(&bad_tool).unwrap().servers[0].declared_tools.len(), 1);
    }
}
