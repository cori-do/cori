//! Dispatch a `code` step to a Deno subprocess.
//!
//! Thin wrapper over [`crate::dispatch::invoke_with_input`]: pure-function
//! user TypeScript is evaluated in the runner and its return value is
//! returned as the activity output.

use std::path::Path;

use serde_json::Value as JsonValue;

use crate::dispatch::{self, RunnerMode};
use crate::runtime::Runtime;
use crate::{ActivityOutcome, ActivityStatus, Result};

/// Run one `code` step. `step_file_path` must be the absolute path to the
/// step's `.ts` source on disk. `selector` addresses a builtin's nested
/// `code` step; `None` runs the file's default export.
pub fn run(
    runtime: &Runtime,
    step_file_path: &Path,
    input: &JsonValue,
    selector: Option<&str>,
) -> Result<ActivityOutcome> {
    let call =
        dispatch::invoke_with_input(runtime, step_file_path, RunnerMode::Code, input, selector)?;
    Ok(ActivityOutcome {
        status: ActivityStatus::Ok,
        output: call.output,
        duration: call.duration,
        stderr: call.stderr,
        cost_eur: None,
        usage: None,
        notes: Vec::new(),
    })
}

/// Evaluate a builtin step's pure selector function (`if` / `on` / `over`
/// / `until`). Same sandbox as a `code` step: read-only workflow folder,
/// no network, no subprocesses.
pub fn eval_builtin(
    runtime: &Runtime,
    step_file_path: &Path,
    eval_fn: &str,
    input: &JsonValue,
) -> Result<ActivityOutcome> {
    let call = dispatch::invoke_builtin_eval(runtime, step_file_path, eval_fn, input)?;
    Ok(ActivityOutcome {
        status: ActivityStatus::Ok,
        output: call.output,
        duration: call.duration,
        stderr: call.stderr,
        cost_eur: None,
        usage: None,
        notes: Vec::new(),
    })
}
