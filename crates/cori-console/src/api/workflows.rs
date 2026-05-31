//! `GET /api/workflows/recent` — recents derived from `~/.cori/runs/<key>/`.

use std::path::Path;

use axum::{Json, extract::State};
use chrono::{DateTime, Utc};
use cori_protocol::RunTrace;
use serde::Serialize;

use crate::{error::ApiError, state::AppState};

#[derive(Serialize)]
pub struct RecentWorkflow {
    /// Directory name under `~/.cori/runs/` — the run-history key.
    pub key: String,
    /// `workflow_id` of the most recent trace under this key.
    pub workflow_id: String,
    /// Origin recorded on the latest trace (`local` / `remote`), if known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<serde_json::Value>,
    pub last_run_at: DateTime<Utc>,
    pub last_status: String,
    pub run_count: usize,
}

pub async fn recent(State(state): State<AppState>) -> Result<Json<Vec<RecentWorkflow>>, ApiError> {
    let runs_root = state.home.join("runs");
    let out = tokio::task::spawn_blocking(move || collect(&runs_root))
        .await
        .map_err(|e| ApiError::Internal(anyhow::anyhow!("recent task join: {e}")))??;
    Ok(Json(out))
}

fn collect(runs_root: &Path) -> anyhow::Result<Vec<RecentWorkflow>> {
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
                source: t.source.as_ref().and_then(|s| serde_json::to_value(s).ok()),
                last_run_at: t.started_at,
                last_status: t.status,
                run_count: count,
            });
        }
    }
    out.sort_by_key(|w| std::cmp::Reverse(w.last_run_at));
    Ok(out)
}
