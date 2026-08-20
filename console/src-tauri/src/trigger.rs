//! Trigger flow: `resolve_workflow`, `start_run`, `subscribe_run`,
//! `record_trust`. Mirrors the deleted axum console's `/api/workflow`,
//! `/api/runs`, `/api/runs/:run_id/stream`, `/api/trust` endpoints but
//! through Tauri IPC.

use std::path::PathBuf;
use std::sync::Arc;

use cori_protocol::{Placement, StepKind};
use cori_run::remote::trust;
use cori_run::{
    ConsentCallback, ConsentDecision, PreflightOutcome, ProgressSink, RunRequest, Trigger,
    new_run_id, preflight, run_workflow,
};
use cori_worker::workflow::ActivitySummary;
use serde::Serialize;
use serde_json::{Map, Value, json};
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
    pub history_key: String,
    pub absolute_path: PathBuf,
    pub steps: Vec<StepSummary>,
    pub required_cli_binaries: Vec<String>,
    pub required_mcp_servers: Vec<String>,
    pub required_llm_providers: Vec<String>,
    pub capabilities: Value,
    pub missing_capabilities: Vec<String>,
    /// Capabilities a step's placement requires but the manifest never
    /// declared — the third display state (`not declared`), computed
    /// from the same compiled steps `check` sees.
    pub undeclared_capabilities: Vec<String>,
    /// Per-step compiled effect surface (`cori_compiler::effects`) —
    /// what each step touches, derived, never agent-declared.
    pub effects: Value,
    pub ready: bool,
    /// True when a step uses a builtin the runtime still defers
    /// (`map` / `parallel`). Executable control flow (`branch`,
    /// `switch`, `for_each`, `loop`, `wait`) does not set this.
    pub has_builtin_step: bool,
    /// True when this payload is a best-effort draft parse of a folder an
    /// agent is mid-writing (full compile failed). Display-only: never
    /// runnable, capability and effect data absent.
    pub draft: bool,
}

#[derive(Debug, Serialize)]
pub struct StepSummary {
    pub activity_id: String,
    pub name: String,
    pub kind: String,
    pub description: String,
    pub placement: Value,
    /// Step source file, relative to the workflow root.
    pub source_path: String,
    /// The step's TypeScript source, read back from the resolved folder —
    /// what the inspector shows as the command / code / prompt actually
    /// executed. `None` when the file cannot be read (or is implausibly
    /// large for a step file).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// `cli` steps only: the frozen binary name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub binary: Option<String>,
    /// `mcp_tool` steps only: the frozen server / tool pair.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    /// What an `llm` step declared. The compiler always normalizes omission
    /// to `medium`. Absent for every other kind.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level: Option<String>,
    /// Which backend would serve this `llm` step under the current
    /// settings. Resolved through the same code path the runtime uses
    /// (`cori_broker::llm::resolve::preview`), so the tooltip cannot
    /// promise a provider the run won't actually use. `None` when the
    /// step isn't an `llm` step, or when nothing is ready to serve it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub llm_resolution: Option<crate::llm_cmd::LlmResolutionInfo>,
    /// Builtin sub-kind (`branch` / `switch` / `for_each` / `loop` /
    /// `wait` / `map` / `parallel`). Absent for every other kind.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub builtin: Option<String>,
    /// Control-flow details for builtin steps: nested slot names and
    /// kinds, case labels, wait spec, iteration bounds. Absent
    /// otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub builtin_detail: Option<Value>,
}

#[tauri::command(rename_all = "snake_case")]
pub async fn resolve_workflow(
    source: String,
    update: Option<bool>,
) -> IpcResult<WorkflowPreflight> {
    let update = update.unwrap_or(false);
    let error_source = source.clone();
    let blocking_source = source.clone();
    let result = tokio::task::spawn_blocking(move || preflight(&blocking_source, update, false))
        .await
        .map_err(|e| IpcError::Internal(anyhow::anyhow!("preflight task join: {e}")))?;

    let outcome = match result {
        Ok(outcome) => outcome,
        Err(error) => {
            // A folder an agent is mid-writing rarely compiles (steps
            // arrive one at a time, routes point at steps not written
            // yet). Fall back to a best-effort draft parse so the canvas
            // grows live instead of sitting on the last good plan.
            let draft = tokio::task::spawn_blocking(move || draft_preflight_payload(&source))
                .await
                .map_err(|e| IpcError::Internal(anyhow::anyhow!("draft task join: {e}")))?;
            if let Some(pf) = draft {
                return Ok(pf);
            }
            return Err(classify_preflight_error(error, error_source));
        }
    };

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

/// Draft fallback for `resolve_workflow`: only for a local folder that an
/// authoring session is actively writing into. Anything else keeps the
/// real resolve error — a broken workflow with no agent behind it is a
/// failure, not a draft.
fn draft_preflight_payload(source: &str) -> Option<WorkflowPreflight> {
    let abs = std::fs::canonicalize(source).ok()?;
    if !abs.is_dir() {
        return None;
    }
    let writing = cori_run::sessions::list().ok()?.into_iter().any(|s| {
        s.state == cori_run::sessions::SessionState::Writing
            && std::fs::canonicalize(&s.workflow_dir)
                .map(|d| d == abs)
                .unwrap_or(s.workflow_dir == abs)
    });
    if !writing {
        return None;
    }

    let draft = cori_compiler::draft(&abs);
    let folder_name = abs
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "workflow".to_string());
    let manifest = match &draft.manifest {
        Some(m) => serde_json::to_value(m).unwrap_or(Value::Null),
        // The frontend needs the summary's shape even before manifest.md
        // parses; the folder name is the only identity there is yet.
        None => json!({
            "id": folder_name,
            "name": folder_name,
            "description": "",
            "parameters": [],
            "tools_required": [],
            "mcp_servers": [],
            "body": "",
        }),
    };
    let (steps, has_builtin) = step_summaries(&draft.steps, &abs);
    let (tools, servers) = draft
        .manifest
        .as_ref()
        .map(|m| (m.tools_required.clone(), m.mcp_servers.clone()))
        .unwrap_or_default();

    Some(WorkflowPreflight {
        manifest,
        content_hash: cori_compiler::workflow_content_hash(&abs).unwrap_or_default(),
        history_key: cori_run::workflow_loader::run_history_key(&abs, &folder_name),
        absolute_path: abs,
        steps,
        required_cli_binaries: tools,
        required_mcp_servers: servers,
        required_llm_providers: Vec::new(),
        capabilities: Value::Null,
        missing_capabilities: Vec::new(),
        undeclared_capabilities: Vec::new(),
        effects: Value::Null,
        ready: false,
        has_builtin_step: has_builtin,
        draft: true,
    })
}

/// Keep expected, recoverable workflow failures out of the Console's generic
/// error renderer. The UI deliberately does not need to know about anyhow's
/// context chain or broker validation internals.
fn classify_preflight_error(error: anyhow::Error, source: String) -> IpcError {
    let resolving_workflow_path = error
        .chain()
        .any(|cause| cause.to_string().starts_with("resolving workflow path `"));
    let path_not_found = error.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|io| io.kind() == std::io::ErrorKind::NotFound)
    });
    if resolving_workflow_path && path_not_found {
        return IpcError::WorkflowMissing(source);
    }

    let invalid_typescript = error.chain().any(|cause| {
        matches!(
            cause.downcast_ref::<cori_broker::runtime::StepValidationError>(),
            Some(
                cori_broker::runtime::StepValidationError::Failed { .. }
                    | cori_broker::runtime::StepValidationError::Graph(_)
                    | cori_broker::runtime::StepValidationError::GraphEscape { .. }
            )
        )
    });
    let invalid_source = error.chain().any(|cause| {
        let message = cause.to_string();
        message.starts_with("compile errors:")
            || message.starts_with("no `manifest.md` in `")
            || message.ends_with("is not a directory")
    });
    if invalid_typescript || invalid_source {
        return IpcError::WorkflowInvalid(source);
    }

    IpcError::Internal(error)
}

/// Map compiled (or draft-parsed) steps to their display summaries.
/// Returns the summaries plus whether any step uses a builtin the
/// runtime still defers.
fn step_summaries(
    compiled_steps: &[cori_protocol::CompiledStep],
    workflow_root: &std::path::Path,
) -> (Vec<StepSummary>, bool) {
    let mut has_builtin = false;
    // Built once per workflow, not once per step: resolving credentials
    // reads the OS keychain.
    let llm_preview = compiled_steps
        .iter()
        .any(|s| matches!(s.kind, StepKind::Llm))
        .then(crate::llm_cmd::PreviewContext::new);
    let steps: Vec<StepSummary> = compiled_steps
        .iter()
        .map(|s| {
            let builtin = s
                .metadata
                .get("builtin")
                .and_then(Value::as_str)
                .map(str::to_string);
            if matches!(s.kind, StepKind::Builtin) && is_deferred_builtin(builtin.as_deref()) {
                has_builtin = true;
            }
            let builtin_detail = builtin.as_deref().and_then(|_| builtin_detail(s));
            let level = s
                .metadata
                .get("level")
                .and_then(Value::as_str)
                .map(str::to_string);
            let llm_resolution = matches!(s.kind, StepKind::Llm)
                .then(|| {
                    llm_preview.as_ref().and_then(|ctx| {
                        level
                            .as_deref()
                            .and_then(cori_broker::llm::LlmLevel::parse)
                            .and_then(|level| ctx.for_level(level))
                    })
                })
                .flatten();
            let meta_string = |key: &str| {
                s.metadata
                    .get(key)
                    .and_then(Value::as_str)
                    .map(str::to_string)
            };
            StepSummary {
                activity_id: s.activity_id.clone(),
                name: s.name.clone(),
                kind: kind_label(&s.kind).to_string(),
                description: s.description.clone(),
                placement: serde_json::to_value(&s.placement).unwrap_or(Value::Null),
                source_path: s.source_path.clone(),
                source: read_step_source(workflow_root, &s.source_path),
                binary: meta_string("binary"),
                server: meta_string("server"),
                tool: meta_string("tool"),
                level,
                llm_resolution,
                builtin,
                builtin_detail,
            }
        })
        .collect();
    (steps, has_builtin)
}

fn build_preflight_payload(outcome: PreflightOutcome) -> WorkflowPreflight {
    let compiled = &outcome.loaded.compiled;
    let manifest = serde_json::to_value(&compiled.manifest).unwrap_or(Value::Null);
    let history_key = cori_run::workflow_loader::loaded_run_history_key(&outcome.loaded);

    let (steps, has_builtin) = step_summaries(&compiled.steps, &outcome.loaded.absolute_path);

    let capabilities = serde_json::to_value(&outcome.cap_report).unwrap_or(Value::Null);
    let ready = outcome.missing_caps.is_empty();

    let declared: std::collections::BTreeSet<&str> = compiled
        .manifest
        .tools_required
        .iter()
        .chain(compiled.manifest.mcp_servers.iter())
        .map(String::as_str)
        .collect();
    let undeclared_capabilities: Vec<String> = compiled
        .steps
        .iter()
        .filter_map(|s| match &s.placement {
            Placement::RequiresCapability { id } if !declared.contains(id.as_str()) => {
                Some(id.clone())
            }
            _ => None,
        })
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();

    WorkflowPreflight {
        manifest,
        content_hash: outcome.loaded.content_hash.clone(),
        history_key,
        absolute_path: outcome.loaded.absolute_path.clone(),
        steps,
        required_cli_binaries: compiled.required_cli_binaries.clone(),
        required_mcp_servers: compiled.required_mcp_servers.clone(),
        required_llm_providers: compiled.required_llm_providers.clone(),
        capabilities,
        missing_capabilities: outcome.missing_caps,
        undeclared_capabilities,
        effects: serde_json::to_value(cori_compiler::effects::compute_effects(compiled))
            .unwrap_or(Value::Null),
        ready,
        has_builtin_step: has_builtin,
        draft: false,
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

/// A step source file is a page of TypeScript; anything past this is not
/// one, and the inspector would choke rendering it.
const MAX_STEP_SOURCE_BYTES: u64 = 256 * 1024;

fn read_step_source(workflow_root: &std::path::Path, source_path: &str) -> Option<String> {
    let path = workflow_root.join(source_path);
    let len = std::fs::metadata(&path).ok()?.len();
    if len > MAX_STEP_SOURCE_BYTES {
        return None;
    }
    std::fs::read_to_string(&path).ok()
}

/// `map` / `parallel` (or an unknown/legacy sub-kind) are still deferred
/// at runtime; the executable five are not.
fn is_deferred_builtin(sub_kind: Option<&str>) -> bool {
    !matches!(
        sub_kind,
        Some("branch") | Some("switch") | Some("for_each") | Some("loop") | Some("wait")
    )
}

/// Project a builtin step's compiled control-flow metadata for display:
/// nested slots with their kinds, plus the wait spec / iteration bounds.
fn builtin_detail(step: &cori_protocol::CompiledStep) -> Option<Value> {
    let mut detail = serde_json::Map::new();
    if let Some(nested) = step.metadata.get("nested").and_then(Value::as_object) {
        let slots: serde_json::Map<String, Value> = nested
            .iter()
            .map(|(slot, meta)| {
                let mut out = serde_json::Map::new();
                // A routing slot carries its resolved target; an inline
                // slot carries its nested step kind.
                if let Some(target) = meta.get("goto").and_then(Value::as_str) {
                    out.insert("goto".to_string(), Value::String(target.to_string()));
                    if let Some(name) = meta.get("goto_name").and_then(Value::as_str) {
                        out.insert("goto_name".to_string(), Value::String(name.to_string()));
                    }
                } else {
                    let kind = meta
                        .get("kind")
                        .and_then(Value::as_str)
                        .unwrap_or("code")
                        .to_string();
                    out.insert("kind".to_string(), Value::String(kind));
                }
                (slot.clone(), Value::Object(out))
            })
            .collect();
        detail.insert("nested".to_string(), Value::Object(slots));
    }
    for key in ["wait", "max_items", "max_iterations"] {
        if let Some(value) = step.metadata.get(key) {
            detail.insert(key.to_string(), value.clone());
        }
    }
    if detail.is_empty() {
        None
    } else {
        Some(Value::Object(detail))
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
            notes: s.notes.clone(),
            cost_eur: s.cost_eur,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_workflow_path_gets_a_recoverable_error_code() {
        let error = anyhow::Error::new(std::io::Error::from(std::io::ErrorKind::NotFound))
            .context("resolving workflow path `/missing/example`");

        assert!(matches!(
            classify_preflight_error(error, "/missing/example".into()),
            IpcError::WorkflowMissing(source) if source == "/missing/example"
        ));
    }

    #[test]
    fn invalid_module_graph_gets_a_remake_error_code() {
        let error = anyhow::Error::new(cori_broker::runtime::StepValidationError::Graph(
            "unsupported specifier `vitest`".into(),
        ))
        .context("validating workflow TypeScript modules");

        assert!(matches!(
            classify_preflight_error(error, "/workflows/example".into()),
            IpcError::WorkflowInvalid(source) if source == "/workflows/example"
        ));
    }
}
