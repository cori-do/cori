//! IPC command handlers. Snake-case on the wire (per §7), so every
//! handler is annotated `rename_all = "snake_case"`.

use std::cmp::Reverse;
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, Utc};
use cori_broker::capabilities::{self, CapabilityReport};
use cori_broker::identity::{IdentitySource, OsUser};
use cori_protocol::trace::RunTrace;
use cori_protocol::{WorkerIdentity, task_queue_for};
use cori_run::{paths, planner, remote, resolve_llm_credentials};
use cori_worker::runtime::preflight_check;
use serde::Serialize;
use serde_json::{Value, json};
use tauri::State;

use crate::error::{IpcError, IpcResult};
use crate::events::StackStatus;
use crate::state::AppState;

// ---------- get_status ----------

#[tauri::command(rename_all = "snake_case")]
pub async fn get_status(state: State<'_, AppState>) -> IpcResult<Value> {
    let target = state.temporal_target.lock().ok().and_then(|g| g.clone());
    tokio::task::spawn_blocking(move || collect_status(target))
        .await
        .map_err(|e| IpcError::Internal(anyhow::anyhow!("status task join: {e}")))?
        .map_err(IpcError::Internal)
}

fn collect_status(published_target: Option<String>) -> anyhow::Result<Value> {
    // The supervisor publishes the target it owns (or the external it
    // adopted) into AppState. If it hasn't yet (very early reads),
    // surface that as unreachable rather than re-probing — re-probing
    // is what produced the race-spawn orphan in earlier iterations.
    let (target, reachable) = match published_target {
        Some(t) => {
            let ok = preflight_check(&t, Duration::from_millis(500)).is_ok();
            (t, ok)
        }
        None => (
            crate::temporal::external_target_configured()
                .unwrap_or_else(|| "http://127.0.0.1:7233".to_string()),
            false,
        ),
    };

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
        "endpoint": target,
        "reachable": reachable,
        "identity": identity,
        "task_queue": queue,
        "self_report": self_report,
        "workers": workers,
        "pinned_remotes": pinned_remotes,
    }))
}

// ---------- list_runs ----------

#[derive(Serialize)]
pub struct RunListEntry {
    pub key: String,
    pub utc: String,
    #[serde(flatten)]
    pub trace: RunTrace,
}

#[tauri::command(rename_all = "snake_case")]
pub async fn list_runs(
    workflow_id: Option<String>,
    limit: Option<usize>,
) -> IpcResult<Vec<RunListEntry>> {
    let runs_root = paths::runs_dir().map_err(IpcError::Internal)?;
    let limit = limit.unwrap_or(50);
    tokio::task::spawn_blocking(move || collect_traces(&runs_root, workflow_id, limit))
        .await
        .map_err(|e| IpcError::Internal(anyhow::anyhow!("runs task join: {e}")))?
        .map_err(IpcError::Internal)
}

fn collect_traces(
    runs_root: &Path,
    workflow_filter: Option<String>,
    limit: usize,
) -> anyhow::Result<Vec<RunListEntry>> {
    let mut out: Vec<RunListEntry> = Vec::new();
    if !runs_root.exists() {
        return Ok(out);
    }
    for hist_dir in std::fs::read_dir(runs_root)? {
        let Ok(hist_dir) = hist_dir else { continue };
        let dir = hist_dir.path();
        if !dir.is_dir() {
            continue;
        }
        let Some(key) = dir.file_name().and_then(|s| s.to_str()).map(str::to_string) else {
            continue;
        };
        let Ok(files) = std::fs::read_dir(&dir) else {
            continue;
        };
        for f in files.flatten() {
            let path: PathBuf = f.path();
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            let Some(filename) = path.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            if filename.starts_with('.') {
                continue;
            }
            let utc = filename.trim_end_matches(".json").to_string();
            let Ok(bytes) = std::fs::read(&path) else {
                continue;
            };
            let Ok(trace) = serde_json::from_slice::<RunTrace>(&bytes) else {
                continue;
            };
            if let Some(wf) = workflow_filter.as_deref()
                && trace.workflow_id != wf
            {
                continue;
            }
            out.push(RunListEntry {
                key: key.clone(),
                utc,
                trace,
            });
        }
    }
    out.sort_by_key(|e| Reverse(e.trace.started_at));
    out.truncate(limit);
    Ok(out)
}

// ---------- get_run ----------

#[tauri::command(rename_all = "snake_case")]
pub async fn get_run(key: String, filename: String) -> IpcResult<Value> {
    if !is_safe_segment(&key) || !is_safe_segment(&filename) {
        return Err(IpcError::BadRequest("invalid path component".into()));
    }
    if !filename.ends_with(".json") {
        return Err(IpcError::BadRequest("expected .json filename".into()));
    }

    let runs_root = paths::runs_dir().map_err(IpcError::Internal)?;
    let path = runs_root.join(&key).join(&filename);

    tokio::task::spawn_blocking(move || -> IpcResult<Value> {
        if !path.is_file() {
            return Err(IpcError::NotFound(format!(
                "run trace `{}/{}` not found",
                key, filename
            )));
        }
        let bytes = std::fs::read(&path).map_err(|e| IpcError::Internal(anyhow::Error::new(e)))?;
        let v: Value = serde_json::from_slice(&bytes)
            .map_err(|e| IpcError::Internal(anyhow::Error::new(e)))?;
        Ok(v)
    })
    .await
    .map_err(|e| IpcError::Internal(anyhow::anyhow!("trace task join: {e}")))?
}

fn is_safe_segment(s: &str) -> bool {
    if s.is_empty() || s == "." || s == ".." {
        return false;
    }
    if s.contains('/') || s.contains('\\') || s.contains('\0') {
        return false;
    }
    !s.chars().any(char::is_control)
}

// ---------- list_recent_workflows ----------

#[derive(Serialize)]
pub struct RecentWorkflow {
    pub key: String,
    pub workflow_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<Value>,
    pub last_run_at: DateTime<Utc>,
    pub last_status: String,
    pub run_count: usize,
}

#[tauri::command(rename_all = "snake_case")]
pub async fn list_recent_workflows() -> IpcResult<Vec<RecentWorkflow>> {
    let runs_root = paths::runs_dir().map_err(IpcError::Internal)?;
    tokio::task::spawn_blocking(move || collect_recents(&runs_root))
        .await
        .map_err(|e| IpcError::Internal(anyhow::anyhow!("recents task join: {e}")))?
        .map_err(IpcError::Internal)
}

fn collect_recents(runs_root: &Path) -> anyhow::Result<Vec<RecentWorkflow>> {
    let mut out: Vec<RecentWorkflow> = Vec::new();
    if !runs_root.exists() {
        return Ok(out);
    }
    for hist_dir in std::fs::read_dir(runs_root)? {
        let Ok(hist_dir) = hist_dir else { continue };
        let dir = hist_dir.path();
        if !dir.is_dir() {
            continue;
        }
        let key = match dir.file_name().and_then(|s| s.to_str()) {
            Some(s) => s.to_string(),
            None => continue,
        };

        let mut latest: Option<RunTrace> = None;
        let mut count = 0usize;
        let Ok(files) = std::fs::read_dir(&dir) else {
            continue;
        };
        for f in files.flatten() {
            let p = f.path();
            if p.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            if p.file_name()
                .and_then(|s| s.to_str())
                .is_some_and(|s| s.starts_with('.'))
            {
                continue;
            }
            count += 1;
            let Ok(bytes) = std::fs::read(&p) else {
                continue;
            };
            let Ok(t) = serde_json::from_slice::<RunTrace>(&bytes) else {
                continue;
            };
            match &latest {
                Some(cur) if t.started_at <= cur.started_at => {}
                _ => latest = Some(t),
            }
        }

        if let Some(t) = latest {
            out.push(RecentWorkflow {
                key,
                workflow_id: t.workflow_id,
                source: serde_json::to_value(&t.source).ok(),
                last_run_at: t.started_at,
                last_status: t.status,
                run_count: count,
            });
        }
    }
    out.sort_by_key(|e| Reverse(e.last_run_at));
    Ok(out)
}

// ---------- get_stack_status ----------

#[tauri::command(rename_all = "snake_case")]
pub async fn get_stack_status(state: State<'_, AppState>) -> IpcResult<StackStatus> {
    let snap = state
        .stack_status
        .lock()
        .map_err(|e| IpcError::Internal(anyhow::anyhow!("stack status poisoned: {e}")))?
        .clone();
    Ok(snap)
}
