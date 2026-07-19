//! Static parser for SDK step files.
//!
//! The parser takes a conservative, regex-based approach: we expect step
//! files to follow the canonical SDK pattern documented in
//! `skill/references/activity_kinds.md`:
//!
//! ```ts
//! import { step } from "@cori-do/sdk";
//! // ...optional imports, type aliases, const declarations...
//! export default step.<kind>({
//!   description: "one-line summary",
//!   // kind-specific fields
//! });
//! ```
//!
//! We extract the step kind from the `step.<kind>` constructor, the
//! `description` string literal from the top-level options object, and
//! kind-specific scalar metadata (CLI binary name, MCP server/tool,
//! `route`). Complex builder expressions are not evaluated here — the Rust
//! worker will fall back to a Deno subprocess for those cases.
//!
//! This deliberately rejects step files whose call structure deviates from
//! the canonical pattern. A future swc/oxc-based parser will relax this.

use regex::Regex;
use serde_json::{Map as JsonMap, Value as JsonValue};

use cori_protocol::StepKind;

#[derive(Debug, Clone)]
pub struct ParsedStep {
    pub kind: StepKind,
    pub description: String,
    pub route: Option<String>,
    pub metadata: JsonMap<String, JsonValue>,
}

#[derive(Debug, Clone)]
pub struct ParseError {
    pub reason: String,
    pub line: Option<usize>,
    pub field: Option<String>,
}

impl ParseError {
    fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
            line: None,
            field: None,
        }
    }
    #[allow(dead_code)]
    fn at(mut self, line: usize) -> Self {
        self.line = Some(line);
        self
    }
    fn field(mut self, f: impl Into<String>) -> Self {
        self.field = Some(f.into());
        self
    }
}

pub fn parse(source: &str) -> Result<ParsedStep, Vec<ParseError>> {
    let stripped = strip_comments(source);
    let mut errors: Vec<ParseError> = Vec::new();

    // 1. Require the canonical SDK import. The runtime import map exposes
    //    `@cori-do/sdk`; accepting a lookalike package here would make
    //    `cori check` pass and then fail only when Deno imports the step.
    let sdk_import_re =
        Regex::new(r#"(?m)^\s*import\s*\{[^}]*\bstep\b[^}]*\}\s*from\s*["']@cori-do/sdk["']\s*;?"#)
            .expect("static regex");
    if !sdk_import_re.is_match(&stripped) {
        let reason = if stripped.contains("@cori/sdk") {
            "invalid SDK import `@cori/sdk` — use `import { step } from \"@cori-do/sdk\";`"
        } else {
            "missing canonical SDK import `import { step } from \"@cori-do/sdk\";`"
        };
        let line = stripped
            .lines()
            .position(|line| line.contains("@cori/sdk"))
            .map(|index| index + 1)
            .unwrap_or(1);
        errors.push(ParseError::new(reason).at(line).field("import"));
    }

    // 2. Locate `export default step.<kind>(`.
    let header_re = Regex::new(r"export\s+default\s+step\.([A-Za-z_][A-Za-z0-9_]*)\s*\(")
        .expect("static regex");
    let cap = match header_re.captures(&stripped) {
        Some(c) => c,
        None => {
            return Err(vec![ParseError::new(
                "missing `export default step.<kind>(...)` — every step file's default export must be a `step.<kind>` call",
            )]);
        }
    };
    let kind_str = cap.get(1).unwrap().as_str();
    let kind = match kind_str {
        "cli" => StepKind::Cli,
        "mcp_tool" => StepKind::McpTool,
        "code" => StepKind::Code,
        "llm" => StepKind::Llm,
        "map" | "for_each" | "branch" | "parallel" | "wait" => StepKind::Builtin,
        other => {
            return Err(vec![ParseError::new(format!(
                "unknown step kind `step.{other}` — must be one of: cli, mcp_tool, code, llm, map, for_each, branch, parallel, wait"
            ))]);
        }
    };

    // 3. Carve out the call's argument span — balance parentheses/braces
    //    starting from just after the `(`.
    let open_paren = cap.get(0).unwrap().end();
    let args_span = match extract_balanced(&stripped, open_paren - 1, '(', ')') {
        Some(s) => s,
        None => {
            return Err(vec![ParseError::new(
                "unbalanced parentheses in `step.<kind>(...)` call",
            )]);
        }
    };
    let args_line_offset = stripped[..open_paren]
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count();

    // 4. Pull `description` out of the options object.
    let description = match extract_string_field(args_span, "description") {
        Some(s) => s,
        None => {
            errors.push(
                ParseError::new("missing required `description: \"...\"` field")
                    .field("description"),
            );
            String::new()
        }
    };

    let route = extract_string_field(args_span, "route");

    // 5. Kind-agnostic scalar metadata shared by every step kind
    //    (`BaseStepOpts` in the SDK).
    let mut metadata = JsonMap::new();
    if let Some(retries) = extract_retries_field(args_span) {
        metadata.insert("retries".into(), JsonValue::Object(retries));
    }

    // 6. Kind-specific scalar metadata.
    match kind {
        StepKind::Cli => {
            if let Some(bin) = extract_cli_binary(args_span) {
                metadata.insert("binary".into(), JsonValue::String(bin));
            } else {
                errors.push(
                    ParseError::new(
                        "could not statically determine CLI binary — the first element of `command(...)`'s returned array must be a string literal",
                    )
                    .field("command"),
                );
            }
            errors.extend(validate_cli_parse_context(args_span, args_line_offset));
        }
        StepKind::McpTool => {
            if let Some(server) = extract_string_field(args_span, "server") {
                metadata.insert("server".into(), JsonValue::String(server));
            } else {
                errors.push(
                    ParseError::new("missing required `server: \"...\"` field").field("server"),
                );
            }
            if let Some(tool) = extract_string_field(args_span, "tool") {
                metadata.insert("tool".into(), JsonValue::String(tool));
            } else {
                errors
                    .push(ParseError::new("missing required `tool: \"...\"` field").field("tool"));
            }
        }
        StepKind::Llm => {
            if let Some(model) = extract_string_field(args_span, "model") {
                metadata.insert("model".into(), JsonValue::String(model));
            } else {
                errors.push(
                    ParseError::new("missing required `model: \"...\"` field").field("model"),
                );
            }
            if let Some((size, by)) = extract_batch_field(args_span) {
                let mut batch = JsonMap::new();
                batch.insert("size".into(), JsonValue::Number(size.into()));
                batch.insert("by".into(), JsonValue::String(by));
                metadata.insert("batch".into(), JsonValue::Object(batch));
            }
        }
        StepKind::Code | StepKind::Builtin => {}
    }

    // 7. Code-step import audit: scan the *full* source (not just the call)
    //    for `node:*` imports. Done with a simple line scan.
    if kind == StepKind::Code {
        let banned = find_node_imports(&stripped);
        if !banned.is_empty() {
            let arr: Vec<JsonValue> = banned.into_iter().map(JsonValue::String).collect();
            metadata.insert("node_imports".into(), JsonValue::Array(arr));
        }
    }

    if !errors.is_empty() {
        return Err(errors);
    }
    Ok(ParsedStep {
        kind,
        description,
        route,
        metadata,
    })
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Strip `//` line comments and `/* */` block comments, replacing them with
/// spaces so byte offsets remain stable for downstream regex matches.
fn strip_comments(source: &str) -> String {
    let bytes = source.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    let mut in_str: Option<u8> = None; // ', ", `
    while i < bytes.len() {
        let b = bytes[i];
        if let Some(q) = in_str {
            out.push(b);
            if b == b'\\' && i + 1 < bytes.len() {
                out.push(bytes[i + 1]);
                i += 2;
                continue;
            }
            if b == q {
                in_str = None;
            }
            i += 1;
            continue;
        }
        match b {
            b'"' | b'\'' | b'`' => {
                in_str = Some(b);
                out.push(b);
                i += 1;
            }
            b'/' if i + 1 < bytes.len()
                && bytes[i + 1] == b'/'
                && !is_backslash_escaped(bytes, i) =>
            {
                while i < bytes.len() && bytes[i] != b'\n' {
                    out.push(b' ');
                    i += 1;
                }
            }
            b'/' if i + 1 < bytes.len()
                && bytes[i + 1] == b'*'
                && !is_backslash_escaped(bytes, i) =>
            {
                let mut j = i + 2;
                while j + 1 < bytes.len() && !(bytes[j] == b'*' && bytes[j + 1] == b'/') {
                    j += 1;
                }
                let end = (j + 2).min(bytes.len());
                for &c in &bytes[i..end] {
                    out.push(if c == b'\n' { b'\n' } else { b' ' });
                }
                i = end;
            }
            _ => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out).unwrap_or_else(|_| source.to_string())
}

/// JavaScript regex literals escape `/` with a backslash. Without this
/// check, the closing `\/` in a pattern such as `/\//g` combines with the
/// regex terminator into `//`, which a comment-only scanner mistakes for a
/// line comment and then hides the rest of the step file.
fn is_backslash_escaped(bytes: &[u8], index: usize) -> bool {
    let mut backslashes = 0;
    let mut cursor = index;
    while cursor > 0 && bytes[cursor - 1] == b'\\' {
        backslashes += 1;
        cursor -= 1;
    }
    backslashes % 2 == 1
}

/// Given a source string and the byte offset of an opening `(` (or `{` or
/// `[`), return the slice between the opener and its matching closer
/// (exclusive). Respects string literals and nested brackets.
fn extract_balanced(s: &str, open_at: usize, open: char, close: char) -> Option<&str> {
    let bytes = s.as_bytes();
    if open_at >= bytes.len() || bytes[open_at] as char != open {
        return None;
    }
    let mut depth: i32 = 1;
    let mut i = open_at + 1;
    let start = i;
    let mut in_str: Option<u8> = None;
    while i < bytes.len() {
        let b = bytes[i];
        if let Some(q) = in_str {
            if b == b'\\' && i + 1 < bytes.len() {
                i += 2;
                continue;
            }
            if b == q {
                in_str = None;
            }
            i += 1;
            continue;
        }
        match b as char {
            '"' | '\'' | '`' => in_str = Some(b),
            c if c == open => depth += 1,
            c if c == close => {
                depth -= 1;
                if depth == 0 {
                    return std::str::from_utf8(&bytes[start..i]).ok();
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Validate the SDK contract for the optional second `parse` argument.
/// CLI parsing happens after the command has completed and receives only
/// `{ stderr, exitCode }`; workflow input is deliberately not re-injected.
fn validate_cli_parse_context(body: &str, line_offset: usize) -> Vec<ParseError> {
    let mut errors = Vec::new();
    let named_context = Regex::new(
        r"parse\s*:\s*(?:async\s*)?\(\s*[^,()]+\s*,\s*([A-Za-z_$][A-Za-z0-9_$]*)\s*\)\s*=>",
    )
    .expect("static regex");
    if let Some(captures) = named_context.captures(body) {
        let signature = captures.get(0).expect("whole regex match");
        let context_name = captures.get(1).expect("parse context capture").as_str();
        let callback_body = &body[signature.end()..];
        let mut reported = Vec::new();
        for (property, offset) in member_properties(callback_body, context_name) {
            if matches!(property.as_str(), "stderr" | "exitCode") || reported.contains(&property) {
                continue;
            }
            reported.push(property.clone());
            let line = line_offset
                + body[..signature.end() + offset]
                    .bytes()
                    .filter(|byte| *byte == b'\n')
                    .count()
                + 1;
            errors.push(invalid_parse_context_property(&property, line));
        }
        return errors;
    }

    let destructured_context =
        Regex::new(r"parse\s*:\s*(?:async\s*)?\(\s*[^,()]+\s*,\s*\{([^{}]*)\}\s*\)\s*=>")
            .expect("static regex");
    if let Some(captures) = destructured_context.captures(body) {
        let signature = captures.get(0).expect("whole regex match");
        let bindings = captures.get(1).expect("parse context bindings").as_str();
        let line = line_offset
            + body[..signature.start()]
                .bytes()
                .filter(|byte| *byte == b'\n')
                .count()
            + 1;
        for binding in bindings
            .split(',')
            .map(str::trim)
            .filter(|part| !part.is_empty())
        {
            let property = binding
                .trim_start_matches("...")
                .split([':', '='])
                .next()
                .unwrap_or_default()
                .trim();
            if !matches!(property, "stderr" | "exitCode") {
                errors.push(invalid_parse_context_property(property, line));
            }
        }
    }
    errors
}

fn invalid_parse_context_property(property: &str, line: usize) -> ParseError {
    ParseError::new(format!(
        "CLI `parse` receives only `(stdout, {{ stderr, exitCode }})`; workflow input property `{property}` is unavailable — derive output from stdout or return a fixed acknowledgement"
    ))
    .at(line)
    .field("parse")
}

/// Return `object.property` accesses outside quoted strings. This small
/// lexical scan avoids treating diagnostic text such as `"ctx.input"` as
/// executable property access while keeping the v1 parser dependency-light.
fn member_properties(source: &str, object: &str) -> Vec<(String, usize)> {
    let bytes = source.as_bytes();
    let object_bytes = object.as_bytes();
    let mut properties = Vec::new();
    let mut index = 0;
    let mut quote: Option<u8> = None;
    while index < bytes.len() {
        let byte = bytes[index];
        if let Some(delimiter) = quote {
            if byte == b'\\' && index + 1 < bytes.len() {
                index += 2;
                continue;
            }
            if byte == delimiter {
                quote = None;
            }
            index += 1;
            continue;
        }
        if matches!(byte, b'\'' | b'"' | b'`') {
            quote = Some(byte);
            index += 1;
            continue;
        }
        if bytes[index..].starts_with(object_bytes)
            && (index == 0 || !is_js_identifier_byte(bytes[index - 1]))
            && (index + object_bytes.len() == bytes.len()
                || !is_js_identifier_byte(bytes[index + object_bytes.len()]))
        {
            let mut cursor = index + object_bytes.len();
            while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
                cursor += 1;
            }
            if cursor + 1 < bytes.len() && bytes[cursor] == b'?' && bytes[cursor + 1] == b'.' {
                cursor += 2;
            } else if cursor < bytes.len() && bytes[cursor] == b'.' {
                cursor += 1;
            } else {
                index += object_bytes.len();
                continue;
            }
            while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
                cursor += 1;
            }
            let property_start = cursor;
            while cursor < bytes.len() && is_js_identifier_byte(bytes[cursor]) {
                cursor += 1;
            }
            if cursor > property_start {
                properties.push((source[property_start..cursor].to_string(), property_start));
            }
            index = cursor;
            continue;
        }
        index += 1;
    }
    properties
}

fn is_js_identifier_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'$')
}

/// Extract a top-level `field: "literal"` from an options-object body.
/// Only matches direct string literals (single, double, or template with no
/// interpolation).
fn extract_string_field(body: &str, field: &str) -> Option<String> {
    let pattern = format!(
        r#"(?m)(^|[\s,{{])\s*{field}\s*:\s*("([^"\\]|\\.)*"|'([^'\\]|\\.)*'|`([^`\\$]|\\.)*`)\s*([,\n}}]|$)"#,
        field = regex::escape(field)
    );
    let re = Regex::new(&pattern).ok()?;
    let cap = re.captures(body)?;
    let raw = cap.get(2)?.as_str();
    Some(unquote(raw))
}

fn unquote(raw: &str) -> String {
    let inner = &raw[1..raw.len() - 1];
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('r') => out.push('\r'),
                Some('\\') => out.push('\\'),
                Some('"') => out.push('"'),
                Some('\'') => out.push('\''),
                Some('`') => out.push('`'),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Pull the CLI binary name out of a `command:` field. Accepts the
/// idiomatic shapes:
///   command: () => ["binary", ...]
///   command: ({...}) => ["binary", ...]
///   command: (input) => { ...; return ["binary", ...]; }
fn extract_cli_binary(body: &str) -> Option<String> {
    let key_re = Regex::new(r"(?m)(^|[\s,{])\s*command\s*:").expect("static regex");
    let m = key_re.find(body)?;
    let after_key = &body[m.end()..];
    // Walk to the `=>` arrow, skipping the parameter list parens.
    let arrow_pos = find_arrow(after_key)?;
    let after_arrow = after_key[arrow_pos + 2..].trim_start();
    if after_arrow.starts_with('[') {
        return read_array_first_string(after_arrow);
    }
    if after_arrow.starts_with('{') {
        // Block body — find the first top-level `return [` inside (i.e. not
        // nested inside another function body or block).
        let block_end = find_matching_brace(after_arrow)?;
        let block = &after_arrow[1..block_end];
        let pos = find_top_level_return_array(block)?;
        return read_array_first_string(&block[pos..]);
    }
    None
}

/// Read the first string literal element of an array literal that starts at
/// `s[0] == '['`.
fn read_array_first_string(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    if bytes.first().copied() != Some(b'[') {
        return None;
    }
    let mut j = 1;
    while j < bytes.len() && (bytes[j] as char).is_whitespace() {
        j += 1;
    }
    if j >= bytes.len() || !matches!(bytes[j], b'"' | b'\'' | b'`') {
        return None;
    }
    let quote = bytes[j];
    let start = j;
    let mut k = j + 1;
    while k < bytes.len() {
        if bytes[k] == b'\\' && k + 1 < bytes.len() {
            k += 2;
            continue;
        }
        if bytes[k] == quote {
            let lit = std::str::from_utf8(&bytes[start..=k]).ok()?;
            return Some(unquote(lit));
        }
        k += 1;
    }
    None
}

/// Locate the `=>` arrow of an arrow function, skipping over the parameter
/// list (which may contain `=>` inside default values — unlikely but
/// handled by tracking paren depth).
fn find_arrow(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut i = 0;
    let mut depth_paren: i32 = 0;
    let mut depth_brace: i32 = 0;
    let mut depth_bracket: i32 = 0;
    let mut in_str: Option<u8> = None;
    while i + 1 < bytes.len() {
        let b = bytes[i];
        if let Some(q) = in_str {
            if b == b'\\' && i + 1 < bytes.len() {
                i += 2;
                continue;
            }
            if b == q {
                in_str = None;
            }
            i += 1;
            continue;
        }
        match b {
            b'"' | b'\'' | b'`' => in_str = Some(b),
            b'(' => depth_paren += 1,
            b')' => depth_paren -= 1,
            b'{' => depth_brace += 1,
            b'}' => depth_brace -= 1,
            b'[' => depth_bracket += 1,
            b']' => depth_bracket -= 1,
            b'=' if depth_paren == 0
                && depth_brace == 0
                && depth_bracket == 0
                && bytes.get(i + 1).copied() == Some(b'>') =>
            {
                return Some(i);
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Given a string that starts with `{`, return the index of the matching `}`.
fn find_matching_brace(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    if bytes.first().copied() != Some(b'{') {
        return None;
    }
    let mut depth: i32 = 0;
    let mut in_str: Option<u8> = None;
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if let Some(q) = in_str {
            if b == b'\\' && i + 1 < bytes.len() {
                i += 2;
                continue;
            }
            if b == q {
                in_str = None;
            }
            i += 1;
            continue;
        }
        match b {
            b'"' | b'\'' | b'`' => in_str = Some(b),
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Find the position of the first `[` of a `return [` statement at the top
/// level of `block` (i.e. not nested inside any `{...}` or `(...)` or
/// `[...]`). Returns the byte index of the `[`.
fn find_top_level_return_array(block: &str) -> Option<usize> {
    let bytes = block.as_bytes();
    let mut i = 0;
    let mut in_str: Option<u8> = None;
    let mut depth_brace: i32 = 0;
    let mut depth_paren: i32 = 0;
    let mut depth_bracket: i32 = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if let Some(q) = in_str {
            if b == b'\\' && i + 1 < bytes.len() {
                i += 2;
                continue;
            }
            if b == q {
                in_str = None;
            }
            i += 1;
            continue;
        }
        match b {
            b'"' | b'\'' | b'`' => {
                in_str = Some(b);
                i += 1;
                continue;
            }
            b'{' => depth_brace += 1,
            b'}' => depth_brace -= 1,
            b'(' => depth_paren += 1,
            b')' => depth_paren -= 1,
            b'[' => depth_bracket += 1,
            b']' => depth_bracket -= 1,
            _ => {}
        }
        if depth_brace == 0
            && depth_paren == 0
            && depth_bracket == 0
            && b == b'r'
            && block[i..].starts_with("return")
        {
            // Confirm word boundary before.
            let prev_ok = i == 0
                || matches!(
                    bytes[i - 1],
                    b' ' | b'\t' | b'\n' | b'\r' | b';' | b'{' | b'}'
                );
            // Confirm word boundary after.
            let after = i + "return".len();
            let next_ok =
                after < bytes.len() && matches!(bytes[after], b' ' | b'\t' | b'\n' | b'\r');
            if prev_ok && next_ok {
                // Skip whitespace then expect `[`.
                let mut j = after;
                while j < bytes.len() && (bytes[j] as char).is_whitespace() {
                    j += 1;
                }
                if j < bytes.len() && bytes[j] == b'[' {
                    return Some(j);
                }
            }
        }
        i += 1;
    }
    None
}

/// Return banned `node:*` module specifiers imported by the source.
fn find_node_imports(source: &str) -> Vec<String> {
    let re =
        Regex::new(r#"(?m)^\s*import\b[^;]*?from\s*['"](node:[^'"]+)['"]"#).expect("static regex");
    let mut out = Vec::new();
    for cap in re.captures_iter(source) {
        if let Some(m) = cap.get(1) {
            out.push(m.as_str().to_string());
        }
    }
    out
}

/// Extract a `batch: { size: N, by: "field" }` declaration from an LLM
/// step's options object. Only literal numeric `size` and string `by` are
/// recognised.
fn extract_batch_field(body: &str) -> Option<(u64, String)> {
    let key_re = Regex::new(r"(?m)(^|[\s,{])\s*batch\s*:\s*\{").expect("static regex");
    let m = key_re.find(body)?;
    let brace_at = body[..m.end()].rfind('{')?;
    let inner = extract_balanced(body, brace_at, '{', '}')?;
    let size_re = Regex::new(r#"(?m)(^|[\s,{])\s*size\s*:\s*(\d+)"#).expect("static regex");
    let by = extract_string_field(inner, "by")?;
    let size = size_re
        .captures(inner)
        .and_then(|c| c.get(2))
        .and_then(|m| m.as_str().parse::<u64>().ok())?;
    Some((size, by))
}

/// Extract a `retries: { max: N, backoff: "exponential" | "linear" }`
/// declaration from any step's options object (`BaseStepOpts` in the SDK).
/// `max` is required for the field to be recognised; `backoff` is optional.
/// The emitted metadata mirrors the SDK field names so the worker can read
/// `retries.max` / `retries.backoff` directly.
fn extract_retries_field(body: &str) -> Option<JsonMap<String, JsonValue>> {
    let key_re = Regex::new(r"(?m)(^|[\s,{])\s*retries\s*:\s*\{").expect("static regex");
    let m = key_re.find(body)?;
    let brace_at = body[..m.end()].rfind('{')?;
    let inner = extract_balanced(body, brace_at, '{', '}')?;
    let max_re = Regex::new(r#"(?m)(^|[\s,{])\s*max\s*:\s*(\d+)"#).expect("static regex");
    let max = max_re
        .captures(inner)
        .and_then(|c| c.get(2))
        .and_then(|m| m.as_str().parse::<u64>().ok())?;
    let mut out = JsonMap::new();
    out.insert("max".into(), JsonValue::Number(max.into()));
    if let Some(backoff) = extract_string_field(inner, "backoff") {
        out.insert("backoff".into(), JsonValue::String(backoff));
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_cli_step() {
        let src = "import { step } from \"@cori-do/sdk\";\nexport default step.cli({\n  description: \"do a thing\",\n  command: () => [\"gws\", \"sheets\"],\n});\n";
        let p = parse(src).unwrap();
        assert_eq!(p.kind, StepKind::Cli);
        assert_eq!(p.description, "do a thing");
        assert_eq!(p.metadata.get("binary").unwrap(), "gws");
    }

    #[test]
    fn rejects_workflow_input_read_from_cli_parse_context() {
        let src = r#"import { step } from "@cori-do/sdk";
export default step.cli({
  description: "update messages",
  command: ({ message_ids }) => ["gws", "gmail", JSON.stringify(message_ids)],
  parse: (_stdout, input) => ({ updated_message_ids: input.message_ids }),
});"#;
        let errors = parse(src).unwrap_err();
        let error = errors
            .iter()
            .find(|error| error.reason.contains("message_ids"))
            .expect("invalid parse context diagnostic");
        assert_eq!(error.field.as_deref(), Some("parse"));
        assert_eq!(error.line, Some(5));
        assert!(error.reason.contains("fixed acknowledgement"));
    }

    #[test]
    fn accepts_supported_cli_parse_context_properties() {
        let named = r#"import { step } from "@cori-do/sdk";
export default step.cli({
  description: "check result",
  command: () => ["gws", "version"],
  parse: (_stdout, context) => ({ stderr: context.stderr, exit_code: context.exitCode }),
});"#;
        assert!(parse(named).is_ok());

        let destructured = r#"import { step } from "@cori-do/sdk";
export default step.cli({
  description: "check result",
  command: () => ["gws", "version"],
  parse: (_stdout, { stderr, exitCode }) => ({ stderr, exit_code: exitCode }),
});"#;
        assert!(parse(destructured).is_ok());
    }

    #[test]
    fn rejects_unsupported_destructured_cli_parse_context_property() {
        let src = r#"import { step } from "@cori-do/sdk";
export default step.cli({
  description: "update messages",
  command: () => ["gws", "gmail"],
  parse: (_stdout, { message_ids }) => ({ updated_message_ids: message_ids }),
});"#;
        let errors = parse(src).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|error| error.reason.contains("message_ids"))
        );
    }

    #[test]
    fn parses_mcp_step() {
        let src = "import { step } from \"@cori-do/sdk\";\nexport default step.mcp_tool({ description: \"post\", server: \"slack\", tool: \"chat_postMessage\", args: () => ({}) });";
        let p = parse(src).unwrap();
        assert_eq!(p.kind, StepKind::McpTool);
        assert_eq!(p.metadata.get("server").unwrap(), "slack");
        assert_eq!(p.metadata.get("tool").unwrap(), "chat_postMessage");
    }

    #[test]
    fn parses_llm_step_with_model() {
        let src = "import { step } from \"@cori-do/sdk\";\nexport default step.llm({ description: \"translate\", model: \"gpt-4o-mini\", prompt: () => `hi` });";
        let p = parse(src).unwrap();
        assert_eq!(p.kind, StepKind::Llm);
        assert_eq!(p.metadata.get("model").unwrap(), "gpt-4o-mini");
    }

    #[test]
    fn parses_code_step() {
        let src = "import { step } from \"@cori-do/sdk\";\nexport default step.code({ description: \"square\", run: (x) => x });";
        let p = parse(src).unwrap();
        assert_eq!(p.kind, StepKind::Code);
    }

    #[test]
    fn regex_with_escaped_slash_is_not_mistaken_for_a_comment() {
        let src = r#"import { step } from "@cori-do/sdk";
export default step.code({
  description: "encode",
  run: () => "value".replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, ""),
});"#;
        let parsed = parse(src).unwrap();
        assert_eq!(parsed.kind, StepKind::Code);
        assert_eq!(parsed.description, "encode");
    }

    #[test]
    fn parses_builtin_map() {
        let src = "import { step } from \"@cori-do/sdk\";\nexport default step.map({ description: \"each\", apply: x });";
        let p = parse(src).unwrap();
        assert_eq!(p.kind, StepKind::Builtin);
    }

    #[test]
    fn missing_description() {
        let src =
            "import { step } from \"@cori-do/sdk\";\nexport default step.code({ run: (x) => x });";
        let errs = parse(src).unwrap_err();
        assert!(errs.iter().any(|e| e.reason.contains("description")));
    }

    #[test]
    fn rejects_legacy_sdk_import() {
        let src = "import { step } from \"@cori/sdk\";\nexport default step.code({ description: \"square\", run: (x) => x });";
        let errs = parse(src).unwrap_err();
        assert!(
            errs.iter()
                .any(|error| error.reason.contains("@cori-do/sdk"))
        );
        assert_eq!(errs[0].line, Some(1));
    }

    #[test]
    fn rejects_missing_sdk_import() {
        let src = "export default step.code({ description: \"square\", run: (x) => x });";
        let errs = parse(src).unwrap_err();
        assert!(
            errs.iter()
                .any(|error| error.reason.contains("canonical SDK import"))
        );
    }

    #[test]
    fn missing_default_export() {
        let src = "import { step } from \"@cori-do/sdk\";\nconst x = step.code({ description: \"x\", run: (x) => x });";
        let errs = parse(src).unwrap_err();
        assert!(errs[0].reason.contains("default export"));
    }

    #[test]
    fn unknown_kind() {
        let src = "import { step } from \"@cori-do/sdk\";\nexport default step.weird({ description: \"x\" });";
        let errs = parse(src).unwrap_err();
        assert!(errs[0].reason.contains("unknown"));
    }

    #[test]
    fn flags_node_imports_in_code() {
        let src = "import { step } from \"@cori-do/sdk\";\nimport fs from \"node:fs\";\nexport default step.code({ description: \"x\", run: (x) => x });";
        let p = parse(src).unwrap();
        let arr = p.metadata.get("node_imports").unwrap().as_array().unwrap();
        assert_eq!(arr[0], "node:fs");
    }

    #[test]
    fn no_node_imports_flagged_for_cli() {
        let src = "import { step } from \"@cori-do/sdk\";\nimport fs from \"node:fs\";\nexport default step.cli({ description: \"x\", command: () => [\"echo\"] });";
        let p = parse(src).unwrap();
        assert!(p.metadata.get("node_imports").is_none());
    }

    #[test]
    fn description_can_be_single_quoted() {
        let src = "import { step } from \"@cori-do/sdk\";\nexport default step.code({ description: 'hi', run: (x) => x });";
        let p = parse(src).unwrap();
        assert_eq!(p.description, "hi");
    }

    #[test]
    fn cli_binary_must_be_literal() {
        let src = "import { step } from \"@cori-do/sdk\";\nconst bin = 'gws';\nexport default step.cli({ description: \"x\", command: () => [bin, 'y'] });";
        let errs = parse(src).unwrap_err();
        assert!(errs[0].reason.contains("command"));
    }

    #[test]
    fn comments_do_not_confuse_parser() {
        let src = "// export default step.cli({ description: \"fake\" })\nimport { step } from \"@cori-do/sdk\";\n/* export default step.cli */\nexport default step.code({ description: \"real\", run: (x) => x });";
        let p = parse(src).unwrap();
        assert_eq!(p.description, "real");
        assert_eq!(p.kind, StepKind::Code);
    }

    #[test]
    fn route_field_extracted() {
        let src = "import { step } from \"@cori-do/sdk\";\nexport default step.code({ description: \"x\", route: \"worker\", run: (x) => x });";
        let p = parse(src).unwrap();
        assert_eq!(p.route.as_deref(), Some("worker"));
    }

    #[test]
    fn retries_field_extracted() {
        let src = "import { step } from \"@cori-do/sdk\";\nexport default step.cli({ description: \"x\", retries: { max: 5, backoff: \"linear\" }, command: () => [\"echo\"] });";
        let p = parse(src).unwrap();
        let retries = p.metadata.get("retries").unwrap().as_object().unwrap();
        assert_eq!(retries.get("max").unwrap(), 5);
        assert_eq!(retries.get("backoff").unwrap(), "linear");
    }

    #[test]
    fn retries_backoff_optional() {
        let src = "import { step } from \"@cori-do/sdk\";\nexport default step.code({ description: \"x\", retries: { max: 2 }, run: (x) => x });";
        let p = parse(src).unwrap();
        let retries = p.metadata.get("retries").unwrap().as_object().unwrap();
        assert_eq!(retries.get("max").unwrap(), 2);
        assert!(retries.get("backoff").is_none());
    }

    #[test]
    fn no_retries_field_means_no_metadata() {
        let src = "import { step } from \"@cori-do/sdk\";\nexport default step.code({ description: \"x\", run: (x) => x });";
        let p = parse(src).unwrap();
        assert!(p.metadata.get("retries").is_none());
    }
}
