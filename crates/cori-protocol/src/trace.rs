//! Run trace types — persisted to `~/.cori/runs/<key>/<utc>.json`.
//!
//! Moved here from `cori-cli` so that `cori-run` and `cori-console`
//! can depend on these wire types without creating a dependency cycle.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use crate::{StepKind, WorkerIdentity};

// ---------------------------------------------------------------------------
// Token accounting
// ---------------------------------------------------------------------------

/// Per-activity token accounting for LLM calls.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

impl std::ops::Add for TokenUsage {
    type Output = TokenUsage;
    fn add(self, rhs: TokenUsage) -> TokenUsage {
        TokenUsage {
            input_tokens: self.input_tokens + rhs.input_tokens,
            output_tokens: self.output_tokens + rhs.output_tokens,
        }
    }
}

// ---------------------------------------------------------------------------
// Workflow source
// ---------------------------------------------------------------------------

/// Origin of a workflow execution — recorded in the run trace.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorkflowSource {
    Local {
        path: String,
    },
    Remote {
        host: String,
        repo: String,
        subpath: String,
        #[serde(rename = "ref")]
        ref_str: String,
        sha: String,
    },
}

// ---------------------------------------------------------------------------
// Trace types
// ---------------------------------------------------------------------------

/// Aggregate cost for a full run.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CostSummary {
    pub total_eur: f64,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

/// One activity's trace entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivityTrace {
    pub activity_id: String,
    pub step_name: String,
    pub kind: StepKind,
    pub status: String,
    pub started_at: DateTime<Utc>,
    pub ended_at: DateTime<Utc>,
    pub duration_ms: u128,
    pub attempts: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route: Option<String>,
    /// Task queue the activity was dispatched to. `None` for legacy traces.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_queue: Option<String>,
    /// Worker identity derived from `task_queue`. `None` for legacy traces.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_identity: Option<WorkerIdentity>,
    pub input_summary: JsonValue,
    pub output_summary: JsonValue,
    pub output: JsonValue,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_eur: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens: Option<TokenUsage>,
    pub error: Option<String>,
    pub notes: Option<String>,
}

/// Full run trace — persisted to `~/.cori/runs/<key>/<utc>.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunTrace {
    pub run_id: String,
    pub workflow_id: String,
    /// 16-hex-char hash of the workflow folder contents at run time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_content_hash: Option<String>,
    pub status: String,
    pub trigger: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub dry_run: bool,
    /// Identity of the user who started this run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requesting_identity: Option<WorkerIdentity>,
    pub started_at: DateTime<Utc>,
    pub ended_at: DateTime<Utc>,
    pub duration_ms: u128,
    /// Origin of the workflow.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<WorkflowSource>,
    pub params: JsonValue,
    pub activities: Vec<ActivityTrace>,
    pub cost: CostSummary,
    pub error: Option<String>,
}
