//! Trigger flow: `resolve_workflow`, `start_run`, `subscribe_run`,
//! `record_trust`. Mirrors the deleted axum console's `/api/workflow`,
//! `/api/runs`, `/api/runs/:run_id/stream`, `/api/trust` endpoints but
//! through Tauri IPC.

use std::path::PathBuf;
use std::sync::Arc;

use cori_protocol::StepKind;
use cori_run::remote::trust;
use cori_run::{
    ConsentCallback, ConsentDecision, PreflightOutcome, ProgressSink, RunRequest, Trigger,
    new_run_id, preflight, run_workflow,
};
use cori_worker::workflow::ActivitySummary;
use serde::Serialize;
use serde_json::{Map, Value};
use tauri::State;
use tauri::ipc::Channel;
use tracing::warn;

use crate::error::{ConsentDetails, IpcError, IpcResult};
use crate::runs::{PlanStep, RunChannel, RunEvent};
use crate::state::AppState;

// ---------- resolve_workflow ----------

#[derive(Debug, Serialize)]
pub struct WorkflowPreflight {
    pub manifest: Value,
    pub content_hash: String,
    pub absolute_path: PathBuf,
    pub steps: Vec<StepSummary>,
    pub required_cli_binaries: Vec<String>,
    pub required_mcp_servers: Vec<String>,
    pub required_llm_providers: Vec<String>,
    pub capabilities: Value,
    pub missing_capabilities: Vec<String>,
    pub ready: bool,
    pub has_builtin_step: bool,
}

#[derive(Debug, Serialize)]
pub struct StepSummary {
    pub activity_id: String,
    pub name: String,
    pub kind: String,
    pub description: String,
    pub placement: Value,
}

#[tauri::command(rename_all = "snake_case")]
pub async fn resolve_workflow(
    source: String,
    update: Option<bool>,
) -> IpcResult<WorkflowPreflight> {
    let update = update.unwrap_or(false);
    let outcome = tokio::task::spawn_blocking(move || preflight(&source, update, false))
        .await
        .map_err(|e| IpcError::Internal(anyhow::anyhow!("preflight task join: {e}")))?
        .map_err(IpcError::Internal)?;

    if let Some(cr) = outcome.consent_required {
        return Err(IpcError::ConsentRequired(ConsentDetails {
            host: cr.spec.host,
            repo: cr.spec.repo,
            subpath: cr.spec.subpath,
            ref_str: cr.spec.ref_str,
            sha: cr.sha,
        }));
    }

    Ok(build_preflight_payload(outcome))
}

fn build_preflight_payload(outcome: PreflightOutcome) -> WorkflowPreflight {
    let compiled = &outcome.loaded.compiled;
    let manifest = serde_json::to_value(&compiled.manifest).unwrap_or(Value::Null);

    let mut has_builtin = false;
    let steps: Vec<StepSummary> = compiled
        .steps
        .iter()
        .map(|s| {
            if matches!(s.kind, StepKind::Builtin) {
                has_builtin = true;
            }
            StepSummary {
                activity_id: s.activity_id.clone(),
                name: s.name.clone(),
                kind: kind_label(&s.kind).to_string(),
                description: s.description.clone(),
                placement: serde_json::to_value(&s.placement).unwrap_or(Value::Null),
            }
        })
        .collect();

    let capabilities = serde_json::to_value(&outcome.cap_report).unwrap_or(Value::Null);
    let ready = outcome.missing_caps.is_empty();

    WorkflowPreflight {
        manifest,
        content_hash: outcome.loaded.content_hash.clone(),
        absolute_path: outcome.loaded.absolute_path.clone(),
        steps,
        required_cli_binaries: compiled.required_cli_binaries.clone(),
        required_mcp_servers: compiled.required_mcp_servers.clone(),
        required_llm_providers: compiled.required_llm_providers.clone(),
        capabilities,
        missing_capabilities: outcome.missing_caps,
        ready,
        has_builtin_step: has_builtin,
    }
}

fn kind_label(k: &StepKind) -> &'static str {
    match k {
        StepKind::Cli => "cli",
        StepKind::McpTool => "mcp_tool",
        StepKind::Code => "code",
        StepKind::Llm => "llm",
        StepKind::Builtin => "builtin",
    }
}

// ---------- record_trust ----------

#[tauri::command(rename_all = "snake_case")]
pub async fn record_trust(
    host: String,
    repo: String,
    subpath: String,
    ref_str: String,
    sha: String,
) -> IpcResult<Map<String, Value>> {
    tokio::task::spawn_blocking(move || {
        use cori_run::remote::refspec::{RemoteRef, RemoteRefKind, Transport};
        // record_consent keys on (host, repo, sha) — the rest of the
        // RemoteRef is informational only for this code path.
        let spec = RemoteRef {
            host,
            repo,
            subpath,
            ref_str,
            kind: RemoteRefKind::ExactTag,
            explicit_split: false,
            transport: Transport::Https,
        };
        trust::record_consent(&spec, &sha, Vec::new())
    })
    .await
    .map_err(|e| IpcError::Internal(anyhow::anyhow!("trust task join: {e}")))?
    .map_err(IpcError::Internal)?;
    Ok(Map::new())
}

// ---------- start_run ----------

#[derive(Debug, Serialize)]
pub struct StartRunResponse {
    pub run_id: String,
}

#[tauri::command(rename_all = "snake_case")]
pub async fn start_run(
    state: State<'_, AppState>,
    source: String,
    params: Value,
    dry_run: bool,
    update: Option<bool>,
    on_event: Channel<RunEvent>,
) -> IpcResult<StartRunResponse> {
    let update = update.unwrap_or(false);
    let run_id = new_run_id();

    // Register a per-run channel so future `subscribe_run` calls can
    // replay buffered events.
    {
        let mut map = state
            .run_channels
            .lock()
            .map_err(|e| IpcError::Internal(anyhow::anyhow!("run_channels poisoned: {e}")))?;
        map.insert(run_id.clone(), RunChannel::new());
    }

    let sink_concrete = Arc::new(ChannelProgressSink {
        run_id: run_id.clone(),
        forward: on_event,
        channels: state.run_channels.clone(),
    });

    // Spawn the run on a dedicated thread (run_workflow drives a
    // !Send Temporal worker handle internally — same constraint as
    // serve_worker_until_cancelled).
    let run_id_for_thread = run_id.clone();
    let sink_for_thread = Arc::clone(&sink_concrete);
    std::thread::Builder::new()
        .name(format!("cori-run-{}", &run_id[..run_id.len().min(8)]))
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    warn!(error = %e, "could not build run thread runtime");
                    return;
                }
            };

            let sink_dyn: Arc<dyn ProgressSink> = sink_for_thread.clone();
            let result = rt.block_on(run_workflow(
                RunRequest {
                    source,
                    params,
                    dry_run,
                    update,
                    trigger: Trigger::Console,
                    run_id: Some(run_id_for_thread.clone()),
                },
                ConsentCallback::Prompt(Box::new(|_p| ConsentDecision::Defer)),
                sink_dyn,
            ));

            match result {
                Ok(trace) => {
                    sink_for_thread.push_event(RunEvent::Completed {
                        trace: Box::new(trace),
                    });
                }
                Err(e) => {
                    sink_for_thread.push_event(RunEvent::Failed {
                        error: format!("{e:#}"),
                    });
                }
            }
        })
        .map_err(|e| IpcError::Internal(anyhow::anyhow!("spawning run thread: {e}")))?;

    Ok(StartRunResponse { run_id })
}

// ---------- subscribe_run ----------

#[tauri::command(rename_all = "snake_case")]
pub async fn subscribe_run(
    state: State<'_, AppState>,
    run_id: String,
    on_event: Channel<RunEvent>,
) -> IpcResult<Map<String, Value>> {
    let (replay, mut rx) = {
        let map = state
            .run_channels
            .lock()
            .map_err(|e| IpcError::Internal(anyhow::anyhow!("run_channels poisoned: {e}")))?;
        let Some(rc) = map.get(&run_id) else {
            return Err(IpcError::NotFound(format!(
                "no live run with id `{run_id}`"
            )));
        };
        (rc.replay.clone(), rc.tx.subscribe())
    };

    // Replay buffered events first.
    for ev in replay {
        if on_event.send(ev).is_err() {
            return Ok(Map::new());
        }
    }

    // Stream live events until the broadcast lags or closes.
    tauri::async_runtime::spawn(async move {
        while let Ok(ev) = rx.recv().await {
            let terminated = matches!(ev, RunEvent::Completed { .. } | RunEvent::Failed { .. });
            if on_event.send(ev).is_err() {
                break;
            }
            if terminated {
                break;
            }
        }
    });

    Ok(Map::new())
}

// ---------- ProgressSink that pushes into a Channel + the replay buffer ----------

struct ChannelProgressSink {
    run_id: String,
    forward: Channel<RunEvent>,
    channels: crate::state::RunChannelMap,
}

impl ChannelProgressSink {
    fn push_event(&self, ev: RunEvent) {
        // Forward to the caller's Channel.
        let _ = self.forward.send(ev.clone());
        // Update the replay buffer + broadcast to subscribers.
        if let Ok(mut map) = self.channels.lock()
            && let Some(rc) = map.get_mut(&self.run_id)
        {
            rc.push(ev);
        }
    }
}

impl ProgressSink for ChannelProgressSink {
    fn on_plan(&self, plan: &[cori_run::planner::StepAssignment]) {
        let assignments = plan
            .iter()
            .map(|a| PlanStep {
                activity_id: a.activity_id.clone(),
                step_name: a.step_name.clone(),
                kind: "".to_string(), // StepAssignment doesn't carry kind; clients use placement
                task_queue: Some(a.task_queue.clone()),
            })
            .collect();
        self.push_event(RunEvent::Plan { assignments });
    }

    fn on_step_start(&self, s: &ActivitySummary) {
        self.push_event(RunEvent::StepStart {
            activity_id: s.activity_id.clone(),
            step_name: s.step_name.clone(),
            kind: kind_label_step(&s.kind).to_string(),
            task_queue: s.route.clone(),
        });
    }

    fn on_step_finish(&self, s: &ActivitySummary) {
        self.push_event(RunEvent::StepFinish {
            activity_id: s.activity_id.clone(),
            step_name: s.step_name.clone(),
            status: s.status.clone(),
            duration_ms: s.duration_ms,
            error: s.error.clone(),
        });
    }
}

fn kind_label_step(k: &StepKind) -> &'static str {
    match k {
        StepKind::Cli => "cli",
        StepKind::McpTool => "mcp_tool",
        StepKind::Code => "code",
        StepKind::Llm => "llm",
        StepKind::Builtin => "builtin",
    }
}
