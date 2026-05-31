//! `POST /api/runs` — start a workflow run.
//!
//! Body: `{ source, params, dry_run, update? }`.
//! Returns `200 { run_id, stream_url }` on success, or
//! `409 { consent_required: {...} }` when a remote ref needs first-run
//! consent (the client posts `/api/trust` and retries).

use std::sync::Arc;

use axum::{
    Json,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use cori_run::{ConsentCallback, RunRequest, Trigger, new_run_id, preflight, run_workflow};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    error::ApiError,
    runs::{RunChannel, RunEvent},
    sink::ConsoleProgressSink,
    state::AppState,
};

#[derive(Deserialize)]
pub struct TriggerBody {
    pub source: String,
    #[serde(default)]
    pub params: Value,
    #[serde(default)]
    pub dry_run: bool,
    #[serde(default)]
    pub update: Option<bool>,
}

#[derive(Serialize)]
pub struct TriggerResponse {
    pub run_id: String,
    pub stream_url: String,
}

pub async fn handler(
    State(state): State<AppState>,
    Json(body): Json<TriggerBody>,
) -> Result<Response, ApiError> {
    let source = body.source.clone();
    let update = body.update.unwrap_or(false);

    // 1. Preflight first — fast (no Temporal contact) and lets us
    // return 409 cleanly before we mint a run_id / open a channel.
    let pre_source = source.clone();
    let outcome = tokio::task::spawn_blocking(move || preflight(&pre_source, update, false))
        .await
        .map_err(|e| ApiError::Internal(anyhow::anyhow!("preflight join: {e}")))??;

    if let Some(cr) = outcome.consent_required {
        let declared =
            cori_run::remote::trust::declared_capability_strings(&outcome.loaded.compiled);
        let body = json!({
            "consent_required": {
                "host": cr.spec.host,
                "repo": cr.spec.repo,
                "subpath": cr.spec.subpath,
                "ref": cr.spec.ref_str,
                "sha": cr.sha,
                "declared_capabilities": declared,
            }
        });
        return Ok((StatusCode::CONFLICT, Json(body)).into_response());
    }

    // 2. Mint run_id, create channel, register.
    let run_id = new_run_id();
    let channel = RunChannel::new(64);
    state
        .runs
        .write()
        .await
        .insert(run_id.clone(), channel.clone());

    // 3. Spawn the run on a dedicated OS thread with its own
    // current-thread tokio runtime. The `run_workflow` future holds
    // non-`Send` state inside the Temporal SDK so we cannot share it
    // across the axum worker pool. Same pattern the CLI uses
    // (a `block_on` from the main thread); we just give every
    // Console-triggered run its own thread + runtime.
    let run_id_for_task = run_id.clone();
    let channel_for_task = channel.clone();
    std::thread::Builder::new()
        .name(format!("cori-run-{run_id}"))
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    channel_for_task.push(RunEvent::Failed {
                        error: format!("failed to start tokio runtime for run: {e}"),
                    });
                    return;
                }
            };
            let sink: Arc<dyn cori_run::ProgressSink> = Arc::new(ConsoleProgressSink {
                channel: channel_for_task.clone(),
            });
            let req = RunRequest {
                source: body.source,
                params: body.params,
                dry_run: body.dry_run,
                update,
                trigger: Trigger::Console,
                run_id: Some(run_id_for_task),
            };
            let result = rt.block_on(run_workflow(req, ConsentCallback::AssumeYes, sink));
            match result {
                Ok(trace) => channel_for_task.push(RunEvent::Completed {
                    trace: Box::new(trace),
                }),
                Err(e) => channel_for_task.push(RunEvent::Failed {
                    error: format!("{e:#}"),
                }),
            }
        })
        .map_err(|e| ApiError::Internal(anyhow::anyhow!("spawning run thread: {e}")))?;

    Ok((
        StatusCode::OK,
        Json(TriggerResponse {
            stream_url: format!("/api/runs/{run_id}/stream"),
            run_id,
        }),
    )
        .into_response())
}
