//! Dry-run support: synthesize a mock [`ActivityOutcome`] without spawning
//! anything external.
//!
//! `cori run --dry-run` exercises the entire pipeline — capability
//! discovery, schema/route validation, the per-step trace shape — except
//! that every step that would touch the outside world returns a placeholder
//! annotated with `mocked: true`. `code` and `builtin` steps still run for
//! real (they're pure).
//!
//! The mocked output is a JSON object with a single `mocked: true` field
//! plus a kind-specific summary so the trace remains readable. The
//! caller marks the activity with [`ActivityStatus::Skipped`] and a
//! `notes` field so the trace can render "DRY RUN — no external calls".

use std::path::Path;
use std::time::Duration;

use serde_json::{Value as JsonValue, json};

use crate::dispatch::{self, RunnerMode};
use crate::runtime::Runtime;
use crate::{ActivityOutcome, ActivityStatus, Result};

/// Mock a `cli` step: evaluate the user's `command(input)` builder so the
/// trace shows the actual argv that would have run, but never spawn the
/// binary.
pub fn cli(runtime: &Runtime, step_file_path: &Path, input: &JsonValue) -> Result<ActivityOutcome> {
    let call = dispatch::invoke_with_input(runtime, step_file_path, RunnerMode::CliCommand, input)?;
    Ok(ActivityOutcome {
        status: ActivityStatus::Skipped,
        output: json!({
            "mocked": true,
            "kind": "cli",
            "argv": call.output.get("command").cloned().unwrap_or(JsonValue::Null),
        }),
        duration: call.duration,
        stderr: call.stderr,
        cost_eur: None,
        usage: None,
    })
}

/// Mock an `mcp_tool` step: evaluate the user's `args(input)` builder so
/// the trace shows the tool call shape, but never speak to the server.
pub fn mcp(runtime: &Runtime, step_file_path: &Path, input: &JsonValue) -> Result<ActivityOutcome> {
    let call = dispatch::invoke_with_input(runtime, step_file_path, RunnerMode::McpArgs, input)?;
    Ok(ActivityOutcome {
        status: ActivityStatus::Skipped,
        output: json!({
            "mocked": true,
            "kind": "mcp_tool",
            "call": call.output,
        }),
        duration: call.duration,
        stderr: call.stderr,
        cost_eur: None,
        usage: None,
    })
}

/// Mock an `llm` step: ask the runner for the step's stubbed default
/// output (`runner.ts` `llm_stub` mode), matching the declared output
/// schema. No HTTP request is made.
pub fn llm(runtime: &Runtime, step_file_path: &Path, input: &JsonValue) -> Result<ActivityOutcome> {
    let call = dispatch::invoke_with_input(runtime, step_file_path, RunnerMode::LlmStub, input)?;
    let mut output = match call.output {
        JsonValue::Object(mut m) => {
            m.insert("mocked".into(), JsonValue::Bool(true));
            JsonValue::Object(m)
        }
        other => json!({ "mocked": true, "stub": other }),
    };
    if let JsonValue::Object(m) = &mut output {
        m.entry("kind").or_insert(JsonValue::String("llm".into()));
    }
    Ok(ActivityOutcome {
        status: ActivityStatus::Skipped,
        output,
        duration: call.duration,
        stderr: call.stderr,
        cost_eur: Some(0.0),
        usage: None,
    })
}

/// Fallback when the runner is unavailable but the caller still wants a
/// placeholder (e.g. an environment without Deno doing a paper-only dry
/// run).
pub fn synthetic(kind: &'static str) -> ActivityOutcome {
    ActivityOutcome {
        status: ActivityStatus::Skipped,
        output: json!({ "mocked": true, "kind": kind, "note": "no runner available" }),
        duration: Duration::from_millis(0),
        stderr: String::new(),
        cost_eur: None,
        usage: None,
    }
}
