//! `GET /api/runs` — list traces (optionally filtered by workflow_id).
//! `GET /api/runs/:key/:filename` — one full trace JSON.

use std::cmp::Reverse;
use std::path::{Path, PathBuf};

use axum::{
    Json,
    extract::{Query, State},
    response::IntoResponse,
};
use cori_protocol::RunTrace;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{error::ApiError, state::AppState};

#[derive(Deserialize, Default)]
pub struct ListQuery {
    pub workflow_id: Option<String>,
    pub limit: Option<usize>,
}

/// One row in the runs list. Carries the on-disk coordinates so the
/// SPA can deep-link to `/runs/:key/:utc` — those aren't recoverable
/// from `RunTrace` alone (the run-history directory key depends on
/// the absolute source path hash, not the workflow id).
#[derive(Serialize)]
pub struct RunListEntry {
    /// Run-history directory under `~/.cori/runs/`. URL-safe.
    pub key: String,
    /// Trace filename without the `.json` extension. URL-safe.
    pub utc: String,
    /// Full trace payload. Inlined so the list view doesn't need a
    /// second round-trip for the columns it shows.
    #[serde(flatten)]
    pub trace: RunTrace,
}

pub async fn list(
    State(state): State<AppState>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Vec<RunListEntry>>, ApiError> {
    let runs_root = state.home.join("runs");
    let limit = q.limit.unwrap_or(50);
    let workflow_id = q.workflow_id.clone();
    let traces =
        tokio::task::spawn_blocking(move || collect_traces(&runs_root, workflow_id, limit))
            .await
            .map_err(|e| ApiError::Internal(anyhow::anyhow!("runs task join: {e}")))??;
    Ok(Json(traces))
}

pub async fn trace(
    State(state): State<AppState>,
    axum::extract::Path((key, filename)): axum::extract::Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    if !is_safe_segment(&key) || !is_safe_segment(&filename) {
        return Err(ApiError::BadRequest("invalid path component".into()));
    }
    if !filename.ends_with(".json") {
        return Err(ApiError::BadRequest("expected .json filename".into()));
    }

    let runs_root = state.home.join("runs");
    let path = runs_root.join(&key).join(&filename);

    let body = tokio::task::spawn_blocking(move || -> Result<Value, ApiError> {
        if !path.is_file() {
            return Err(ApiError::NotFound(format!(
                "run trace `{}/{}` not found",
                key, filename
            )));
        }
        let bytes = std::fs::read(&path).map_err(|e| ApiError::Internal(anyhow::Error::new(e)))?;
        let v: Value = serde_json::from_slice(&bytes)
            .map_err(|e| ApiError::Internal(anyhow::Error::new(e)))?;
        Ok(v)
    })
    .await
    .map_err(|e| ApiError::Internal(anyhow::anyhow!("trace task join: {e}")))??;

    Ok(Json(body))
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

/// Reject anything that could escape a single directory level: empty,
/// path separators, `.`/`..`, NUL, control chars.
pub fn is_safe_segment(s: &str) -> bool {
    if s.is_empty() || s == "." || s == ".." {
        return false;
    }
    if s.contains('/') || s.contains('\\') || s.contains('\0') {
        return false;
    }
    !s.chars().any(char::is_control)
}

#[allow(dead_code)]
fn _force_impl(_r: impl IntoResponse) {}
