//! Dry-run support: synthesize a mock [`ActivityOutcome`] without spawning
//! anything external.
//!
//! `cori run --dry-run` exercises the entire pipeline — capability
//! discovery, schema/route validation, the per-step trace shape — except
//! that every step that would touch the outside world returns a schema-valid
//! stub. `code` and `builtin` steps still run for real (they're pure).
//!
//! Dry-run outputs deliberately contain no trace annotations: they participate
//! in downstream schema dataflow exactly like real outputs. The caller marks
//! the activity with [`ActivityStatus::Skipped`] and a `notes` field so the
//! trace can still render "DRY RUN — no external calls".

use std::path::Path;
use std::time::Duration;

use serde_json::{Value as JsonValue, json};

use crate::dispatch::{self, RunnerMode};
use crate::runtime::Runtime;
use crate::{ActivityOutcome, ActivityStatus, BrokerError, Result};

/// Mock a `cli` step: evaluate and boundary-check the user's `command(input)`
/// builder, then return a schema-valid output without spawning the binary.
pub fn cli(
    runtime: &Runtime,
    step_file_path: &Path,
    input: &JsonValue,
    expected_binary: Option<&str>,
) -> Result<ActivityOutcome> {
    let call = dispatch::invoke_with_input(runtime, step_file_path, RunnerMode::CliCommand, input)?;
    let binary = call
        .output
        .get("command")
        .and_then(JsonValue::as_array)
        .and_then(|argv| argv.first())
        .and_then(JsonValue::as_str)
        .ok_or_else(|| BrokerError::StepFailed {
            message: "cli step produced an empty command".to_string(),
            stack: None,
        })?;
    crate::cli::validate_binary_boundary(expected_binary, binary)?;
    let preview = call
        .output
        .get("command")
        .cloned()
        .unwrap_or(JsonValue::Null);
    let stub = output_stub(runtime, step_file_path)?;
    Ok(ActivityOutcome {
        status: ActivityStatus::Skipped,
        output: stub.output,
        duration: call.duration + stub.duration,
        stderr: combine_stderr(call.stderr, stub.stderr),
        cost_eur: None,
        usage: None,
        notes: vec![format!("would run CLI argv: {preview}")],
    })
}

/// Mock an `mcp_tool` step: validate the user's `args(input)` builder and its
/// frozen target, then return a schema-valid output without speaking to the
/// server.
pub fn mcp(
    runtime: &Runtime,
    step_file_path: &Path,
    input: &JsonValue,
    expected_server: Option<&str>,
    expected_tool: Option<&str>,
) -> Result<ActivityOutcome> {
    let call = dispatch::invoke_with_input(runtime, step_file_path, RunnerMode::McpArgs, input)?;
    let actual_server = call
        .output
        .get("server")
        .and_then(JsonValue::as_str)
        .ok_or_else(|| BrokerError::StepFailed {
            message: "mcp_tool step produced no server".to_string(),
            stack: None,
        })?;
    let actual_tool = call
        .output
        .get("tool")
        .and_then(JsonValue::as_str)
        .ok_or_else(|| BrokerError::StepFailed {
            message: "mcp_tool step produced no tool".to_string(),
            stack: None,
        })?;
    crate::mcp::validate_target_boundary(
        expected_server,
        expected_tool,
        actual_server,
        actual_tool,
    )?;
    let stub = output_stub(runtime, step_file_path)?;
    Ok(ActivityOutcome {
        status: ActivityStatus::Skipped,
        output: stub.output,
        duration: call.duration + stub.duration,
        stderr: combine_stderr(call.stderr, stub.stderr),
        cost_eur: None,
        usage: None,
        notes: vec![format!("would call MCP tool: {}", call.output)],
    })
}

/// Mock an `llm` step: validate its input, prompt builder, and frozen model,
/// then synthesize a schema-valid output without contacting a provider.
pub fn llm(
    runtime: &Runtime,
    step_file_path: &Path,
    input: &JsonValue,
    expected_model: Option<&str>,
) -> Result<ActivityOutcome> {
    let prompt =
        dispatch::invoke_with_input(runtime, step_file_path, RunnerMode::LlmPrompt, input)?;
    let actual_model = prompt
        .output
        .get("model")
        .and_then(JsonValue::as_str)
        .ok_or_else(|| BrokerError::StepFailed {
            message: "llm step produced no model".to_string(),
            stack: None,
        })?;
    crate::llm::validate_model_boundary(expected_model, actual_model)?;
    let stub = output_stub(runtime, step_file_path)?;
    Ok(ActivityOutcome {
        status: ActivityStatus::Skipped,
        output: stub.output,
        duration: prompt.duration + stub.duration,
        stderr: combine_stderr(prompt.stderr, stub.stderr),
        cost_eur: Some(0.0),
        usage: None,
        notes: Vec::new(),
    })
}

fn output_stub(runtime: &Runtime, step_file_path: &Path) -> Result<dispatch::RunnerCall> {
    dispatch::invoke(runtime, step_file_path, RunnerMode::OutputStub, &json!({}))
}

fn combine_stderr(left: String, right: String) -> String {
    match (left.trim().is_empty(), right.trim().is_empty()) {
        (true, _) => right,
        (_, true) => left,
        (false, false) => format!("{left}\n{right}"),
    }
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
        notes: Vec::new(),
    }
}
