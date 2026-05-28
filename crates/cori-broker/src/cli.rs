//! Dispatch a `cli` step.
//!
//! The flow is:
//!
//! 1. Invoke the runner in `cli_command` mode to materialise the argv +
//!    optional env additions from the user's `command(input)` builder.
//! 2. Check the resolved binary against the worker's capability whitelist
//!    (computed by [`crate::capabilities`]). The whitelist comes from the
//!    workflow's `tools_required` declaration, so a step trying to spawn
//!    `kubectl` from a workflow that only declared `gws` is refused
//!    *before* the process spawns.
//! 3. Spawn the binary with `std::process::Command`, capturing stdout,
//!    stderr, and exit code.
//! 4. Invoke the runner in `cli_parse` mode to translate stdout into the
//!    typed output (or `JSON.parse(stdout)` when no `parse` is declared).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

use serde::Deserialize;
use serde_json::{Value as JsonValue, json};

use crate::capabilities::Capabilities;
use crate::cli_auth::{self, AuthState};
use crate::dispatch::{self, RunnerMode};
use crate::runtime::Runtime;
use crate::{ActivityOutcome, ActivityStatus, BrokerError, Result};

#[derive(Debug, Deserialize)]
struct CommandSpec {
    command: Vec<String>,
    #[serde(default)]
    env: Option<HashMap<String, String>>,
}

/// Run one `cli` step.
pub fn run(
    runtime: &Runtime,
    capabilities: &Capabilities,
    step_file_path: &Path,
    input: &JsonValue,
    user_id: &str,
) -> Result<ActivityOutcome> {
    let started = Instant::now();

    // 1. Resolve argv via the runner.
    let cmd_call =
        dispatch::invoke_with_input(runtime, step_file_path, RunnerMode::CliCommand, input)?;
    let spec: CommandSpec =
        serde_json::from_value(cmd_call.output.clone()).map_err(|e| BrokerError::BadEnvelope {
            envelope: cmd_call.output.to_string(),
            source: e,
        })?;
    let argv = spec.command;
    let binary = argv
        .first()
        .cloned()
        .ok_or_else(|| BrokerError::StepFailed {
            message: "cli step produced an empty command".to_string(),
            stack: None,
        })?;

    // 2. Capability check.
    let resolved_bin: PathBuf = match capabilities.cli_binaries.get(&binary) {
        Some(p) => p.clone(),
        None => {
            return Err(BrokerError::CapabilityDenied {
                kind: "CLI",
                name: binary.clone(),
                hint: format!(
                    "binary `{binary}` is not in the workflow's `tools_required` whitelist or is not installed on PATH"
                ),
            });
        }
    };

    // 2b. Per-CLI auth check (Phase 5). For known CLIs that carry their
    //     own login state (e.g. `gws`), refuse to spawn when the CLI is
    //     not authenticated so the user sees a clean `NeedsReauth`
    //     instead of an opaque 401 from the CLI itself.
    if let AuthState::NeedsReauth { hint } = cli_auth::check_known(&binary) {
        return Err(BrokerError::NeedsReauth {
            server_id: binary.clone(),
            owner_kind: "user",
            owner_id: user_id.to_string(),
            auth_kind: "cli",
            hint,
        });
    }

    // 3. Spawn.
    let mut cmd = Command::new(&resolved_bin);
    cmd.args(&argv[1..]);
    if let Some(env) = &spec.env {
        for (k, v) in env {
            cmd.env(k, v);
        }
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let proc_output = cmd.output().map_err(|e| BrokerError::CliSpawn {
        binary: binary.clone(),
        source: e,
    })?;
    let stdout_str = String::from_utf8_lossy(&proc_output.stdout).into_owned();
    let stderr_str = String::from_utf8_lossy(&proc_output.stderr).into_owned();
    let exit_code = proc_output.status.code().unwrap_or(-1);

    if !proc_output.status.success() {
        // Surface the failure with the captured stderr in the error message
        // — the run loop also stores it on the trace.
        return Err(BrokerError::CliExitNonZero {
            binary,
            exit_code,
            stderr: truncate(&stderr_str, 4096),
        });
    }

    // 4. Parse stdout via the runner.
    let parse_payload = json!({
        "input": input,
        "parseCtx": {
            "stdout": stdout_str,
            "stderr": stderr_str,
            "exitCode": exit_code,
        }
    });
    let parse_call = dispatch::invoke(
        runtime,
        step_file_path,
        RunnerMode::CliParse,
        &parse_payload,
    )?;

    Ok(ActivityOutcome {
        status: ActivityStatus::Ok,
        output: parse_call.output,
        duration: started.elapsed(),
        stderr: combine_stderr(&cmd_call.stderr, &stderr_str, &parse_call.stderr),
        cost_eur: None,
        usage: None,
    })
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let mut t = s[..max].to_string();
        t.push_str("\n…(truncated)");
        t
    }
}

fn combine_stderr(runner_a: &str, child: &str, runner_b: &str) -> String {
    let mut out = String::new();
    if !runner_a.trim().is_empty() {
        out.push_str("[runner: cli_command]\n");
        out.push_str(runner_a);
        if !runner_a.ends_with('\n') {
            out.push('\n');
        }
    }
    if !child.trim().is_empty() {
        out.push_str("[cli stderr]\n");
        out.push_str(child);
        if !child.ends_with('\n') {
            out.push('\n');
        }
    }
    if !runner_b.trim().is_empty() {
        out.push_str("[runner: cli_parse]\n");
        out.push_str(runner_b);
    }
    out
}
