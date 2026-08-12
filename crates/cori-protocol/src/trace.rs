//! Run trace types — persisted to `~/.cori/runs/<key>/<utc>.json`.
//!
//! Moved here from `cori-cli` so that `cori-run` and `cori-console`
//! can depend on these wire types without creating a dependency cycle.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use crate::{StepKind, WorkerIdentity};
use cori_manifest::{ResultFieldFormat, ResultFieldTone, ResultSectionDisplay};

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

/// A result field after its manifest path has been resolved.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ResolvedResultField {
    pub label: String,
    pub value: JsonValue,
    pub format: ResultFieldFormat,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub currency: Option<String>,
    pub tone: ResultFieldTone,
}

/// A result section after its manifest path has been resolved.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ResolvedResultSection {
    pub label: String,
    pub value: JsonValue,
    pub display: ResultSectionDisplay,
}

/// An explicit link produced by a workflow result declaration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResolvedResultArtifact {
    pub label: String,
    pub url: String,
}

/// A non-fatal problem encountered while resolving a declared result.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResultIssue {
    /// Manifest location, such as `fields[1]` or `headline`.
    pub item: String,
    #[serde(rename = "type")]
    pub kind: ResultIssueKind,
    pub message: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ResultIssueKind {
    MissingValue,
    TypeMismatch,
    NonScalarTemplate,
    InvalidUrl,
}

/// Persisted presentation resolved from the manifest and available run state.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ResolvedResult {
    pub headline: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fields: Vec<ResolvedResultField>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sections: Vec<ResolvedResultSection>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<ResolvedResultArtifact>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub issues: Vec<ResultIssue>,
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
    /// User-facing result resolved at run completion. Absent for workflows
    /// without a result declaration and for traces written by older Cori versions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<ResolvedResult>,
    pub activities: Vec<ActivityTrace>,
    pub cost: CostSummary,
    pub error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn old_trace_without_result_deserializes() {
        let trace: RunTrace = serde_json::from_value(json!({
            "run_id": "run_old",
            "workflow_id": "old",
            "status": "succeeded",
            "trigger": "cli",
            "started_at": "2026-08-04T10:00:00Z",
            "ended_at": "2026-08-04T10:00:01Z",
            "duration_ms": 1000,
            "params": {},
            "activities": [],
            "cost": { "total_eur": 0.0, "input_tokens": 0, "output_tokens": 0 },
            "error": null
        }))
        .unwrap();
        assert!(trace.result.is_none());
    }

    #[test]
    fn resolved_result_round_trips() {
        let result = ResolvedResult {
            headline: "12 rows ready".into(),
            description: Some("Report generated".into()),
            fields: vec![ResolvedResultField {
                label: "Rows".into(),
                value: json!(12),
                format: ResultFieldFormat::Number,
                currency: None,
                tone: ResultFieldTone::Success,
            }],
            sections: vec![],
            artifacts: vec![ResolvedResultArtifact {
                label: "Open".into(),
                url: "https://example.com/report".into(),
            }],
            issues: vec![],
        };
        let encoded = serde_json::to_value(&result).unwrap();
        assert_eq!(
            serde_json::from_value::<ResolvedResult>(encoded).unwrap(),
            result
        );
    }
}
