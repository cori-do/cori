//! Generic Deno runner dispatch.
//!
//! All current step kinds that need to evaluate user-authored JavaScript
//! (the `code` step's `run`, the `cli` step's `command`/`parse`, etc.) go
//! through this module. It spawns the bundled Deno runner with a
//! `mode` argv, writes a JSON payload to stdin, and parses one envelope
//! line from stdout.
//!
//! Side-effects (spawning external CLIs, calling MCP servers, hitting
//! LLM APIs) happen in Rust, not Deno — this module is only used to
//! evaluate pure user expressions inside a sandbox.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{Value as JsonValue, json};
use tracing::debug;

use crate::BrokerError;
use crate::process::hide_console_window;
use crate::runtime::Runtime;

pub const ENVELOPE_PREFIX: &str = "\u{001E}CORI_RUNNER\u{001E}";

/// Result of one runner invocation.
#[derive(Debug, Clone)]
pub struct RunnerCall {
    pub output: JsonValue,
    pub duration: Duration,
    pub stderr: String,
}

/// Modes recognised by `runner.ts`. Mirror the switch-cases there.
#[derive(Debug, Clone, Copy)]
pub enum RunnerMode {
    Code,
    CliCommand,
    CliParse,
    McpArgs,
    LlmPrompt,
    LlmStub,
}

impl RunnerMode {
    fn as_str(self) -> &'static str {
        match self {
            RunnerMode::Code => "code",
            RunnerMode::CliCommand => "cli_command",
            RunnerMode::CliParse => "cli_parse",
            RunnerMode::McpArgs => "mcp_args",
            RunnerMode::LlmPrompt => "llm_prompt",
            RunnerMode::LlmStub => "llm_stub",
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum Envelope {
    Ok { ok: bool, output: JsonValue },
    Err { ok: bool, error: EnvelopeError },
}

#[derive(Debug, Deserialize)]
struct EnvelopeError {
    message: String,
    #[serde(default)]
    stack: Option<String>,
}

/// Invoke the runner. `payload` is written verbatim to stdin (it should
/// be a JSON object — by convention with at least an `input` field).
pub fn invoke(
    runtime: &Runtime,
    step_file_path: &Path,
    mode: RunnerMode,
    payload: &JsonValue,
) -> crate::Result<RunnerCall> {
    let payload_bytes = serde_json::to_vec(payload).expect("payload is always serializable");

    let started = Instant::now();
    debug!(
        deno = %runtime.deno_bin.display(),
        runner = %runtime.runner_script.display(),
        step = %step_file_path.display(),
        mode = mode.as_str(),
        "spawning deno runner",
    );

    let mut cmd = Command::new(&runtime.deno_bin);
    cmd.arg("run")
        .arg("--quiet")
        .arg("--no-prompt")
        .arg("--sloppy-imports")
        .arg("--allow-read")
        .arg("--allow-env")
        .arg("--allow-net=registry.npmjs.org,esm.sh,jsr.io")
        .arg("--config")
        .arg(&runtime.config_path)
        .arg(&runtime.runner_script)
        .arg(step_file_path)
        .arg(mode.as_str())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    hide_console_window(&mut cmd);
    let mut child = cmd.spawn().map_err(BrokerError::Spawn)?;

    {
        let mut stdin = child.stdin.take().expect("stdin was configured as piped");
        stdin.write_all(&payload_bytes).map_err(BrokerError::Io)?;
    }

    let output = child.wait_with_output().map_err(BrokerError::Io)?;
    let duration = started.elapsed();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();

    let envelope_json = match extract_envelope(&stdout) {
        Some(s) => s,
        None => {
            return Err(BrokerError::MissingEnvelope {
                exit_code: output.status.code().unwrap_or(-1),
                stderr,
            });
        }
    };

    let parsed: Envelope =
        serde_json::from_str(envelope_json).map_err(|e| BrokerError::BadEnvelope {
            envelope: envelope_json.to_string(),
            source: e,
        })?;

    match parsed {
        Envelope::Ok { ok: true, output } => Ok(RunnerCall {
            output,
            duration,
            stderr,
        }),
        Envelope::Err {
            ok: false,
            error: EnvelopeError { message, stack },
        } => Err(BrokerError::StepFailed { message, stack }),
        Envelope::Ok { ok: false, .. } | Envelope::Err { ok: true, .. } => {
            Err(BrokerError::BadEnvelope {
                envelope: envelope_json.to_string(),
                source: serde_json::Error::io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "envelope ok flag is inconsistent",
                )),
            })
        }
    }
}

/// Convenience: invoke with `{ "input": <value> }`.
pub fn invoke_with_input(
    runtime: &Runtime,
    step_file_path: &Path,
    mode: RunnerMode,
    input: &JsonValue,
) -> crate::Result<RunnerCall> {
    invoke(runtime, step_file_path, mode, &json!({ "input": input }))
}

/// Find the last `ENVELOPE_PREFIX`-marked line in the runner's stdout and
/// return the JSON that follows it (stripped of trailing whitespace).
pub fn extract_envelope(stdout: &str) -> Option<&str> {
    let idx = stdout.rfind(ENVELOPE_PREFIX)?;
    let after = &stdout[idx + ENVELOPE_PREFIX.len()..];
    let end = after.find('\n').unwrap_or(after.len());
    let candidate = after[..end].trim();
    if candidate.is_empty() {
        None
    } else {
        Some(candidate)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_envelope_finds_last_marker() {
        let s = format!("user log line\n{ENVELOPE_PREFIX}{{\"ok\":true,\"output\":42}}\n");
        assert_eq!(extract_envelope(&s), Some("{\"ok\":true,\"output\":42}"));
    }

    #[test]
    fn extract_envelope_none_without_marker() {
        assert!(extract_envelope("nothing here\n").is_none());
    }

    #[test]
    fn extract_envelope_prefers_last_marker() {
        let s = format!(
            "{ENVELOPE_PREFIX}{{\"ok\":true,\"output\":1}}\n\
             user later\n\
             {ENVELOPE_PREFIX}{{\"ok\":true,\"output\":2}}\n"
        );
        assert_eq!(extract_envelope(&s), Some("{\"ok\":true,\"output\":2}"));
    }
}
