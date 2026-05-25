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
/// step's `.ts` source on disk.
pub fn run(runtime: &Runtime, step_file_path: &Path, input: &JsonValue) -> Result<ActivityOutcome> {
    let call = dispatch::invoke_with_input(runtime, step_file_path, RunnerMode::Code, input)?;
    Ok(ActivityOutcome {
        status: ActivityStatus::Ok,
        output: call.output,
        duration: call.duration,
        stderr: call.stderr,
        cost_eur: None,
        usage: None,
    })
}
