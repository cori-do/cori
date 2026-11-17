// SPDX-License-Identifier: Apache-2.0
//! JSON Schema (draft 7 / 2020-12 subset, what MCP servers publish) → Zod v4 source text.
use serde_json::Value;

pub fn zod(schema: &Value) -> String { emit(schema, 0) }

fn emit(s: &Value, depth: usize) -> String {
    if depth > 12 { return "z.unknown()".into(); }
    let nullable = s.get("nullable").and_then(|b| b.as_bool()).unwrap_or(false);
    let mut out = if let Some(e) = s.get("enum").and_then(|e| e.as_array()) {
        if e.iter().all(|v| v.is_string()) { format!("z.enum([{}])", e.iter().map(|v| format!("{:?}", v.as_str().unwrap())).collect::<Vec<_>>().join(", ")) }
        else { format!("z.union([{}])", e.iter().map(|v| format!("z.literal({})", v)).collect::<Vec<_>>().join(", ")) }
    } else if let Some(c) = s.get("const") { format!("z.literal({c})") }
    else if let Some(any) = s.get("anyOf").or_else(|| s.get("oneOf")).and_then(|a| a.as_array()) {
        let parts: Vec<String> = any.iter().map(|x| emit(x, depth + 1)).collect();
        if parts.len() == 1 { parts[0].clone() } else { format!("z.union([{}])", parts.join(", ")) }
    } else if let Some(all) = s.get("allOf").and_then(|a| a.as_array()) {
        let parts: Vec<String> = all.iter().map(|x| emit(x, depth + 1)).collect();
        if parts.len() == 1 { parts[0].clone() } else { format!("z.intersection({})", parts.join(", ")) }
    } else {
        let ty = match s.get("type") { Some(Value::String(t)) => t.clone(), Some(Value::Array(a)) => { let non_null: Vec<&str> = a.iter().filter_map(|t| t.as_str()).filter(|t| *t != "null").collect(); if non_null.len() == 1 { non_null[0].to_string() } else { "mixed".into() } } _ => if s.get("properties").is_some() { "object".into() } else { "unknown".into() } };
        let null_in_type = matches!(s.get("type"), Some(Value::Array(a)) if a.iter().any(|t| t.as_str() == Some("null")));
        let base = match ty.as_str() {
            "string" => { let mut z = "z.string()".to_string(); if let Some(f) = s.get("format").and_then(|f| f.as_str()) { match f { "email" => z.push_str(".email()"), "uri" | "url" => z.push_str(".url()"), "uuid" => z.push_str(".uuid()"), "date-time" => z.push_str(".datetime()"), _ => {} } } z }
            "integer" => "z.number().int()".into(),
            "number" => "z.number()".into(),
            "boolean" => "z.boolean()".into(),
            "null" => "z.null()".into(),
            "array" => format!("z.array({})", s.get("items").map(|i| emit(i, depth + 1)).unwrap_or_else(|| "z.unknown()".into())),
            "object" => {
                let req: Vec<&str> = s.get("required").and_then(|r| r.as_array()).map(|a| a.iter().filter_map(|x| x.as_str()).collect()).unwrap_or_default();
                match s.get("properties").and_then(|p| p.as_object()) {
                    Some(props) if !props.is_empty() => {
                        let fields: Vec<String> = props.iter().map(|(k, v)| { let key = if k.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '$') && !k.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(true) { k.clone() } else { format!("{k:?}") }; let mut z = emit(v, depth + 1); if !req.contains(&k.as_str()) { z.push_str(".optional()"); } format!("{}{key}: {z}", "  ".repeat(depth + 1)) }).collect();
                        let obj = format!("z.object({{\n{}\n{}}})", fields.join(",\n"), "  ".repeat(depth));
                        if s.get("additionalProperties").map(|a| a != &Value::Bool(false)).unwrap_or(false) { format!("{obj}.passthrough()") } else { obj }
                    }
                    _ => match s.get("additionalProperties") { Some(ap) if ap.is_object() => format!("z.record(z.string(), {})", emit(ap, depth + 1)), _ => "z.record(z.string(), z.unknown())".into() },
                }
            }
            _ => "z.unknown()".into(),
        };
        if null_in_type { format!("{base}.nullable()") } else { base }
    };
    if nullable && !out.ends_with(".nullable()") { out.push_str(".nullable()"); }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn objects_and_optionals() {
        let s = serde_json::json!({ "type": "object", "properties": { "id": { "type": "string" }, "n": { "type": "integer" }, "tags": { "type": "array", "items": { "type": "string" } }, "kind": { "enum": ["a", "b"] } }, "required": ["id"] });
        let z = zod(&s);
        assert!(z.contains("id: z.string()"), "{z}"); assert!(z.contains("n: z.number().int().optional()"), "{z}"); assert!(z.contains("tags: z.array(z.string()).optional()"), "{z}"); assert!(z.contains("kind: z.enum([\"a\", \"b\"]).optional()"), "{z}");
    }
}
