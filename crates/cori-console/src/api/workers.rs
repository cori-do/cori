//! `GET /api/workers` — every worker visible on this machine's cluster
//! view (the union of `~/.cori/cluster/*.json` reports plus this
//! process's own report, if running under `cori work`).
//!
//! Source: `cori_run::planner::ClusterView::load`. Phase 4 reads
//! cached cluster reports only; a future phase can layer a
//! `DescribeTaskQueue` poll on top (cached ≥ 5 s) for "is currently
//! polling" liveness — kept out of v1 to honour the "human-frequency
//! Temporal calls only" rule.

use axum::Json;
use cori_broker::identity::{IdentitySource, OsUser};
use cori_protocol::{WorkerIdentity, task_queue_for};
use cori_run::planner;
use serde_json::{Value, json};

use crate::error::ApiError;

pub async fn handler() -> Result<Json<Value>, ApiError> {
    let body = tokio::task::spawn_blocking(collect)
        .await
        .map_err(|e| ApiError::Internal(anyhow::anyhow!("workers join: {e}")))??;
    Ok(Json(body))
}

fn collect() -> anyhow::Result<Value> {
    let identity = OsUser.resolve()?;
    let my_queue = task_queue_for(&identity);
    let cluster = planner::ClusterView::load().unwrap_or_default();

    let workers: Vec<Value> = cluster
        .reports
        .iter()
        .map(|r| {
            let kind = match &r.identity {
                WorkerIdentity::Person { .. } => "user",
                WorkerIdentity::Service { .. } => "shared",
            };
            json!({
                "task_queue": r.task_queue,
                "identity": r.identity,
                "kind": kind,
                "is_self": r.task_queue == my_queue,
                "capabilities": r.capabilities,
            })
        })
        .collect();

    Ok(json!({
        "this_queue": my_queue,
        "workers": workers,
    }))
}
