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

use cori_protocol::{
    DEFAULT_FOR_EACH_ITEMS, DEFAULT_LOOP_ITERATIONS, MAX_ACTIVITY_ATTEMPTS,
    MAX_ACTIVITY_TIMEOUT_MS, MAX_FOR_EACH_ITEMS, MAX_LOOP_ITERATIONS, MAX_WAIT_TIMEOUT_MS,
    StepKind, parse_wait_until,
};

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
        "map" | "for_each" | "branch" | "switch" | "loop" | "parallel" | "wait" => {
            StepKind::Builtin
        }
        other => {
            return Err(vec![ParseError::new(format!(
                "unknown step kind `step.{other}` — must be one of: cli, mcp_tool, code, llm, branch, switch, for_each, loop, wait, map, parallel"
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

    // 4. Pull `description` out of the options object. Builtins hold
    //    nested step definitions whose own `description` fields would
    //    satisfy a span-wide regex, so they only accept the top-level
    //    property.
    let top_level_only = kind == StepKind::Builtin;
    let description = match extract_description(args_span, top_level_only) {
        Some(s) => s,
        None => {
            errors.push(
                ParseError::new("missing required `description: \"...\"` field")
                    .field("description"),
            );
            String::new()
        }
    };

    let route = if top_level_only {
        top_level_string_field(args_span, "route")
    } else {
        extract_string_field(args_span, "route")
    };

    // 5. Kind-agnostic scalar metadata shared by every step kind
    //    (`BaseStepOpts` in the SDK). Builtins skip this: their nested
    //    steps carry their own `retries` / `timeout_ms`, extracted per
    //    slot below, and the outer control flow is not an activity.
    let mut metadata = JsonMap::new();
    if kind != StepKind::Builtin {
        match extract_retries_field(args_span) {
            Ok(Some(retries)) => {
                metadata.insert("retries".into(), JsonValue::Object(retries));
            }
            Ok(None) => {}
            Err(error) => errors.push(error),
        }
        match extract_timeout_field(args_span) {
            Ok(Some(timeout_ms)) => {
                metadata.insert("timeout_ms".into(), JsonValue::Number(timeout_ms.into()));
            }
            Ok(None) => {}
            Err(error) => errors.push(error),
        }
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
            if top_level_field_value(args_span, "model").is_some() {
                errors.push(
                    ParseError::new(
                        "`model` is no longer supported on `llm` steps — replace it with `level: \"low\" | \"medium\" | \"high\"`",
                    )
                    .field("model"),
                );
            }

            let level = match top_level_field_value(args_span, "level") {
                None => "medium".to_string(),
                Some(value) => match leading_string_literal(value) {
                    Some(level) if matches!(level.as_str(), "low" | "medium" | "high") => level,
                    Some(level) => {
                        errors.push(
                            ParseError::new(format!(
                                "invalid LLM level `{level}` — expected `low`, `medium`, or `high`"
                            ))
                            .field("level"),
                        );
                        "medium".to_string()
                    }
                    None => {
                        errors.push(
                            ParseError::new(
                                "`level` must be a direct string literal: `low`, `medium`, or `high`",
                            )
                            .field("level"),
                        );
                        "medium".to_string()
                    }
                },
            };
            if top_level_field_value(args_span, "model").is_none() {
                metadata.insert("level".into(), JsonValue::String(level));
            }
            if let Some((size, by)) = extract_batch_field(args_span) {
                let mut batch = JsonMap::new();
                batch.insert("size".into(), JsonValue::Number(size.into()));
                batch.insert("by".into(), JsonValue::String(by));
                metadata.insert("batch".into(), JsonValue::Object(batch));
            }
        }
        StepKind::Code => {}
        StepKind::Builtin => {
            metadata.insert("builtin".into(), JsonValue::String(kind_str.to_string()));
            extract_builtin_metadata(args_span, kind_str, &mut metadata, &mut errors);
        }
    }

    // 7. Code-step import audit: scan the *full* source (not just the call)
    //    for `node:*` imports. Done with a simple line scan. Builtin files
    //    get the same audit: their nested `code` steps run in the same
    //    sandbox, and nothing else in a builtin file may do I/O either.
    if kind == StepKind::Code || kind == StepKind::Builtin {
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
// Builtin control-flow extraction
// ---------------------------------------------------------------------------

/// Extract the control-flow metadata for one builtin step:
///
/// - `branch`   → requires `if`, nested `then`, optional nested `else`
/// - `switch`   → requires `on`, nested `cases.<label>`, optional `default`
/// - `for_each` → requires `over`, nested `apply`, optional `max_items`
/// - `loop`     → requires `until`, nested `body`, optional `max_iterations`
/// - `wait`     → requires `for: { timeout_ms | until | signal }`
///
/// Nested steps land in `metadata.nested` as `{ "<slot>": { kind, ...} }`
/// where `<slot>` doubles as the runner's selector dot-path.
fn extract_builtin_metadata(
    args_span: &str,
    sub_kind: &str,
    metadata: &mut JsonMap<String, JsonValue>,
    errors: &mut Vec<ParseError>,
) {
    let mut nested = JsonMap::new();

    match sub_kind {
        "branch" => {
            require_selector_fn(args_span, sub_kind, "if", errors);
            extract_nested_slot(args_span, "then", true, Routable::Yes, &mut nested, errors);
            extract_nested_slot(args_span, "else", false, Routable::Yes, &mut nested, errors);
        }
        "switch" => {
            require_selector_fn(args_span, sub_kind, "on", errors);
            extract_switch_cases(args_span, &mut nested, errors);
            extract_nested_slot(
                args_span,
                "default",
                false,
                Routable::Yes,
                &mut nested,
                errors,
            );
        }
        "for_each" => {
            require_selector_fn(args_span, sub_kind, "over", errors);
            extract_nested_slot(args_span, "apply", true, Routable::No, &mut nested, errors);
            let max_items = bounded_top_level_integer(
                args_span,
                "max_items",
                MAX_FOR_EACH_ITEMS,
                DEFAULT_FOR_EACH_ITEMS,
                errors,
            );
            metadata.insert("max_items".into(), JsonValue::Number(max_items.into()));
        }
        "loop" => {
            require_selector_fn(args_span, sub_kind, "until", errors);
            extract_nested_slot(args_span, "body", true, Routable::No, &mut nested, errors);
            let max_iterations = bounded_top_level_integer(
                args_span,
                "max_iterations",
                MAX_LOOP_ITERATIONS,
                DEFAULT_LOOP_ITERATIONS,
                errors,
            );
            metadata.insert(
                "max_iterations".into(),
                JsonValue::Number(max_iterations.into()),
            );
        }
        "wait" => {
            extract_wait_spec(args_span, metadata, errors);
        }
        // `map` / `parallel`: accepted, deferred at runtime — no metadata.
        _ => {}
    }

    if !nested.is_empty() {
        metadata.insert("nested".into(), JsonValue::Object(nested));
    }
}

/// Require the builtin's selector function (`if` / `on` / `over` /
/// `until`) to be declared. Presence is all the static parser can check;
/// the runner validates it is a function returning the right type.
fn require_selector_fn(
    args_span: &str,
    sub_kind: &str,
    field: &'static str,
    errors: &mut Vec<ParseError>,
) {
    if top_level_field_value(args_span, field).is_none() {
        errors.push(
            ParseError::new(format!(
                "builtin `{sub_kind}` requires a `{field}` function (a pure arrow function of the accumulated input)"
            ))
            .field(field),
        );
    }
}

/// Whether a slot's value may be a `goto("name")` route instead of an
/// inline nested step. Branch / switch paths route; loop bodies do not.
#[derive(Clone, Copy, PartialEq)]
enum Routable {
    Yes,
    No,
}

/// Extract one nested `slot: step.<kind>({...})` (or, for routable
/// slots, `slot: goto("name")`) declaration into `nested["<slot>"]`.
/// Nested steps must be non-builtin; their scalar capability metadata
/// (CLI binary, MCP server/tool, LLM level) is extracted with the same
/// rules as top-level steps so placement, preflight, and the
/// activity-boundary checks see through the builtin.
fn extract_nested_slot(
    args_span: &str,
    slot: &str,
    required: bool,
    routable: Routable,
    nested: &mut JsonMap<String, JsonValue>,
    errors: &mut Vec<ParseError>,
) {
    let Some(value) = top_level_field_value(args_span, slot) else {
        if required {
            errors.push(
                ParseError::new(format!(
                    "builtin step is missing its required `{slot}: step.<kind>({{...}})` nested step"
                ))
                .field(slot),
            );
        }
        return;
    };
    match parse_path(value, slot, routable) {
        Ok(meta) => {
            nested.insert(slot.to_string(), JsonValue::Object(meta));
        }
        Err(es) => errors.extend(es),
    }
}

/// Parse a routable slot value: either `goto("name")` or an inline
/// nested step.
fn parse_path(
    value: &str,
    slot: &str,
    routable: Routable,
) -> Result<JsonMap<String, JsonValue>, Vec<ParseError>> {
    let goto_re = Regex::new(r#"^goto\s*\(\s*["']([^"']*)["']\s*\)"#).expect("static regex");
    if let Some(cap) = goto_re.captures(value.trim_start()) {
        if routable == Routable::No {
            return Err(vec![
                ParseError::new(format!(
                    "`{slot}` cannot be a `goto(...)` route — only branch/switch paths route; loop bodies are inline steps"
                ))
                .field(slot),
            ]);
        }
        let target = cap.get(1).unwrap().as_str().to_string();
        let name_re = Regex::new(r"^[a-z][a-z0-9_]*$").expect("static regex");
        if !name_re.is_match(&target) {
            return Err(vec![
                ParseError::new(format!(
                    "`{slot}` has invalid goto target `{target}` — use the target step's snake_case name (the part of `NN_name.ts` after the number), or `end`"
                ))
                .field(slot),
            ]);
        }
        let mut meta = JsonMap::new();
        meta.insert("goto_name".into(), JsonValue::String(target));
        return Ok(meta);
    }
    parse_nested_step(value, slot)
}

/// Enumerate `cases: { <label>: step.<kind>({...}) | goto("name"), ... }`.
fn extract_switch_cases(
    args_span: &str,
    nested: &mut JsonMap<String, JsonValue>,
    errors: &mut Vec<ParseError>,
) {
    let Some(value) = top_level_field_value(args_span, "cases") else {
        errors.push(
            ParseError::new(
                "builtin `switch` requires a `cases: { <label>: step.<kind>({...}), ... }` object",
            )
            .field("cases"),
        );
        return;
    };
    if !value.starts_with('{') {
        errors.push(
            ParseError::new("`cases` must be an inline object literal of nested steps")
                .field("cases"),
        );
        return;
    }
    let Some(inner) = extract_balanced(value, 0, '{', '}') else {
        errors.push(ParseError::new("unbalanced `cases` object").field("cases"));
        return;
    };
    let label_re = Regex::new(r"^[A-Za-z0-9_-]+$").expect("static regex");
    let properties = object_properties(inner);
    if properties.is_empty() {
        errors.push(
            ParseError::new("`cases` must declare at least one `<label>: step.<kind>({...})` case")
                .field("cases"),
        );
        return;
    }
    for (label, value_offset) in properties {
        if !label_re.is_match(&label) {
            errors.push(
                ParseError::new(format!(
                    "invalid case label `{label}` — labels must match [A-Za-z0-9_-]+ so they can address the nested step"
                ))
                .field("cases"),
            );
            continue;
        }
        let slot = format!("cases.{label}");
        match parse_path(inner[value_offset..].trim_start(), &slot, Routable::Yes) {
            Ok(meta) => {
                nested.insert(slot, JsonValue::Object(meta));
            }
            Err(es) => errors.extend(es),
        }
    }
}

/// Parse a nested `step.<kind>({...})` expression starting at `value`.
/// Returns the nested step's metadata map (always carrying `kind`).
fn parse_nested_step(
    value: &str,
    slot: &str,
) -> Result<JsonMap<String, JsonValue>, Vec<ParseError>> {
    let header_re = Regex::new(r"^step\.([A-Za-z_][A-Za-z0-9_]*)\s*\(").expect("static regex");
    let Some(cap) = header_re.captures(value) else {
        return Err(vec![
            ParseError::new(format!(
                "`{slot}` must be an inline `step.<kind>({{...}})` call — helper variables and imports cannot be statically verified"
            ))
            .field(slot),
        ]);
    };
    let nested_kind_str = cap.get(1).unwrap().as_str();
    let nested_kind = match nested_kind_str {
        "cli" => StepKind::Cli,
        "mcp_tool" => StepKind::McpTool,
        "code" => StepKind::Code,
        "llm" => StepKind::Llm,
        "map" | "for_each" | "branch" | "switch" | "loop" | "parallel" | "wait" => {
            return Err(vec![
                ParseError::new(format!(
                    "`{slot}` is `step.{nested_kind_str}` — builtins cannot nest other builtins; move the inner control flow into its own step file"
                ))
                .field(slot),
            ]);
        }
        other => {
            return Err(vec![
                ParseError::new(format!(
                    "`{slot}` uses unknown step kind `step.{other}` — nested steps must be one of: cli, mcp_tool, code, llm"
                ))
                .field(slot),
            ]);
        }
    };
    let open_paren = cap.get(0).unwrap().end();
    let Some(nested_span) = extract_balanced(value, open_paren - 1, '(', ')') else {
        return Err(vec![
            ParseError::new(format!("unbalanced parentheses in `{slot}` nested step")).field(slot),
        ]);
    };

    let mut errors: Vec<ParseError> = Vec::new();
    let mut meta = JsonMap::new();
    meta.insert(
        "kind".into(),
        JsonValue::String(nested_kind_str.to_string()),
    );

    match nested_kind {
        StepKind::Cli => {
            if let Some(bin) = extract_cli_binary(nested_span) {
                meta.insert("binary".into(), JsonValue::String(bin));
            } else {
                errors.push(
                    ParseError::new(format!(
                        "could not statically determine the CLI binary of `{slot}` — the first element of its `command(...)` array must be a string literal"
                    ))
                    .field(slot),
                );
            }
        }
        StepKind::McpTool => {
            match extract_string_field(nested_span, "server") {
                Some(server) => {
                    meta.insert("server".into(), JsonValue::String(server));
                }
                None => errors.push(
                    ParseError::new(format!(
                        "`{slot}` is missing its required `server: \"...\"` field"
                    ))
                    .field(slot),
                ),
            }
            match extract_string_field(nested_span, "tool") {
                Some(tool) => {
                    meta.insert("tool".into(), JsonValue::String(tool));
                }
                None => errors.push(
                    ParseError::new(format!(
                        "`{slot}` is missing its required `tool: \"...\"` field"
                    ))
                    .field(slot),
                ),
            }
        }
        StepKind::Llm => {
            if top_level_field_value(nested_span, "model").is_some() {
                errors.push(
                    ParseError::new(format!(
                        "`model` is not supported on the `{slot}` llm step — use `level: \"low\" | \"medium\" | \"high\"`"
                    ))
                    .field(slot),
                );
            } else {
                let level = match top_level_field_value(nested_span, "level") {
                    None => Some("medium".to_string()),
                    Some(value) => match leading_string_literal(value) {
                        Some(level) if matches!(level.as_str(), "low" | "medium" | "high") => {
                            Some(level)
                        }
                        _ => {
                            errors.push(
                                ParseError::new(format!(
                                    "`{slot}.level` must be a direct string literal: `low`, `medium`, or `high`"
                                ))
                                .field(slot),
                            );
                            None
                        }
                    },
                };
                if let Some(level) = level {
                    meta.insert("level".into(), JsonValue::String(level));
                }
            }
        }
        StepKind::Code => {}
        StepKind::Builtin => unreachable!("rejected above"),
    }

    // Nested retries / timeout apply to the nested activity dispatch.
    match extract_retries_field(nested_span) {
        Ok(Some(retries)) => {
            meta.insert("retries".into(), JsonValue::Object(retries));
        }
        Ok(None) => {}
        Err(error) => errors.push(error),
    }
    match extract_timeout_field(nested_span) {
        Ok(Some(timeout_ms)) => {
            meta.insert("timeout_ms".into(), JsonValue::Number(timeout_ms.into()));
        }
        Ok(None) => {}
        Err(error) => errors.push(error),
    }

    if errors.is_empty() {
        Ok(meta)
    } else {
        Err(errors)
    }
}

/// Extract `for: { timeout_ms?, until?, signal? }` for a `wait` builtin.
fn extract_wait_spec(
    args_span: &str,
    metadata: &mut JsonMap<String, JsonValue>,
    errors: &mut Vec<ParseError>,
) {
    let Some(value) = top_level_field_value(args_span, "for") else {
        errors.push(
            ParseError::new(
                "builtin `wait` requires a `for: { timeout_ms?, until?, signal? }` object",
            )
            .field("for"),
        );
        return;
    };
    if !value.starts_with('{') {
        errors.push(ParseError::new("`for` must be an inline object literal").field("for"));
        return;
    }
    let Some(inner) = extract_balanced(value, 0, '{', '}') else {
        errors.push(ParseError::new("unbalanced `for` object").field("for"));
        return;
    };

    let mut wait = JsonMap::new();

    let timeout_re =
        Regex::new(r"(?m)(^|[\s,{])\s*timeout_ms\s*:\s*(-?\d+)").expect("static regex");
    if let Some(cap) = timeout_re.captures(inner) {
        match cap.get(2).unwrap().as_str().parse::<u64>() {
            Ok(ms) if (1..=MAX_WAIT_TIMEOUT_MS).contains(&ms) => {
                wait.insert("timeout_ms".into(), JsonValue::Number(ms.into()));
            }
            _ => errors.push(
                ParseError::new(format!(
                    "`for.timeout_ms` must be an integer from 1 through {MAX_WAIT_TIMEOUT_MS} (30 days)"
                ))
                .field("for.timeout_ms"),
            ),
        }
    } else if inner.contains("timeout_ms") {
        errors.push(
            ParseError::new("`for.timeout_ms` must be a direct integer literal")
                .field("for.timeout_ms"),
        );
    }

    if let Some(until) = extract_string_field(inner, "until") {
        if parse_wait_until(&until).is_some() {
            wait.insert("until".into(), JsonValue::String(until));
        } else {
            errors.push(
                ParseError::new(format!(
                    "`for.until` must be an RFC 3339 timestamp with an explicit offset (e.g. `2026-09-01T09:00:00Z`); got `{until}`"
                ))
                .field("for.until"),
            );
        }
    }

    if let Some(signal) = extract_string_field(inner, "signal") {
        let signal_re = Regex::new(r"^[A-Za-z0-9_.-]+$").expect("static regex");
        if signal_re.is_match(&signal) {
            wait.insert("signal".into(), JsonValue::String(signal));
        } else {
            errors.push(
                ParseError::new(format!(
                    "`for.signal` must match [A-Za-z0-9_.-]+; got `{signal}`"
                ))
                .field("for.signal"),
            );
        }
    }

    if wait.is_empty() && errors.is_empty() {
        errors.push(
            ParseError::new(
                "`for` must declare at least one of `timeout_ms` (delay), `until` (absolute time), or `signal` (external event)",
            )
            .field("for"),
        );
        return;
    }
    if !wait.is_empty() {
        metadata.insert("wait".into(), JsonValue::Object(wait));
    }
}

/// Extract a bounded top-level integer field, falling back to `default`
/// when the field is absent.
fn bounded_top_level_integer(
    args_span: &str,
    field: &str,
    max: u64,
    default: u64,
    errors: &mut Vec<ParseError>,
) -> u64 {
    let Some(value) = top_level_field_value(args_span, field) else {
        return default;
    };
    let literal_re = Regex::new(r"^(-?\d+)").expect("static regex");
    let parsed = literal_re
        .captures(value)
        .and_then(|cap| cap.get(1))
        .and_then(|m| m.as_str().parse::<u64>().ok());
    match parsed {
        Some(n) if (1..=max).contains(&n) => n,
        _ => {
            errors.push(
                ParseError::new(format!("`{field}` must be an integer from 1 through {max}"))
                    .field(field),
            );
            default
        }
    }
}

/// Extract the top-level `description`, optionally restricted to the
/// direct options object (used for builtins, whose nested steps carry
/// their own descriptions).
fn extract_description(args_span: &str, top_level_only: bool) -> Option<String> {
    if top_level_only {
        top_level_string_field(args_span, "description")
    } else {
        extract_string_field(args_span, "description")
    }
}

/// A top-level property whose value is a direct string literal.
fn top_level_string_field(body: &str, field: &str) -> Option<String> {
    top_level_field_value(body, field).and_then(leading_string_literal)
}

/// Enumerate the properties of an object literal's inner span (the text
/// between its braces). Returns `(name, value_offset)` pairs where
/// `value_offset` points just past the `:`. Handles identifier keys and
/// quoted string keys; skips nested structures.
fn object_properties(inner: &str) -> Vec<(String, usize)> {
    let bytes = inner.as_bytes();
    let mut properties = Vec::new();
    let mut index = 0;
    let mut quote: Option<u8> = None;
    let mut brace_depth = 0_i32;
    let mut paren_depth = 0_i32;
    let mut bracket_depth = 0_i32;
    let mut expecting_key = true;

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
        let at_top = brace_depth == 0 && paren_depth == 0 && bracket_depth == 0;
        if at_top && expecting_key {
            if byte.is_ascii_whitespace() {
                index += 1;
                continue;
            }
            // Identifier or quoted key.
            let (name, mut cursor) = if matches!(byte, b'\'' | b'"') {
                let delimiter = byte;
                let start = index + 1;
                let mut end = start;
                while end < bytes.len() && bytes[end] != delimiter {
                    if bytes[end] == b'\\' {
                        end += 1;
                    }
                    end += 1;
                }
                if end >= bytes.len() {
                    return properties;
                }
                (inner[start..end].to_string(), end + 1)
            } else if byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'$' | b'-') {
                let start = index;
                let mut end = index;
                while end < bytes.len() && (is_js_identifier_byte(bytes[end]) || bytes[end] == b'-')
                {
                    end += 1;
                }
                (inner[start..end].to_string(), end)
            } else {
                index += 1;
                continue;
            };
            while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
                cursor += 1;
            }
            if cursor < bytes.len() && bytes[cursor] == b':' {
                properties.push((name, cursor + 1));
                expecting_key = false;
                index = cursor + 1;
                continue;
            }
            index = cursor.max(index + 1);
            continue;
        }
        match byte {
            b'\'' | b'"' | b'`' => quote = Some(byte),
            b'{' => brace_depth += 1,
            b'}' => brace_depth -= 1,
            b'(' => paren_depth += 1,
            b')' => paren_depth -= 1,
            b'[' => bracket_depth += 1,
            b']' => bracket_depth -= 1,
            b',' if brace_depth == 0 && paren_depth == 0 && bracket_depth == 0 => {
                expecting_key = true;
            }
            _ => {}
        }
        index += 1;
    }
    properties
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
        if b == b'/' && regex_can_start(bytes, i, start) {
            i = skip_regex_literal(bytes, i)?;
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

fn regex_can_start(bytes: &[u8], slash_at: usize, span_start: usize) -> bool {
    let mut cursor = slash_at;
    while cursor > span_start && bytes[cursor - 1].is_ascii_whitespace() {
        cursor -= 1;
    }
    if cursor == span_start {
        return true;
    }
    let previous = bytes[cursor - 1];
    if matches!(
        previous,
        b'(' | b'['
            | b'{'
            | b','
            | b':'
            | b';'
            | b'='
            | b'!'
            | b'?'
            | b'&'
            | b'|'
            | b'+'
            | b'-'
            | b'*'
            | b'%'
            | b'^'
            | b'~'
            | b'<'
            | b'>'
    ) {
        return true;
    }
    if previous.is_ascii_alphanumeric() || matches!(previous, b'_' | b'$') {
        let end = cursor;
        while cursor > span_start
            && (bytes[cursor - 1].is_ascii_alphanumeric()
                || matches!(bytes[cursor - 1], b'_' | b'$'))
        {
            cursor -= 1;
        }
        return matches!(
            &bytes[cursor..end],
            b"return" | b"throw" | b"case" | b"delete" | b"void" | b"typeof" | b"yield"
        );
    }
    false
}

fn skip_regex_literal(bytes: &[u8], slash_at: usize) -> Option<usize> {
    let mut cursor = slash_at + 1;
    let mut in_class = false;
    while cursor < bytes.len() {
        match bytes[cursor] {
            b'\\' if cursor + 1 < bytes.len() => cursor += 2,
            b'[' if !in_class => {
                in_class = true;
                cursor += 1;
            }
            b']' if in_class => {
                in_class = false;
                cursor += 1;
            }
            b'/' if !in_class => {
                cursor += 1;
                while cursor < bytes.len() && bytes[cursor].is_ascii_alphabetic() {
                    cursor += 1;
                }
                return Some(cursor);
            }
            b'\n' | b'\r' => return None,
            _ => cursor += 1,
        }
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
        let (Some(signature), Some(context_name)) = (captures.get(0), captures.get(1)) else {
            return errors;
        };
        let context_name = context_name.as_str();
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
        let (Some(signature), Some(bindings)) = (captures.get(0), captures.get(1)) else {
            return errors;
        };
        let bindings = bindings.as_str();
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

/// Return the source immediately after a direct property in the step's
/// top-level options object. Nested schema fields with the same name do not
/// count; this matters for outputs such as `z.object({ model: z.string() })`.
fn top_level_field_value<'a>(body: &'a str, field: &str) -> Option<&'a str> {
    let bytes = body.as_bytes();
    let mut index = 0;
    let mut quote: Option<u8> = None;
    let mut brace_depth = 0_i32;
    let mut paren_depth = 0_i32;
    let mut bracket_depth = 0_i32;

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
        match byte {
            b'{' => brace_depth += 1,
            b'}' => brace_depth -= 1,
            b'(' => paren_depth += 1,
            b')' => paren_depth -= 1,
            b'[' => bracket_depth += 1,
            b']' => bracket_depth -= 1,
            _ => {}
        }
        if brace_depth == 1
            && paren_depth == 0
            && bracket_depth == 0
            && (byte.is_ascii_alphabetic() || matches!(byte, b'_' | b'$'))
        {
            let start = index;
            index += 1;
            while index < bytes.len() && is_js_identifier_byte(bytes[index]) {
                index += 1;
            }
            let name = &body[start..index];
            let mut colon = index;
            while colon < bytes.len() && bytes[colon].is_ascii_whitespace() {
                colon += 1;
            }
            if name == field && bytes.get(colon) == Some(&b':') {
                return Some(body[colon + 1..].trim_start());
            }
            continue;
        }
        index += 1;
    }
    None
}

fn leading_string_literal(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let delimiter = *bytes.first()?;
    if !matches!(delimiter, b'\'' | b'"' | b'`') {
        return None;
    }
    let mut index = 1;
    while index < bytes.len() {
        if bytes[index] == b'\\' && index + 1 < bytes.len() {
            index += 2;
            continue;
        }
        if bytes[index] == delimiter {
            return Some(unquote(&value[..=index]));
        }
        if delimiter == b'`' && bytes[index] == b'$' {
            return None;
        }
        index += 1;
    }
    None
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
fn extract_retries_field(body: &str) -> Result<Option<JsonMap<String, JsonValue>>, ParseError> {
    let key_re = Regex::new(r"(?m)(^|[\s,{])\s*retries\s*:\s*\{").expect("static regex");
    let Some(m) = key_re.find(body) else {
        return Ok(None);
    };
    let brace_at = body[..m.end()]
        .rfind('{')
        .ok_or_else(|| ParseError::new("invalid `retries` object").field("retries"))?;
    let inner = extract_balanced(body, brace_at, '{', '}')
        .ok_or_else(|| ParseError::new("unbalanced `retries` object").field("retries"))?;
    let max_re = Regex::new(r#"(?m)(^|[\s,{])\s*max\s*:\s*(-?\d+)"#).expect("static regex");
    let max = max_re
        .captures(inner)
        .and_then(|c| c.get(2))
        .ok_or_else(|| {
            ParseError::new(format!(
                "`retries.max` must be an integer from 1 through {MAX_ACTIVITY_ATTEMPTS}"
            ))
            .field("retries.max")
        })?
        .as_str()
        .parse::<u64>()
        .map_err(|_| {
            ParseError::new(format!(
                "`retries.max` must be an integer from 1 through {MAX_ACTIVITY_ATTEMPTS}"
            ))
            .field("retries.max")
        })?;
    if !(1..=u64::from(MAX_ACTIVITY_ATTEMPTS)).contains(&max) {
        return Err(
            ParseError::new(format!(
                "`retries.max` must be from 1 through {MAX_ACTIVITY_ATTEMPTS}; Temporal treats 0 as unlimited"
            ))
            .field("retries.max"),
        );
    }
    let mut out = JsonMap::new();
    out.insert("max".into(), JsonValue::Number(max.into()));
    if let Some(backoff) = extract_string_field(inner, "backoff") {
        if !matches!(backoff.as_str(), "exponential" | "linear") {
            return Err(ParseError::new(
                "`retries.backoff` must be either `\"exponential\"` or `\"linear\"`",
            )
            .field("retries.backoff"));
        }
        out.insert("backoff".into(), JsonValue::String(backoff));
    }
    Ok(Some(out))
}

fn extract_timeout_field(body: &str) -> Result<Option<u64>, ParseError> {
    let key_re = Regex::new(r"(?m)(^|[\s,{])\s*timeout_ms\s*:").expect("static regex");
    let Some(key) = key_re.find(body) else {
        return Ok(None);
    };
    let value = &body[key.end()..];
    let literal_re = Regex::new(r"^\s*(-?\d+)").expect("static regex");
    let literal = literal_re
        .captures(value)
        .and_then(|captures| captures.get(1))
        .ok_or_else(|| {
            ParseError::new(format!(
                "`timeout_ms` must be an integer from 1 through {MAX_ACTIVITY_TIMEOUT_MS}"
            ))
            .field("timeout_ms")
        })?;
    let timeout_ms = literal.as_str().parse::<u64>().map_err(|_| {
        ParseError::new(format!(
            "`timeout_ms` must be an integer from 1 through {MAX_ACTIVITY_TIMEOUT_MS}"
        ))
        .field("timeout_ms")
    })?;
    if !(1..=MAX_ACTIVITY_TIMEOUT_MS).contains(&timeout_ms) {
        return Err(ParseError::new(format!(
            "`timeout_ms` must be from 1 through {MAX_ACTIVITY_TIMEOUT_MS}"
        ))
        .field("timeout_ms"));
    }
    Ok(Some(timeout_ms))
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
    fn rejects_legacy_llm_model() {
        let src = "import { step } from \"@cori-do/sdk\";\nexport default step.llm({ description: \"translate\", model: \"gpt-4o-mini\", prompt: () => `hi` });";
        let errors = parse(src).unwrap_err();
        assert!(errors.iter().any(|error| error.reason.contains("level")));
    }

    #[test]
    fn llm_step_without_a_level_defaults_to_medium() {
        let src = "import { step } from \"@cori-do/sdk\";\nexport default step.llm({ description: \"summarise\", prompt: () => `hi` });";
        let p = parse(src).unwrap();
        assert_eq!(p.kind, StepKind::Llm);
        assert_eq!(p.metadata.get("level").unwrap(), "medium");
    }

    #[test]
    fn llm_step_accepts_all_levels() {
        for level in ["low", "medium", "high"] {
            let src = format!(
                "import {{ step }} from \"@cori-do/sdk\";\nexport default step.llm({{ description: \"triage\", level: \"{level}\", prompt: () => `hi` }});"
            );
            let parsed = parse(&src).unwrap();
            assert_eq!(parsed.metadata.get("level").unwrap(), level);
        }
    }

    #[test]
    fn llm_step_rejects_old_tiers_and_model_names_as_levels() {
        for invalid in ["fast", "balanced", "deep", "gpt-4o-mini"] {
            let src = format!(
                "import {{ step }} from \"@cori-do/sdk\";\nexport default step.llm({{ description: \"triage\", level: \"{invalid}\", prompt: () => `hi` }});"
            );
            let errors = parse(&src).expect_err("invalid level must fail");
            let message = errors
                .iter()
                .map(|error| error.reason.as_str())
                .collect::<Vec<_>>()
                .join(" ");
            assert!(message.contains("low"), "{message}");
            assert!(message.contains("medium"), "{message}");
            assert!(message.contains("high"), "{message}");
        }
    }

    #[test]
    fn nested_output_model_field_is_not_a_legacy_option() {
        let src = "import { step } from \"@cori-do/sdk\"; import { z } from \"zod\";\nexport default step.llm({ description: \"extract\", output: z.object({ model: z.string() }), prompt: () => `hi` });";
        let p = parse(src).unwrap();
        assert_eq!(p.metadata.get("level").unwrap(), "medium");
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
    fn regex_with_escaped_balancing_delimiters_does_not_close_the_step_call() {
        let src = r#"import { step } from "@cori-do/sdk";
export default step.code({
  description: "match delimiters",
  run: ({ value }) => ({
    parenthesis: /\)/u.test(value),
    bracket: /\]/u.test(value),
    brace: /\}/u.test(value),
  }),
});"#;
        let parsed = parse(src).expect("regex delimiters stay inside the step body");
        assert_eq!(parsed.kind, StepKind::Code);
        assert_eq!(parsed.description, "match delimiters");
    }

    #[test]
    fn parses_builtin_map() {
        let src = "import { step } from \"@cori-do/sdk\";\nexport default step.map({ description: \"each\", apply: x });";
        let p = parse(src).unwrap();
        assert_eq!(p.kind, StepKind::Builtin);
        assert_eq!(p.metadata.get("builtin").unwrap(), "map");
    }

    #[test]
    fn parses_builtin_branch_with_nested_steps() {
        let src = r#"import { step } from "@cori-do/sdk";
export default step.branch({
  description: "big or small",
  if: ({ count }) => count > 10,
  then: step.cli({
    description: "summarise the big list",
    command: () => ["gws", "sheets", "append"],
  }),
  else: step.code({
    description: "pass through",
    run: (input) => input,
  }),
});"#;
        let p = parse(src).unwrap();
        assert_eq!(p.kind, StepKind::Builtin);
        assert_eq!(p.description, "big or small");
        assert_eq!(p.metadata.get("builtin").unwrap(), "branch");
        let nested = p.metadata.get("nested").unwrap().as_object().unwrap();
        let then = nested.get("then").unwrap().as_object().unwrap();
        assert_eq!(then.get("kind").unwrap(), "cli");
        assert_eq!(then.get("binary").unwrap(), "gws");
        let alt = nested.get("else").unwrap().as_object().unwrap();
        assert_eq!(alt.get("kind").unwrap(), "code");
    }

    #[test]
    fn branch_requires_if_and_then() {
        let src = "import { step } from \"@cori-do/sdk\";\nexport default step.branch({ description: \"x\" });";
        let errs = parse(src).unwrap_err();
        assert!(errs.iter().any(|e| e.field.as_deref() == Some("if")));
        assert!(errs.iter().any(|e| e.field.as_deref() == Some("then")));
    }

    #[test]
    fn parses_builtin_switch_with_cases_and_default() {
        let src = r#"import { step } from "@cori-do/sdk";
export default step.switch({
  description: "route by severity",
  on: ({ severity }) => severity,
  cases: {
    high: step.mcp_tool({
      description: "page the on-call",
      server: "pagerduty",
      tool: "create_incident",
      args: (input) => ({ summary: input.title }),
    }),
    low: step.llm({
      description: "draft a note",
      level: "low",
      prompt: () => `summarise`,
    }),
  },
  default: step.code({ description: "record unknown", run: (x) => x }),
});"#;
        let p = parse(src).unwrap();
        assert_eq!(p.metadata.get("builtin").unwrap(), "switch");
        let nested = p.metadata.get("nested").unwrap().as_object().unwrap();
        let high = nested.get("cases.high").unwrap().as_object().unwrap();
        assert_eq!(high.get("kind").unwrap(), "mcp_tool");
        assert_eq!(high.get("server").unwrap(), "pagerduty");
        assert_eq!(high.get("tool").unwrap(), "create_incident");
        let low = nested.get("cases.low").unwrap().as_object().unwrap();
        assert_eq!(low.get("kind").unwrap(), "llm");
        assert_eq!(low.get("level").unwrap(), "low");
        assert_eq!(
            nested
                .get("default")
                .unwrap()
                .as_object()
                .unwrap()
                .get("kind")
                .unwrap(),
            "code"
        );
    }

    #[test]
    fn switch_requires_at_least_one_case() {
        let src = "import { step } from \"@cori-do/sdk\";\nexport default step.switch({ description: \"x\", on: (i) => i.k, cases: {} });";
        let errs = parse(src).unwrap_err();
        assert!(errs.iter().any(|e| e.reason.contains("at least one")));
    }

    #[test]
    fn parses_builtin_for_each_with_default_cap() {
        let src = r#"import { step } from "@cori-do/sdk";
export default step.for_each({
  description: "translate every sheet",
  over: ({ sheets }) => sheets,
  apply: step.llm({
    description: "translate one sheet",
    prompt: ({ item }) => `translate ${JSON.stringify(item)}`,
  }),
});"#;
        let p = parse(src).unwrap();
        assert_eq!(p.metadata.get("builtin").unwrap(), "for_each");
        assert_eq!(
            p.metadata.get("max_items").unwrap().as_u64().unwrap(),
            DEFAULT_FOR_EACH_ITEMS
        );
        let nested = p.metadata.get("nested").unwrap().as_object().unwrap();
        let apply = nested.get("apply").unwrap().as_object().unwrap();
        assert_eq!(apply.get("kind").unwrap(), "llm");
        assert_eq!(apply.get("level").unwrap(), "medium");
    }

    #[test]
    fn parses_builtin_loop_with_bounds() {
        let src = r#"import { step } from "@cori-do/sdk";
export default step.loop({
  description: "poll until exported",
  body: step.cli({
    description: "poll the export job",
    command: ({ job_id }) => ["gws", "drive", "export-status", job_id],
    retries: { max: 2 },
  }),
  until: ({ export_state }) => export_state === "done",
  max_iterations: 25,
});"#;
        let p = parse(src).unwrap();
        assert_eq!(p.metadata.get("builtin").unwrap(), "loop");
        assert_eq!(p.metadata.get("max_iterations").unwrap().as_u64(), Some(25));
        let nested = p.metadata.get("nested").unwrap().as_object().unwrap();
        let body = nested.get("body").unwrap().as_object().unwrap();
        assert_eq!(body.get("kind").unwrap(), "cli");
        assert_eq!(body.get("binary").unwrap(), "gws");
        let retries = body.get("retries").unwrap().as_object().unwrap();
        assert_eq!(retries.get("max").unwrap(), 2);
    }

    #[test]
    fn loop_iteration_bound_is_enforced() {
        let src = "import { step } from \"@cori-do/sdk\";\nexport default step.loop({ description: \"x\", body: step.code({ description: \"b\", run: (x) => x }), until: (i) => true, max_iterations: 5000 });";
        let errs = parse(src).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.field.as_deref() == Some("max_iterations"))
        );
    }

    #[test]
    fn parses_builtin_wait_variants() {
        let delay = "import { step } from \"@cori-do/sdk\";\nexport default step.wait({ description: \"cool down\", for: { timeout_ms: 60000 } });";
        let p = parse(delay).unwrap();
        assert_eq!(p.metadata.get("builtin").unwrap(), "wait");
        let wait = p.metadata.get("wait").unwrap().as_object().unwrap();
        assert_eq!(wait.get("timeout_ms").unwrap().as_u64(), Some(60000));

        let until = "import { step } from \"@cori-do/sdk\";\nexport default step.wait({ description: \"until launch\", for: { until: \"2026-09-01T09:00:00Z\" } });";
        let p = parse(until).unwrap();
        let wait = p.metadata.get("wait").unwrap().as_object().unwrap();
        assert_eq!(wait.get("until").unwrap(), "2026-09-01T09:00:00Z");

        let signal = "import { step } from \"@cori-do/sdk\";\nexport default step.wait({ description: \"await approval\", for: { signal: \"approved\", timeout_ms: 3600000 } });";
        let p = parse(signal).unwrap();
        let wait = p.metadata.get("wait").unwrap().as_object().unwrap();
        assert_eq!(wait.get("signal").unwrap(), "approved");
        assert_eq!(wait.get("timeout_ms").unwrap().as_u64(), Some(3600000));
    }

    #[test]
    fn wait_rejects_empty_or_invalid_specs() {
        let empty = "import { step } from \"@cori-do/sdk\";\nexport default step.wait({ description: \"x\", for: {} });";
        let errs = parse(empty).unwrap_err();
        assert!(errs.iter().any(|e| e.reason.contains("at least one")));

        let bad_until = "import { step } from \"@cori-do/sdk\";\nexport default step.wait({ description: \"x\", for: { until: \"tomorrow\" } });";
        let errs = parse(bad_until).unwrap_err();
        assert!(errs.iter().any(|e| e.field.as_deref() == Some("for.until")));

        let offsetless = "import { step } from \"@cori-do/sdk\";\nexport default step.wait({ description: \"x\", for: { until: \"2026-09-01T09:00:00\" } });";
        let errs = parse(offsetless).unwrap_err();
        assert!(errs.iter().any(|e| e.field.as_deref() == Some("for.until")));
    }

    #[test]
    fn builtins_cannot_nest_builtins() {
        let src = "import { step } from \"@cori-do/sdk\";\nexport default step.branch({ description: \"x\", if: (i) => true, then: step.wait({ description: \"w\", for: { timeout_ms: 1 } }) });";
        let errs = parse(src).unwrap_err();
        assert!(errs.iter().any(|e| e.reason.contains("cannot nest")));
    }

    #[test]
    fn nested_steps_must_be_inline_calls() {
        let src = "import { step } from \"@cori-do/sdk\";\nconst helper = step.code({ description: \"h\", run: (x) => x });\nexport default step.branch({ description: \"x\", if: (i) => true, then: helper });";
        let errs = parse(src).unwrap_err();
        assert!(errs.iter().any(|e| e.reason.contains("inline")));
    }

    #[test]
    fn builtin_description_ignores_nested_descriptions() {
        let src = r#"import { step } from "@cori-do/sdk";
export default step.branch({
  if: ({ ok }) => ok,
  then: step.code({ description: "nested only", run: (x) => x }),
});"#;
        let errs = parse(src).unwrap_err();
        assert!(errs.iter().any(|e| e.reason.contains("description")));
    }

    #[test]
    fn nested_llm_rejects_legacy_model() {
        let src = "import { step } from \"@cori-do/sdk\";\nexport default step.branch({ description: \"x\", if: (i) => true, then: step.llm({ description: \"n\", model: \"gpt-4o\", prompt: () => `p` }) });";
        let errs = parse(src).unwrap_err();
        assert!(errs.iter().any(|e| e.reason.contains("level")));
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
    fn retries_must_be_finite_and_bounded() {
        for max in ["0", "-1", "11", "999999999999999999999999999"] {
            let src = format!(
                "import {{ step }} from \"@cori-do/sdk\";\nexport default step.code({{ description: \"x\", retries: {{ max: {max} }}, run: (x) => x }});"
            );
            let errors = parse(&src).expect_err("unsafe retry count must fail");
            assert!(
                errors
                    .iter()
                    .any(|error| error.field.as_deref() == Some("retries.max")),
                "{max}: {errors:?}"
            );
        }
    }

    #[test]
    fn timeout_is_extracted_and_bounded() {
        let src = "import { step } from \"@cori-do/sdk\";\nexport default step.code({ description: \"x\", timeout_ms: 2500, run: (x) => x });";
        let parsed = parse(src).expect("literal timeout");
        assert_eq!(
            parsed.metadata.get("timeout_ms"),
            Some(&JsonValue::from(2500))
        );

        for timeout in ["0", "-1", "3600001", "timeout"] {
            let src = format!(
                "import {{ step }} from \"@cori-do/sdk\";\nexport default step.code({{ description: \"x\", timeout_ms: {timeout}, run: (x) => x }});"
            );
            let errors = parse(&src).expect_err("unsafe timeout must fail");
            assert!(
                errors
                    .iter()
                    .any(|error| error.field.as_deref() == Some("timeout_ms")),
                "{timeout}: {errors:?}"
            );
        }
    }

    #[test]
    fn no_retries_field_means_no_metadata() {
        let src = "import { step } from \"@cori-do/sdk\";\nexport default step.code({ description: \"x\", run: (x) => x });";
        let p = parse(src).unwrap();
        assert!(p.metadata.get("retries").is_none());
    }
}
