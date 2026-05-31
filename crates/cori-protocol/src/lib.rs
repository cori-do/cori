//! Wire types shared between the CLI, worker, and Deno runner.
//!
//! [`CompiledWorkflow`] is the canonical representation a validated workflow
//! compiles to. The CLI stores it, the worker executes it, and the trace
//! recorder references its `activity_id` values.

pub mod trace;
pub use trace::{ActivityTrace, CostSummary, RunTrace, TokenUsage, WorkflowSource};

use serde::{Deserialize, Serialize};

pub use cori_manifest::Manifest;

/// Closed set of step kinds. Mirrors the SDK's `StepKind`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum StepKind {
    Cli,
    McpTool,
    Code,
    Llm,
    Builtin,
}

/// Retry policy attached to a step. Mirrors the SDK's `retries` field.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RetryPolicy {
    pub max: u32,
    pub backoff: BackoffKind,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BackoffKind {
    Exponential,
    Linear,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max: 3,
            backoff: BackoffKind::Exponential,
        }
    }
}

/// Where a step is allowed to run. Set by the compiler from
/// `kind` + static metadata; consumed by the CLI planner (Phase 4) to
/// resolve a concrete task queue per step.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Placement {
    /// Pure step (`code`, `llm`, `builtin`). Any worker may run it.
    #[default]
    Anywhere,
    /// Needs the requesting user's local filesystem (`cli`, and `code`
    /// steps that declare `reads_path` / `writes_path` metadata).
    /// Routed to `cori.user.<requesting_user>`.
    RequiresLocalFs,
    /// Needs a worker advertising the named capability (e.g. an MCP
    /// server id). Routed to a queue whose worker reports `id` ready.
    RequiresCapability { id: String },
}

/// One compiled step in the workflow's DAG.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompiledStep {
    /// Stable identifier: `NN_name`, derived from the file name.
    pub activity_id: String,
    /// Zero-based position in the linear sequence.
    pub index: u32,
    /// Filename relative to the workflow root (e.g. `steps/01_read.ts`).
    pub source_path: String,
    pub kind: StepKind,
    pub name: String,
    pub description: String,
    pub route: Option<String>,
    /// Activity IDs this step depends on. The initial DAG is purely
    /// linear: `steps[i]` depends on `steps[i-1]`. Builtins reshape this
    /// later.
    #[serde(default)]
    pub depends_on: Vec<String>,
    /// Free-form metadata the static parser extracted (e.g. `server` and
    /// `tool` for `mcp_tool`, the binary name for `cli`, etc.).
    #[serde(default)]
    pub metadata: serde_json::Map<String, serde_json::Value>,
    /// Where this step is allowed to run. Computed by the compiler.
    #[serde(default)]
    pub placement: Placement,
    /// Resolved task queue chosen by the CLI planner. `None` until the
    /// planner runs (Phase 4); the workflow body dispatches on this.
    #[serde(default)]
    pub task_queue: Option<String>,
}

/// A compiled, ready-to-execute workflow.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompiledWorkflow {
    pub manifest: Manifest,
    pub steps: Vec<CompiledStep>,
    /// CLI binary names referenced anywhere in the steps. Subset of
    /// `manifest.tools_required` after validation.
    pub required_cli_binaries: Vec<String>,
    /// MCP server names referenced anywhere in the steps. Subset of
    /// `manifest.mcp_servers` after validation.
    pub required_mcp_servers: Vec<String>,
    /// LLM providers (`openai` / `anthropic` / `gemini`) referenced by
    /// any `llm` step's model name. The CLI validates each one against
    /// the credentials it could resolve before any step runs.
    #[serde(default)]
    pub required_llm_providers: Vec<String>,
}

// ---------------------------------------------------------------------------
// Worker identity (Phase 3)
// ---------------------------------------------------------------------------

/// Authored identity of a Cori worker process.
///
/// A worker's identity is **fixed at launch** and determines the
/// Temporal task queue it polls (see [`task_queue_for`]). A `Person`
/// worker exposes the local user's credentials, files, and CLIs; a
/// `Service` worker exposes a shared pool's credentials usable by any
/// authorized user whose run routes to it.
///
/// There is no mechanism to mix the two in one process — mixed
/// ownership on a machine means running two worker processes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum WorkerIdentity {
    /// The current OS / SSO user. Default for `cori work`.
    Person { user_id: String },
    /// A shared service worker named `<pool>`. Requires `--shared <name>`.
    Service { pool: String },
}

/// Validation error for identity strings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdentityValidationError {
    Empty,
    InvalidChar { value: String, ch: char },
}

impl std::fmt::Display for IdentityValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => write!(f, "identity must be non-empty"),
            Self::InvalidChar { value, ch } => write!(
                f,
                "identity `{value}` contains invalid character `{ch}` \
                 (allowed: lowercase a-z, 0-9, `-`, `_`)"
            ),
        }
    }
}

impl std::error::Error for IdentityValidationError {}

/// Validate a `user_id` / `pool` string. Allowed: lowercase alnum + `-_`.
/// Queue names depend on this — non-negotiable.
pub fn validate_identity_token(s: &str) -> Result<(), IdentityValidationError> {
    if s.is_empty() {
        return Err(IdentityValidationError::Empty);
    }
    for ch in s.chars() {
        let ok = ch.is_ascii_digit() || (ch.is_ascii_lowercase()) || ch == '-' || ch == '_';
        if !ok {
            return Err(IdentityValidationError::InvalidChar {
                value: s.to_string(),
                ch,
            });
        }
    }
    Ok(())
}

impl WorkerIdentity {
    /// Construct a `Person` identity, validating `user_id`.
    pub fn person(user_id: impl Into<String>) -> Result<Self, IdentityValidationError> {
        let user_id = user_id.into();
        validate_identity_token(&user_id)?;
        Ok(Self::Person { user_id })
    }

    /// Construct a `Service` identity, validating `pool`.
    pub fn service(pool: impl Into<String>) -> Result<Self, IdentityValidationError> {
        let pool = pool.into();
        validate_identity_token(&pool)?;
        Ok(Self::Service { pool })
    }
}

/// Derive the Temporal task queue name a worker with this identity
/// must bind. **Queue names derive from authenticated identity, never
/// from user input** — Temporal's matching layer makes cross-user
/// dispatch impossible.
pub fn task_queue_for(identity: &WorkerIdentity) -> String {
    match identity {
        WorkerIdentity::Person { user_id } => format!("cori.user.{user_id}"),
        WorkerIdentity::Service { pool } => format!("cori.service.{pool}"),
    }
}

/// Inverse of [`task_queue_for`]: recover the [`WorkerIdentity`] from
/// a queue name. Returns `None` for queues that don't follow the
/// `cori.user.<id>` / `cori.service.<pool>` shape, or whose token does
/// not pass [`validate_identity_token`].
pub fn identity_from_queue(queue: &str) -> Option<WorkerIdentity> {
    if let Some(rest) = queue.strip_prefix("cori.user.") {
        WorkerIdentity::person(rest).ok()
    } else if let Some(rest) = queue.strip_prefix("cori.service.") {
        WorkerIdentity::service(rest).ok()
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_queue_for_person() {
        let id = WorkerIdentity::person("jean").unwrap();
        assert_eq!(task_queue_for(&id), "cori.user.jean");
    }

    #[test]
    fn task_queue_for_service() {
        let id = WorkerIdentity::service("billing").unwrap();
        assert_eq!(task_queue_for(&id), "cori.service.billing");
    }

    #[test]
    fn rejects_uppercase_and_special() {
        assert!(WorkerIdentity::person("Jean").is_err());
        assert!(WorkerIdentity::person("j.ean").is_err());
        assert!(WorkerIdentity::person("").is_err());
        assert!(WorkerIdentity::person("ok-name_2").is_ok());
    }

    #[test]
    fn round_trips_through_serde() {
        let id = WorkerIdentity::service("pool-a").unwrap();
        let json = serde_json::to_string(&id).unwrap();
        let back: WorkerIdentity = serde_json::from_str(&json).unwrap();
        assert_eq!(id, back);
    }
}
