//! Capability broker: the trust boundary for all external side-effects.
//!
//! Every step kind that touches the outside world (`cli`, `mcp_tool`,
//! `llm`) — plus the `code` kind, which evaluates user TypeScript in a
//! sandboxed Deno subprocess — is dispatched through this crate. The CLI
//! and worker stay free of `std::process::Command` calls; only the broker
//! spawns external processes, which gives us one place to enforce
//! capability declarations.
//!
//! This crate currently handles `code`, `cli`, `mcp_tool`, and `llm`
//! dispatch.
//!
//! All public entry points return [`ActivityOutcome`] so the run loop in
//! `cori-cli` can record a uniform trace regardless of which kind ran.
//!
//! ## Layout
//!
//! - [`runtime`] resolves the Deno binary and the bundled runner script.
//! - [`dispatch`] is the generic Deno-runner subprocess wrapper.
//! - [`code`] runs `code` activities.
//! - [`cli`] runs `cli` activities.
//! - [`mcp`] runs `mcp_tool` activities.
//! - [`llm`] runs `llm` activities.
//! - [`capabilities`] discovers worker capabilities at startup.

pub mod capabilities;
pub mod cli;
pub mod cli_auth;
pub mod code;
pub mod credentials;
pub mod dispatch;
pub mod dry_run;
pub mod identity;
pub mod llm;
pub mod mcp;
pub mod oauth;
pub mod runtime;

use std::time::Duration;

use serde::Serialize;
use serde_json::Value as JsonValue;
use thiserror::Error;

/// One step's worth of execution result. Returned by every broker entry
/// point so the CLI can append a uniform row to the run's trace.
#[derive(Debug, Clone, Serialize)]
pub struct ActivityOutcome {
    pub status: ActivityStatus,
    /// Decoded JSON output of the activity. `Null` when the activity
    /// failed or was skipped.
    pub output: JsonValue,
    pub duration: Duration,
    /// Captured stderr from the subprocess. Useful for surfacing in the
    /// CLI; the worker stores a truncated copy on the trace.
    pub stderr: String,
    /// Monetary cost in EUR, if this activity made a paid API call.
    /// `None` for kinds that don't incur per-call cost (most CLI, MCP,
    /// code).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_eur: Option<f64>,
    /// Token usage breakdown for LLM activities.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<TokenUsage>,
}

/// Per-activity token accounting for LLM calls.
/// Re-exported from `cori-protocol` for backward compatibility.
pub use cori_protocol::TokenUsage;

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ActivityStatus {
    Ok,
    Failed,
    Skipped,
}

/// Errors produced by the broker. Application code converts these into
/// CLI-friendly diagnostics; the broker itself never prints.
#[derive(Debug, Error)]
pub enum BrokerError {
    #[error(
        "Deno runtime is not available: {0}\n\nInstall Deno from https://deno.land or set CORI_DENO to a Deno binary."
    )]
    RuntimeUnavailable(String),

    #[error("runner subprocess failed to spawn: {0}")]
    Spawn(#[source] std::io::Error),

    #[error("runner subprocess I/O error: {0}")]
    Io(#[source] std::io::Error),

    #[error("runner produced no envelope on stdout (exit {exit_code})\n--- stderr ---\n{stderr}")]
    MissingEnvelope { exit_code: i32, stderr: String },

    #[error("runner envelope was not valid JSON: {source}\n--- envelope ---\n{envelope}")]
    BadEnvelope {
        envelope: String,
        #[source]
        source: serde_json::Error,
    },

    #[error("step failed: {message}")]
    StepFailed {
        message: String,
        stack: Option<String>,
    },

    #[error("capability denied: {kind} `{name}` — {hint}")]
    CapabilityDenied {
        kind: &'static str,
        name: String,
        hint: String,
    },

    #[error("failed to spawn CLI `{binary}`: {source}")]
    CliSpawn {
        binary: String,
        #[source]
        source: std::io::Error,
    },

    #[error("CLI `{binary}` exited with code {exit_code}\n--- stderr ---\n{stderr}")]
    CliExitNonZero {
        binary: String,
        exit_code: i32,
        stderr: String,
    },

    #[error("failed to spawn MCP server `{binary}`: {source}")]
    McpSpawn {
        binary: String,
        #[source]
        source: std::io::Error,
    },

    #[error("MCP protocol error: {0}")]
    McpProtocol(String),

    #[error(
        "no LLM provider matches model `{model}` — supported model prefixes: gpt-/o1-/o3-/o4- (OpenAI), claude- (Anthropic), gemini- (Gemini)"
    )]
    LlmUnknownModel { model: String },

    #[error(
        "LLM credentials missing for provider `{provider}` — set the {env_var} environment variable, or run `cori config set llm.{provider}.api_key <key>`"
    )]
    LlmMissingCredentials {
        provider: &'static str,
        env_var: &'static str,
    },

    #[error("LLM HTTP error: {0}")]
    LlmHttp(#[source] reqwest::Error),

    #[error("LLM provider `{provider}` returned an error ({status}): {body}")]
    LlmProviderError {
        provider: &'static str,
        status: u16,
        body: String,
    },

    #[error(
        "LLM provider `{provider}` returned a response that did not match the requested schema after {attempts} attempt(s): {reason}"
    )]
    LlmSchemaMismatch {
        provider: &'static str,
        attempts: u32,
        reason: String,
    },

    /// A capability that requires per-user authentication has no usable
    /// credential for the requesting owner. The CLI surfaces `hint` to
    /// the user; the Temporal classifier marks this non-retryable so
    /// retries don't loop while the user goes through `cori login`.
    /// Phase 6 wires a workflow-side signal handler that catches this
    /// error type, waits for `cori login`, and resumes the step.
    #[error("{server_id} needs sign-in for {owner_kind} `{owner_id}` — {hint}")]
    NeedsReauth {
        server_id: String,
        owner_kind: &'static str,
        owner_id: String,
        auth_kind: &'static str,
        hint: String,
    },
}

/// What invoked the run. v1 supports only `Cli`; the enum exists so the
/// LLM broker can pick the right provider strategy (org-configured vs.
/// MCP-sampling vs. scheduled) in later execution modes.
#[derive(Debug, Clone, Copy)]
pub enum TriggerContext {
    Cli,
}

pub type Result<T> = std::result::Result<T, BrokerError>;
