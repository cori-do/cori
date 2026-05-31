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
use serde::Deserialize;
use serde_json::Value;

use crate::{error::ApiError, state::AppState};

#[derive(Deserialize, Default)]
pub struct ListQuery {
    pub workflow_id: Option<String>,
    pub limit: Option<usize>,
}

pub async fn list(
    State(state): State<AppState>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Vec<RunTrace>>, ApiError> {
    let runs_root = state.home.join("runs");
    let limit = q.limit.unwrap_or(50);
    let workflow_id = q.workflow_id.clone();
    let traces = tokio::task::spawn_blocking(move || collect_traces(&runs_root, workflow_id, limit))
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
) -> anyhow::Result<Vec<RunTrace>> {
    let mut out: Vec<RunTrace> = Vec::new();
    if !runs_root.exists() {
        return Ok(out);
    }
    for hist_dir in std::fs::read_dir(runs_root)? {
        let Ok(hist_dir) = hist_dir else { continue };
        let dir = hist_dir.path();
        if !dir.is_dir() {
            continue;
        }
        let Ok(files) = std::fs::read_dir(&dir) else {
            continue;
        };
        for f in files.flatten() {
            let path: PathBuf = f.path();
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            if path
                .file_name()
                .and_then(|s| s.to_str())
                .is_some_and(|s| s.starts_with('.'))
            {
                continue;
            }
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
            out.push(trace);
        }
    }
    out.sort_by_key(|t| Reverse(t.started_at));
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
