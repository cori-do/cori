// SPDX-License-Identifier: Apache-2.0
//! Replays recorded histories of the template's workflows (tests/histories/, from the e2e runs) with the current
//! `CoriWorkflow`. A change that gives the same history a different command sequence would break the runs in flight
//! when a worker is upgraded (a human approval can wait for days): it fails here. Such a change is gated on data the run
//! carries (a field only newly compiled DAGs have) or on a patch.
//!
//! To add a history: `temporal workflow show -w <workflow id> -o json > tests/histories/<name>.json` (a run without real
//! credentials in its input: dev auth).
use cori_engine::workflow::CoriWorkflow;
use temporalio_client::WorkflowHistory;
use temporalio_sdk::workflow_replayer::{WorkflowReplayer, WorkflowReplayerOptions};

#[tokio::test]
async fn recorded_histories_replay_with_the_current_workflow() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/histories");
    let mut names = Vec::new();
    let mut histories = Vec::new();
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") { continue; }
        histories.push(WorkflowHistory::from_json(&std::fs::read(&path).unwrap()).unwrap_or_else(|e| panic!("{}: {e}", path.display())));
        names.push(path.file_name().unwrap().to_string_lossy().to_string());
    }
    assert!(histories.len() >= 8, "the recorded histories are missing from {}", dir.display());
    let options = WorkflowReplayerOptions::new().register_workflow::<CoriWorkflow>().unwrap().build();
    let results = WorkflowReplayer::new(options).unwrap().replay_workflows(histories).await.unwrap();
    let failures: Vec<String> = names.iter().zip(&results).filter_map(|(n, r)| r.replay_failure.as_ref().map(|f| format!("{n}: {f}"))).collect();
    assert!(failures.is_empty(), "histories that no longer replay:\n{}", failures.join("\n"));
}
