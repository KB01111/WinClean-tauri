//! The subset of JSON Schema WinClean needs to validate MCP tool arguments.
//!
//! The schema is whatever the connector reported in `tools/list`; it is
//! untrusted input itself, so parsing is tolerant, bounded and never panics.
//! Validation is deliberately stricter than JSON Schema's default: arguments
//! the schema does not declare are always refused.

use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::Value;

const MAX_PROPERTIES: usize = 64;
const MAX_ENUM: usize = 64;
const MAX_STRING_ARG: usize = 2048;
const MAX_ARRAY_ARG: usize = 32;

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PropertySchema {
    /// JSON types the property accepts (`string`, `integer`, `array`, …).
    pub kinds: Vec<String>,
    pub description: Option<String>,
    pub enum_values: Vec<String>,
    /// For arrays: the accepted item types.
    pub item_kinds: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InputSchema {
    /// False when the server reported no usable schema at all.
    pub declared: bool,
    pub properties: BTreeMap<String, PropertySchema>,
    pub required: Vec<String>,
}

fn kinds_of(v: &Value) -> Vec<String> {
    let mut out = Vec::new();
    let mut push = |t: &Value| {
        if let Some(s) = t.as_str() {
            if out.len() < 8 && !out.iter().any(|k: &String| k == s) {
                out.push(s.to_string());
            }
        }
    };
    match v.get("type") {
        Some(Value::Array(a)) => a.iter().for_each(&mut push),
        Some(t) => push(t),
        None => {}
    }
    for key in ["anyOf", "oneOf"] {
        if let Some(Value::Array(alts)) = v.get(key) {
            for alt in alts.iter().take(8) {
                if let Some(t) = alt.get("type") {
                    push(t);
                }
            }
        }
    }
    out
}

fn clean(s: &str, max: usize) -> String {
    s.chars().filter(|c| !c.is_control()).take(max).collect()
}

impl InputSchema {
    pub fn from_value(v: &Value) -> Self {
        let Some(obj) = v.as_object() else { return Self::default() };
        let mut schema = InputSchema { declared: true, ..Default::default() };
        if let Some(props) = obj.get("properties").and_then(|p| p.as_object()) {
            for (name, p) in props.iter().take(MAX_PROPERTIES) {
                let items = p.get("items").map(kinds_of).unwrap_or_default();
                let enum_values = p
                    .get("enum")
                    .and_then(|e| e.as_array())
                    .map(|a| a.iter().filter_map(|x| x.as_str()).take(MAX_ENUM).map(|s| clean(s, 64)).collect())
                    .unwrap_or_default();
                schema.properties.insert(
                    clean(name, 64),
                    PropertySchema {
                        kinds: kinds_of(p),
                        description: p.get("description").and_then(|d| d.as_str()).map(|d| clean(d, 300)),
                        enum_values,
                        item_kinds: items,
                    },
                );
            }
        }
        if let Some(req) = obj.get("required").and_then(|r| r.as_array()) {
            schema.required = req.iter().filter_map(|r| r.as_str()).take(MAX_PROPERTIES).map(|s| clean(s, 64)).collect();
        }
        schema
    }

    pub fn has(&self, key: &str) -> bool {
        self.properties.contains_key(key)
    }

    pub fn property(&self, key: &str) -> Option<&PropertySchema> {
        self.properties.get(key)
    }

    pub fn property_names(&self) -> Vec<String> {
        self.properties.keys().cloned().collect()
    }
}

impl PropertySchema {
    pub fn accepts(&self, kind: &str) -> bool {
        // An empty type list means the server did not constrain the value.
        self.kinds.is_empty() || self.kinds.iter().any(|k| k == kind || (kind == "integer" && k == "number"))
    }
}

fn kind_matches(kind: &str, value: &Value) -> bool {
    match kind {
        "string" => value.is_string(),
        "integer" => value.is_i64() || value.is_u64(),
        "number" => value.is_number(),
        "boolean" => value.is_boolean(),
        "array" => value.is_array(),
        "object" => value.is_object(),
        "null" => value.is_null(),
        _ => false,
    }
}

/// Validates `args` against the connector's own schema. Unknown keys, missing
/// required keys, wrong types and oversized values are all refused.
pub fn validate_args(schema: &InputSchema, args: &Value) -> Result<(), String> {
    if !schema.declared {
        return Err("the connector did not report an input schema for this tool, so WinClean cannot validate a call".into());
    }
    let obj = args.as_object().ok_or("arguments must be an object")?;
    for (key, value) in obj {
        let Some(prop) = schema.properties.get(key) else {
            return Err(format!("argument '{key}' is not part of the tool's schema"));
        };
        if !prop.kinds.is_empty() && !prop.kinds.iter().any(|k| kind_matches(k, value)) {
            return Err(format!("argument '{key}' must be {}", prop.kinds.join(" or ")));
        }
        if let Some(s) = value.as_str() {
            if s.chars().count() > MAX_STRING_ARG || s.chars().any(|c| c == '\0') {
                return Err(format!("argument '{key}' is too long or contains NUL"));
            }
            if !prop.enum_values.is_empty() && !prop.enum_values.iter().any(|e| e == s) {
                return Err(format!("argument '{key}' must be one of {}", prop.enum_values.join(", ")));
            }
        }
        if let Some(items) = value.as_array() {
            if items.len() > MAX_ARRAY_ARG {
                return Err(format!("argument '{key}' has too many items"));
            }
            if !prop.item_kinds.is_empty() && !items.iter().all(|i| prop.item_kinds.iter().any(|k| kind_matches(k, i))) {
                return Err(format!("argument '{key}' has items of the wrong type"));
            }
        }
    }
    for req in &schema.required {
        if !obj.contains_key(req) {
            return Err(format!("missing required argument '{req}'"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn search_schema() -> InputSchema {
        InputSchema::from_value(&json!({
            "type": "object",
            "properties": {
                "startingDir": { "type": "string", "description": "Where to look" },
                "searchPatterns": { "type": "string" },
                "specifiedFileTypes": { "type": "array", "items": { "type": "string" } },
                "startDate": { "type": ["string", "null"] }
            },
            "required": ["startingDir", "searchPatterns"]
        }))
    }

    #[test]
    fn schema_is_read_from_the_connector_including_type_unions_and_items() {
        let s = search_schema();
        assert!(s.declared && s.has("startingDir") && !s.has("path"));
        assert_eq!(s.required, vec!["startingDir", "searchPatterns"]);
        assert!(s.property("startDate").unwrap().accepts("string"));
        assert_eq!(s.property("specifiedFileTypes").unwrap().item_kinds, vec!["string"]);
        assert!(InputSchema::from_value(&json!("nonsense")).declared == false);
        assert_eq!(InputSchema::from_value(&json!({ "type": "object" })).properties.len(), 0);
    }

    #[test]
    fn valid_calls_pass_and_every_kind_of_invalid_call_is_refused() {
        let s = search_schema();
        assert!(validate_args(&s, &json!({ "startingDir": "C:\\Users\\me\\Downloads", "searchPatterns": "*.iso" })).is_ok());
        assert!(validate_args(&s, &json!({ "startingDir": "C:\\a", "searchPatterns": "x", "specifiedFileTypes": ["iso"] })).is_ok());
        assert!(validate_args(&s, &json!({ "startingDir": "C:\\a" })).unwrap_err().contains("missing required"));
        assert!(validate_args(&s, &json!({ "startingDir": "C:\\a", "searchPatterns": "x", "extra": 1 })).unwrap_err().contains("not part"));
        assert!(validate_args(&s, &json!({ "startingDir": 5, "searchPatterns": "x" })).unwrap_err().contains("must be string"));
        assert!(validate_args(&s, &json!({ "startingDir": "C:\\a", "searchPatterns": "x", "specifiedFileTypes": [1] })).is_err());
        assert!(validate_args(&s, &json!({ "startingDir": "C:\\a", "searchPatterns": "x".repeat(3000) })).is_err());
        assert!(validate_args(&s, &json!({ "startingDir": "C:\\a\u{0}", "searchPatterns": "x" })).is_err());
        assert!(validate_args(&s, &json!(["x"])).is_err());
        assert!(validate_args(&InputSchema::default(), &json!({})).unwrap_err().contains("did not report"));
    }

    #[test]
    fn hostile_schemas_are_bounded() {
        let mut props = serde_json::Map::new();
        for i in 0..500 {
            props.insert(format!("p{i}"), json!({ "type": "string", "description": "d".repeat(10_000) }));
        }
        let s = InputSchema::from_value(&json!({ "properties": props }));
        assert!(s.properties.len() <= 64);
        assert!(s.properties.values().all(|p| p.description.as_ref().unwrap().len() <= 300));
    }
}
