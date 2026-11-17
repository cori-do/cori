// SPDX-License-Identifier: Apache-2.0
//! JSON Schema → TypeScript type text, for the generated context (the tool inputs of declared connectors) and a front's
//! typed client (`cori gen client`, from the steps' Zod output schemas).
use serde_json::Value;

/// How the generated text types arrays: `readonly (T)[]` in the context steps read, `(T)[]` in a front's client (whose
/// code may sort or fill them).
#[derive(Debug, Clone, Copy)]
pub struct Style { pub readonly_arrays: bool }

/// The context's style (read-only arrays).
pub fn ts(schema: &Value) -> String { ts_with(schema, Style { readonly_arrays: true }) }

pub fn ts_with(schema: &Value, style: Style) -> String { emit(schema, 0, style) }

/// A property name as TypeScript takes it: bare when it is an identifier, a quoted string otherwise.
pub fn key(k: &str) -> String {
    let ident = !k.is_empty() && k.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '$') && !k.starts_with(|c: char| c.is_ascii_digit());
    if ident { k.to_string() } else { serde_json::to_string(k).unwrap_or_default() }
}

fn emit(s: &Value, depth: usize, style: Style) -> String {
    if depth > 12 { return "unknown".into(); }
    let union = |xs: Vec<String>| if xs.is_empty() { "never".to_string() } else { xs.join(" | ") };
    if let Some(e) = s.get("enum").and_then(|e| e.as_array()) { return union(e.iter().map(|v| v.to_string()).collect()); }
    if let Some(c) = s.get("const") { return c.to_string(); }
    if let Some(any) = s.get("anyOf").or_else(|| s.get("oneOf")).and_then(|a| a.as_array()) { return union(any.iter().map(|x| emit(x, depth + 1, style)).collect()); }
    if let Some(all) = s.get("allOf").and_then(|a| a.as_array()) {
        return all.iter().map(|x| { let t = emit(x, depth + 1, style); if t.contains('|') { format!("({t})") } else { t } }).collect::<Vec<_>>().join(" & ");
    }
    // `type` names one type or several (`["string", "null"]`); without it, `properties` still means an object
    let (types, nullable): (Vec<&str>, bool) = match s.get("type") {
        Some(Value::String(t)) => (vec![t.as_str()], false),
        Some(Value::Array(a)) => { let all: Vec<&str> = a.iter().filter_map(|t| t.as_str()).collect(); (all.iter().copied().filter(|t| *t != "null").collect(), all.contains(&"null")) }
        _ => (if s.get("properties").is_some() { vec!["object"] } else { vec![] }, false),
    };
    let base = if types.is_empty() { "unknown".to_string() } else { types.iter().map(|t| one(s, t, depth, style)).collect::<Vec<_>>().join(" | ") };
    if nullable || s.get("nullable").and_then(|b| b.as_bool()).unwrap_or(false) { format!("{base} | null") } else { base }
}

fn one(s: &Value, ty: &str, depth: usize, style: Style) -> String {
    match ty {
        "string" => "string".into(),
        "integer" | "number" => "number".into(),
        "boolean" => "boolean".into(),
        "null" => "null".into(),
        "array" => {
            let items = s.get("items").map(|i| emit(i, depth + 1, style)).unwrap_or_else(|| "unknown".into());
            format!("{}({items})[]", if style.readonly_arrays { "readonly " } else { "" })
        }
        "object" => {
            let req: Vec<&str> = s.get("required").and_then(|r| r.as_array()).map(|a| a.iter().filter_map(|x| x.as_str()).collect()).unwrap_or_default();
            match s.get("properties").and_then(|p| p.as_object()) {
                Some(props) if !props.is_empty() => {
                    let fields: Vec<String> = props.iter().map(|(k, v)| format!("{}{}: {}", key(k), if req.contains(&k.as_str()) { "" } else { "?" }, emit(v, depth + 1, style))).collect();
                    let open = s.get("additionalProperties").map(|a| a != &Value::Bool(false)).unwrap_or(false);
                    format!("{{ {}{} }}", fields.join("; "), if open { "; [k: string]: unknown" } else { "" })
                }
                _ => match s.get("additionalProperties") { Some(ap) if ap.is_object() => format!("Record<string, {}>", emit(ap, depth + 1, style)), _ => "Record<string, unknown>".into() },
            }
        }
        _ => "unknown".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn schemas_become_the_types_they_describe() {
        assert_eq!(ts(&json!({ "type": "array", "items": { "enum": ["a", "b"] } })), r#"readonly ("a" | "b")[]"#);
        assert_eq!(ts(&json!({ "type": ["string", "number", "null"] })), "string | number | null");
        assert_eq!(ts(&json!({ "type": "string", "nullable": true })), "string | null");
        assert_eq!(ts(&json!({ "allOf": [{ "anyOf": [{ "type": "string" }, { "type": "number" }] }, { "type": "object", "properties": { "a": { "type": "boolean" } }, "required": ["a"], "additionalProperties": false }] })), "(string | number) & { a: boolean }");
        assert_eq!(ts(&json!({ "properties": { "2fa": { "type": "string" }, "ok": { "type": "boolean" } }, "required": ["ok"] })), r#"{ "2fa"?: string; ok: boolean }"#);
        assert_eq!(ts(&json!({ "type": "object", "additionalProperties": { "type": "number" } })), "Record<string, number>");
        assert_eq!(ts(&json!({ "enum": [] })), "never");
        assert_eq!(ts_with(&json!({ "type": "array", "items": { "type": "string" } }), Style { readonly_arrays: false }), "(string)[]");
    }

    #[test]
    fn keys_are_quoted_unless_identifiers() {
        assert_eq!((key("ticket_id"), key("$ref"), key("2fa"), key("a-b"), key("")), ("ticket_id".into(), "$ref".into(), r#""2fa""#.into(), r#""a-b""#.into(), r#""""#.into()));
    }
}
