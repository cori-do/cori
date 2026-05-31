//! `GET /api/status` — endpoint + identity + capabilities + workers
//! + pinned remotes. Same data `cori status` prints.

use std::time::Duration;

use axum::{Json, extract::State};
use cori_broker::capabilities::{self, CapabilityReport};
use cori_broker::identity::{IdentitySource, OsUser};
use cori_protocol::{WorkerIdentity, task_queue_for};
use cori_run::{paths, planner, remote, resolve_llm_credentials, temporal_endpoint};
use cori_worker::runtime::preflight_check;
use serde_json::{Value, json};

use crate::{error::ApiError, state::AppState};

pub async fn handler(State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    let _ = state; // home is read implicitly via $CORI_HOME (see paths.rs)
    let body = tokio::task::spawn_blocking(collect)
        .await
        .map_err(|e| ApiError::Internal(anyhow::anyhow!("status task join: {e}")))??;
    Ok(Json(body))
}

fn collect() -> anyhow::Result<Value> {
    let endpoint = temporal_endpoint::resolve()?;
    let reachable = preflight_check(&endpoint.target, Duration::from_millis(500)).is_ok();

    let identity = OsUser.resolve()?;
    let queue = task_queue_for(&identity);

    let credentials = resolve_llm_credentials();
    let home = paths::home()?;
    let caps = capabilities::discover(&home, &[], &credentials);
    let self_report = CapabilityReport::from_capabilities_with(
        identity.clone(),
        &caps,
        Some(&paths::credentials_dir()?),
    );

    let cluster = planner::ClusterView::load().unwrap_or_default();
    let pins = remote::pins::load().unwrap_or_default();
    let trust = remote::trust::load().unwrap_or_default();

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
                "kind": kind,
                "is_self": r.task_queue == queue,
            })
        })
        .collect();

    let pinned_remotes: Vec<Value> = pins
        .entries
        .iter()
        .map(|(key, entry)| {
            let (repo_part, _) = key.split_once('@').unwrap_or((key.as_str(), ""));
            let host_repo = repo_part
                .split_once("//")
                .map(|(hr, _)| hr)
                .unwrap_or(repo_part);
            let trust_key = format!("{host_repo}@{}", entry.sha);
            let trusted = trust.entries.contains_key(&trust_key);
            json!({
                "key": key,
                "sha": entry.sha,
                "resolved_at": entry.resolved_at,
                "trusted": trusted,
            })
        })
        .collect();

    Ok(json!({
        "endpoint": endpoint.target,
        "reachable": reachable,
        "identity": identity,
        "task_queue": queue,
        "self_report": self_report,
        "workers": workers,
        "pinned_remotes": pinned_remotes,
    }))
}
