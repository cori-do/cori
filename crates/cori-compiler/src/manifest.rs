// SPDX-License-Identifier: Apache-2.0
//! Manifest v3: `workflow.yaml`, pure YAML. `steps:` is the ordered list of top-level step files (`steps/<name>.ts`).
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Parameter {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub values: Option<Vec<String>>,
    #[serde(default)]
    pub default: Option<Value>,
    #[serde(default = "default_true")]
    pub required: bool,
    /// `file` / `files`: the media types (`application/pdf`), families (`image/*`) or extensions (`.pdf`) a file may
    /// have; none: any file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accept: Option<Vec<String>>,
    /// `file` / `files`: the largest file, as `10MB`, `500KB`, `2MiB` or a number of bytes; at most the server's own limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_size: Option<Value>,
    /// `files`: how many files at most (`files::MAX_FILES_DEFAULT` when unset, `files::MAX_FILES_LIMIT` at most).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_files: Option<u64>,
}
fn default_true() -> bool { true }

impl Parameter {
    /// A parameter that takes files: `file` (one) or `files` (a list).
    pub fn is_file(&self) -> bool { matches!(self.ty.as_str(), "file" | "files") }
    /// The largest file it takes, in bytes, when it sets one (`max_size`).
    pub fn max_size_bytes(&self) -> Option<u64> { self.max_size.as_ref().and_then(|v| crate::files::parse_size(v).ok()) }
    /// How many files a `files` parameter takes at most.
    pub fn max_files(&self) -> u64 { self.max_files.unwrap_or(crate::files::MAX_FILES_DEFAULT).min(crate::files::MAX_FILES_LIMIT) }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Endpoint {
    #[serde(default = "default_auth")]
    pub auth: String,
    #[serde(default)]
    pub allow: Option<Value>,
    #[serde(default = "default_mode")]
    pub mode: String,
    #[serde(default)]
    pub timeout: Option<String>,
    /// `parameters`, `header` or `none`; empty in workflow.yaml means derived by `compile` from the effects
    #[serde(default)]
    pub idempotency: String,
    #[serde(default)]
    pub rate_limit: Option<String>,
}
fn default_auth() -> String { "sso".into() }
fn default_mode() -> String { "async".into() }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub parameters: Vec<Parameter>,
    #[serde(default)]
    pub connectors: Vec<String>,
    /// Ordered top-level steps: `steps/<name>.ts` per entry. Writes keep this order (`writes_by_declared_order`).
    /// `workflow.yaml` must list it (checked in `parse`); the default only keeps versions stored before it existed readable.
    #[serde(default)]
    pub steps: Vec<String>,
    #[serde(default)]
    pub endpoint: Option<Endpoint>,
    #[serde(default)]
    pub result: Option<Value>,
    #[serde(default)]
    pub schedule: Option<String>,
    #[serde(default)]
    pub schedule_tz: Option<String>,
    #[serde(default)]
    pub schedule_as: Option<Value>,
    #[serde(default)]
    pub budget: Option<Value>,
    #[serde(default)]
    pub parallel: Option<Value>,
    #[serde(default = "default_ordering")]
    pub ordering: String,
    #[serde(default)]
    pub redact: Vec<String>,
    #[serde(default)]
    pub placement: Option<Value>,
    #[serde(default)]
    pub tags: Vec<String>,
}
fn default_ordering() -> String { "writes_by_declared_order".into() }

pub fn parse(text: &str) -> Result<Manifest, String> {
    let text = text.trim_start_matches('\u{feff}');
    if text.trim_start().starts_with("---") { return Err("workflow.yaml is pure YAML: drop the `---` front matter fences (prose goes in README.md)".into()); }
    let mut raw: serde_yaml::Value = serde_yaml::from_str(text).map_err(|e| format!("workflow.yaml: {e}"))?;
    // `endpoint: true` expands to the safe defaults.
    if let serde_yaml::Value::Mapping(m) = &mut raw {
        let key = serde_yaml::Value::String("endpoint".into());
        if let Some(serde_yaml::Value::Bool(b)) = m.get(&key).cloned() {
            if b { m.insert(key, serde_yaml::Value::Mapping(Default::default())); } else { m.remove(&key); }
        }
        if !m.contains_key(&serde_yaml::Value::String("steps".into())) { return Err("workflow.yaml: `steps: [a, b, c]` is required (the ordered list of top-level step files, steps/<name>.ts)".into()); }
    }
    let m: Manifest = serde_yaml::from_value(raw).map_err(|e| format!("workflow.yaml: {e}"))?;
    validate(&m)?;
    Ok(m)
}

fn validate(m: &Manifest) -> Result<(), String> {
    let ok_id = !m.id.is_empty() && m.id.len() <= 64 && m.id.chars().next().unwrap().is_ascii_lowercase()
        && m.id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    if !ok_id { return Err(format!("id `{}` must be snake_case ([a-z][a-z0-9_]*, ≤ 64)", m.id)); }
    let mut seen = std::collections::HashSet::new();
    for p in &m.parameters {
        if !seen.insert(&p.name) { return Err(format!("duplicate parameter `{}`", p.name)); }
        match p.ty.as_str() {
            "string" | "number" | "boolean" | "enum" | "path" | "list" | "object" | "file" | "files" => {}
            other => return Err(format!("parameter `{}`: unknown type `{other}`", p.name)),
        }
        if p.ty == "enum" && p.values.as_ref().map_or(true, |v| v.is_empty()) { return Err(format!("parameter `{}`: enum needs `values`", p.name)); }
        validate_file_param(m, p)?;
    }
    if let Some(e) = &m.endpoint {
        if !matches!(e.auth.as_str(), "sso" | "service" | "public" | "none") { return Err(format!("endpoint.auth `{}` unknown", e.auth)); }
        if !matches!(e.mode.as_str(), "sync" | "async") { return Err(format!("endpoint.mode `{}` unknown", e.mode)); }
    }
    if !matches!(m.ordering.as_str(), "writes_by_declared_order" | "dataflow_only") { return Err(format!("ordering `{}` unknown (writes_by_declared_order | dataflow_only)", m.ordering)); }
    let mut seen_steps = std::collections::HashSet::new();
    for st in &m.steps {
        let ok = !st.is_empty() && st.chars().next().unwrap().is_ascii_lowercase() && st.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
        if !ok { return Err(format!("steps: `{st}` must be a snake_case step name (the file is steps/{st}.ts)")); }
        if !seen_steps.insert(st) { return Err(format!("steps: `{st}` is listed twice")); }
    }
    Ok(())
}

/// A `file` / `files` parameter's own fields, and those only it may have.
fn validate_file_param(m: &Manifest, p: &Parameter) -> Result<(), String> {
    if !p.is_file() {
        if p.accept.is_some() || p.max_size.is_some() || p.max_files.is_some() { return Err(format!("parameter `{}`: `accept`, `max_size` and `max_files` are for `file` and `files` parameters", p.name)); }
        return Ok(());
    }
    if p.default.is_some() { return Err(format!("parameter `{}`: a `{}` parameter has no `default` (each run is given its own files)", p.name, p.ty)); }
    if p.required && m.schedule.is_some() { return Err(format!("parameter `{}`: a scheduled workflow starts with no one to upload a file; make it `required: false`", p.name)); }
    if let Some(a) = &p.accept {
        if a.is_empty() { return Err(format!("parameter `{}`: `accept` lists no type (leave it out to take any file)", p.name)); }
        for e in a { crate::files::check_accept_entry(e).map_err(|e| format!("parameter `{}`: accept: {e}", p.name))?; }
    }
    if let Some(v) = &p.max_size { crate::files::parse_size(v).map_err(|e| format!("parameter `{}`: max_size: {e}", p.name))?; }
    if let Some(n) = p.max_files {
        if p.ty != "files" { return Err(format!("parameter `{}`: `max_files` is for `files` parameters (a list)", p.name)); }
        if n == 0 || n > crate::files::MAX_FILES_LIMIT { return Err(format!("parameter `{}`: max_files is 1 to {}", p.name, crate::files::MAX_FILES_LIMIT)); }
    }
    Ok(())
}

/// Run parameters against the manifest: fills defaults, rejects a missing required one, type-checks every declared one
/// (undeclared names pass through untouched). `null` counts as absent. Shared by `cori run`, the server and the MCP surface.
pub fn validate_params(params: &mut Value, parameters: &[Parameter]) -> Result<(), String> {
    let obj = params.as_object_mut().ok_or("parameters must be a JSON object")?;
    for p in parameters {
        let v = match obj.get(&p.name).filter(|v| !v.is_null()) {
            Some(v) => v,
            None => match &p.default { Some(d) => { obj.insert(p.name.clone(), d.clone()); continue } None if p.required => return Err(format!("missing parameter `{}`", p.name)), None => continue },
        };
        let ok = match p.ty.as_str() {
            "string" | "path" => v.is_string(),
            "number" => v.is_number(),
            "boolean" => v.is_boolean(),
            "enum" => v.as_str().is_some_and(|s| p.values.as_ref().is_some_and(|vs| vs.iter().any(|x| x == s))),
            "list" => v.as_array().is_some_and(|a| a.iter().all(|x| x.is_string())),
            "object" => v.is_object(),
            "file" => file_value(v),
            "files" => v.as_array().is_some_and(|a| a.iter().all(file_value)),
            _ => true,
        };
        if !ok {
            let want = match p.ty.as_str() { "enum" => format!("one of {}", p.values.as_deref().unwrap_or_default().iter().map(|x| format!("{x:?}")).collect::<Vec<_>>().join(", ")), "list" => "a list of strings".into(), "object" => "an object".into(), "path" | "string" => "a string".into(),
                "file" => "a file: the id an upload gave (`POST /v1/apps/{app}/files`), or its reference `{ id, … }`".into(), "files" => "a list of files: ids uploads gave, or their references".into(), t => format!("a {t}") };
            return Err(format!("parameter `{}` must be {want}, got {}", p.name, short(v)));
        }
        if p.ty == "files" && let Some(n) = v.as_array().map(|a| a.len()) && n as u64 > p.max_files() { return Err(format!("parameter `{}` takes at most {} files, got {n}", p.name, p.max_files())); }
    }
    Ok(())
}

/// A file parameter's value as a run takes it: an id, or an object with one.
fn file_value(v: &Value) -> bool { crate::files::id_of(v).is_some_and(|id| !id.is_empty()) }

/// A value for an error message: at most 120 characters.
fn short(v: &Value) -> String { let s = v.to_string(); if s.chars().count() > 120 { s.chars().take(120).collect::<String>() + "…" } else { s } }

/// TypeScript type for a parameter, used in the generated context.
pub fn ts_type(p: &Parameter) -> String {
    match p.ty.as_str() {
        "string" | "path" => "string".into(),
        "number" => "number".into(),
        "boolean" => "boolean".into(),
        "enum" => p.values.as_ref().unwrap().iter().map(|v| format!("{:?}", v)).collect::<Vec<_>>().join(" | "),
        "list" => "readonly string[]".into(),
        "object" => "Record<string, unknown>".into(),
        "file" => "CoriFile".into(),
        "files" => "readonly CoriFile[]".into(),
        _ => "unknown".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn param(name: &str, ty: &str, required: bool, default: Option<Value>) -> Parameter { Parameter { name: name.into(), ty: ty.into(), description: None, values: (ty == "enum").then(|| vec!["low".into(), "high".into()]), default, required, accept: None, max_size: None, max_files: None } }

    #[test]
    fn validate_params_types_defaults_and_required() {
        use serde_json::json;
        let ps = vec![param("title", "string", true, None), param("done", "boolean", false, Some(json!(false))), param("n", "number", false, None), param("prio", "enum", false, None), param("tags", "list", false, None), param("meta", "object", false, None)];
        let mut ok = json!({ "title": "x", "prio": "high", "tags": ["a"], "meta": {}, "extra": 1 });
        validate_params(&mut ok, &ps).unwrap();
        assert_eq!(ok["done"], json!(false));
        assert_eq!(ok["extra"], json!(1));
        for (bad, needle) in [(json!({ "title": "x", "done": "maybe" }), "`done` must be a boolean"), (json!({}), "missing parameter `title`"), (json!({ "title": null }), "missing parameter `title`"), (json!({ "title": 3 }), "`title` must be a string"),
                              (json!({ "title": "x", "n": "1" }), "`n` must be a number"), (json!({ "title": "x", "prio": "mid" }), "`prio` must be one of \"low\", \"high\""), (json!({ "title": "x", "tags": [1] }), "list of strings"), (json!({ "title": "x", "meta": [] }), "an object")] {
            let mut b = bad; let e = validate_params(&mut b, &ps).unwrap_err();
            assert!(e.contains(needle), "{e}");
        }
        assert!(validate_params(&mut json!([]), &ps).is_err());
    }

    #[test]
    fn file_parameters_take_ids_or_references() {
        use serde_json::json;
        let mut one = param("invoice", "file", true, None);
        one.accept = Some(vec!["application/pdf".into()]);
        let mut many = param("attachments", "files", false, None);
        many.max_files = Some(2);
        let ps = vec![one, many];
        let mut ok = json!({ "invoice": "file_abc", "attachments": [{ "id": "file_1", "name": "a.pdf" }, "file_2"] });
        validate_params(&mut ok, &ps).unwrap();
        for (bad, needle) in [(json!({}), "missing parameter `invoice`"), (json!({ "invoice": 3 }), "must be a file"), (json!({ "invoice": { "name": "x.pdf" } }), "must be a file"),
                              (json!({ "invoice": "file_a", "attachments": "file_b" }), "must be a list of files"), (json!({ "invoice": "file_a", "attachments": ["file_1", "file_2", "file_3"] }), "at most 2 files")] {
            let mut b = bad; let e = validate_params(&mut b, &ps).unwrap_err();
            assert!(e.contains(needle), "{e}");
        }
        assert_eq!(ts_type(&ps[0]), "CoriFile");
        assert_eq!(ts_type(&ps[1]), "readonly CoriFile[]");
        // the new fields stay out of manifests that do not use them
        assert!(!serde_json::to_string(&param("t", "string", true, None)).unwrap().contains("accept"));
    }

    #[test]
    fn file_parameters_are_checked_where_declared() {
        let base = "id: w\nname: W\nsteps: [a]\n";
        let ok = format!("{base}parameters:\n  - {{ name: f, type: file, accept: [application/pdf, image/*, .csv], max_size: 10MB }}\n  - {{ name: fs, type: files, required: false, max_files: 5 }}\n");
        let m = parse(&ok).unwrap();
        assert_eq!(m.parameters[0].max_size_bytes(), Some(10_000_000));
        assert_eq!(m.parameters[1].max_files(), 5);
        for (bad, needle) in [
            (format!("{base}parameters:\n  - {{ name: f, type: file, default: x }}\n"), "has no `default`"),
            (format!("{base}parameters:\n  - {{ name: f, type: file, accept: [pdf] }}\n"), "accept: `pdf`"),
            (format!("{base}parameters:\n  - {{ name: f, type: file, accept: [] }}\n"), "lists no type"),
            (format!("{base}parameters:\n  - {{ name: f, type: file, max_size: huge }}\n"), "max_size"),
            (format!("{base}parameters:\n  - {{ name: f, type: file, max_files: 3 }}\n"), "`max_files` is for `files`"),
            (format!("{base}parameters:\n  - {{ name: f, type: files, max_files: 500 }}\n"), "max_files is 1 to 100"),
            (format!("{base}parameters:\n  - {{ name: t, type: string, accept: [.pdf] }}\n"), "are for `file` and `files`"),
            (format!("{base}schedule: \"0 8 * * *\"\nparameters:\n  - {{ name: f, type: file }}\n"), "scheduled workflow"),
        ] {
            let e = parse(&bad).unwrap_err();
            assert!(e.contains(needle), "{bad}: {e}");
        }
        // an optional file on a scheduled workflow is fine: the schedule runs without it
        parse(&format!("{base}schedule: \"0 8 * * *\"\nparameters:\n  - {{ name: f, type: file, required: false }}\n")).unwrap();
    }

    /// Versions stored before `steps:` existed carry a manifest without it; the server must still read them.
    #[test]
    fn stored_manifest_without_steps_still_deserializes() {
        let m: Manifest = serde_json::from_str(r#"{"id":"triage","name":"Triage","parameters":[],"connectors":["helpdesk"]}"#).unwrap();
        assert!(m.steps.is_empty());
        assert_eq!(m.id, "triage");
    }
}
