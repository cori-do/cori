//! Wire types shared between the CLI, worker, and Deno runner.
//!
//! [`CompiledWorkflow`] is the canonical representation a validated runbook
//! compiles to. The CLI stores it, the worker executes it, and the trace
//! recorder references its `activity_id` values.

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

/// One compiled step in the workflow's DAG.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompiledStep {
    /// Stable identifier: `NN_name`, derived from the file name.
    pub activity_id: String,
    /// Zero-based position in the linear sequence.
    pub index: u32,
    /// Filename relative to the runbook root (e.g. `steps/01_read.ts`).
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
