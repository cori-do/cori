//! Integration smoke test for the Temporal-backed worker.
//!
//! Gated behind `#[ignore]` because it requires a running Temporal dev
//! server. Run manually with:
//!
//! ```bash
//! temporal server start-dev --port 7233 &
//! cargo test -p cori-worker --test temporal_smoke -- --ignored --nocapture
//! ```
//!
//! Override the target with `CORI_TEMPORAL_TARGET=http://host:port`.
//!
//! The DAG used here contains a single `builtin` step, which the
//! workflow short-circuits to `"skipped"` without invoking any
//! activity. That lets the test exercise connect → worker bootstrap →
//! start_workflow → get_result without requiring Deno or LLM credentials.

use chrono::NaiveDate;
use cori_manifest::Manifest;
use cori_protocol::{CompiledStep, CompiledWorkflow, Placement, StepKind};
use cori_worker::runner::run_workflow_once;
use cori_worker::runtime::{CoriTemporalRuntime, DEFAULT_NAMESPACE};
use cori_worker::workflow::WorkflowInput;
use serde_json::{Map as JsonMap, Value as JsonValue, json};

fn temporal_target() -> String {
    std::env::var("CORI_TEMPORAL_TARGET").unwrap_or_else(|_| "http://localhost:7233".to_string())
}

#[ignore]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn runs_a_trivial_builtin_workflow() {
    let rt = CoriTemporalRuntime::connect(temporal_target(), DEFAULT_NAMESPACE, "cori.test.smoke")
        .await
        .expect("connect to local Temporal dev server");

    let mut metadata = JsonMap::new();
    metadata.insert("builtin".to_string(), json!("noop"));
    let dag = CompiledWorkflow {
        manifest: Manifest {
            id: "smoke".to_string(),
            name: "smoke".to_string(),
            description: String::new(),
            created: NaiveDate::from_ymd_opt(2025, 1, 1).unwrap(),
            version: 1,
            updated: None,
            parameters: vec![],
            tools_required: vec![],
            mcp_servers: vec![],
            tags: vec![],
            route_default: None,
            schedule: None,
            schedule_tz: None,
            body: String::new(),
        },
        steps: vec![CompiledStep {
            activity_id: "step-1".to_string(),
            index: 0,
            source_path: String::new(),
            kind: StepKind::Builtin,
            name: "noop".to_string(),
            description: String::new(),
            route: None,
            depends_on: vec![],
            metadata,
            placement: Placement::Anywhere,
            task_queue: Some("cori.test.smoke".to_string()),
        }],
        required_cli_binaries: vec![],
        required_mcp_servers: vec![],
        required_llm_providers: vec![],
    };

    let input = WorkflowInput {
        workflow_id: "smoke".to_string(),
        workflow_content_hash: None,
        user_id: "smoke".to_string(),
        compiled_dag: dag,
        user_params: JsonValue::Object(JsonMap::new()),
        dry_run: false,
        reauth_timeout_secs: None,
    };

    let out = run_workflow_once(&rt, "cori-smoke".to_string(), input)
        .await
        .expect("workflow completes");
    assert_eq!(out.activities.len(), 1);
    assert_eq!(out.activities[0].status, "skipped");
}
