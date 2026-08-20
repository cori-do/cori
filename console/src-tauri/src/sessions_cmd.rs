//! Authoring-sessions IPC — the Console's live view of `~/.cori/sessions/`.
//!
//! MCP agents journal every workflow edit through `cori_run::sessions`;
//! the Console renders the rail (who is editing what, last event), the
//! per-session journal (the live semantic diff and the ledger), and owns
//! the human's Stop and Rewind. Disk is the only truth: a background
//! watcher polls it and pushes `sessions:changed` so the frontend never
//! needs its own timer.

use std::time::Duration;

use cori_run::{proposals, sessions};
use serde_json::{Value, json};
use tauri::{AppHandle, Emitter};
use tracing::warn;

use crate::error::{IpcError, IpcResult};

fn event_json(e: &sessions::JournalEvent) -> Value {
    json!({
        "seq": e.seq,
        "kind": e.kind,
        "rel_path": e.rel_path,
        "to_rel_path": e.to_rel_path,
        "note": e.note,
        "ts": e.ts,
    })
}

fn session_rows() -> anyhow::Result<Value> {
    let rows: Vec<Value> = sessions::list()?
        .into_iter()
        .map(|s| {
            let last_event = sessions::events(&s.session_id)
                .ok()
                .and_then(|evs| evs.into_iter().next_back())
                .map(|e| event_json(&e));
            let folder_name = s
                .workflow_dir
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            // The review card's data: present whenever the session ever
            // proposed (resolved proposals keep their audit record).
            let proposal = proposals::load(&s.session_id)
                .ok()
                .flatten()
                .and_then(|p| serde_json::to_value(p).ok());
            json!({
                "session_id": s.session_id,
                "agent": s.agent,
                "workflow_dir": s.workflow_dir.display().to_string(),
                "folder_name": folder_name,
                "state": s.state,
                "stop_reason": s.stop_reason,
                "created_at": s.created_at,
                "updated_at": s.updated_at,
                "current_seq": s.next_seq.saturating_sub(1),
                "last_event": last_event,
                "proposal": proposal,
            })
        })
        .collect();
    Ok(json!(rows))
}

/// Sessions newest-first, each with its journal tail summarized.
#[tauri::command(rename_all = "snake_case")]
pub async fn list_authoring_sessions() -> IpcResult<Value> {
    tokio::task::spawn_blocking(session_rows)
        .await
        .map_err(|e| IpcError::Internal(anyhow::anyhow!("sessions task join: {e}")))?
        .map_err(IpcError::Internal)
}

/// The full journal of one session, oldest-first — the data behind the
/// live semantic diff and the ledger. Every row is one attributable,
/// undoable agent operation.
#[tauri::command(rename_all = "snake_case")]
pub async fn session_journal(session_id: String) -> IpcResult<Value> {
    tokio::task::spawn_blocking(move || -> anyhow::Result<Value> {
        let events: Vec<Value> = sessions::events(&session_id)?
            .iter()
            .map(event_json)
            .collect();
        Ok(json!(events))
    })
    .await
    .map_err(|e| IpcError::Internal(anyhow::anyhow!("sessions task join: {e}")))?
    .map_err(|e| IpcError::BadRequest(format!("{e:#}")))
}

/// The human presses Stop: the agent's next mutating call returns
/// `session_stopped` with this reason.
#[tauri::command(rename_all = "snake_case")]
pub async fn stop_authoring_session(session_id: String, reason: Option<String>) -> IpcResult<()> {
    tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        sessions::stop(&session_id, reason.as_deref())?;
        Ok(())
    })
    .await
    .map_err(|e| IpcError::Internal(anyhow::anyhow!("sessions task join: {e}")))?
    .map_err(|e| IpcError::BadRequest(format!("{e:#}")))
}

/// Rewind the workflow folder to the state after journal entry `seq` —
/// the same operation the agent-side `session_rewind` MCP tool performs,
/// now on the human's side of the glass.
#[tauri::command(rename_all = "snake_case")]
pub async fn rewind_authoring_session(session_id: String, seq: u64) -> IpcResult<()> {
    tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        sessions::rewind(&session_id, seq)?;
        Ok(())
    })
    .await
    .map_err(|e| IpcError::Internal(anyhow::anyhow!("sessions task join: {e}")))?
    .map_err(|e| IpcError::BadRequest(format!("{e:#}")))
}

/// The human accepts a proposal: re-verifies the gates, publishes the
/// next version, and stops the session — the same consent act as a
/// granted publish approval, recorded in the version's metadata.
#[tauri::command(rename_all = "snake_case")]
pub async fn accept_authoring_proposal(session_id: String) -> IpcResult<Value> {
    tokio::task::spawn_blocking(move || -> anyhow::Result<Value> {
        let out = proposals::accept(&session_id, "console", None)?;
        Ok(serde_json::to_value(out)?)
    })
    .await
    .map_err(|e| IpcError::Internal(anyhow::anyhow!("sessions task join: {e}")))?
    .map_err(|e| IpcError::BadRequest(format!("{e:#}")))
}

/// The human rejects a proposal: the session stops with the reason
/// (echoed to the agent), optionally rewinding the folder to its
/// pre-session state.
#[tauri::command(rename_all = "snake_case")]
pub async fn reject_authoring_proposal(
    session_id: String,
    reason: Option<String>,
    discard_changes: bool,
) -> IpcResult<()> {
    tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        proposals::reject(&session_id, "console", reason.as_deref(), discard_changes)?;
        Ok(())
    })
    .await
    .map_err(|e| IpcError::Internal(anyhow::anyhow!("sessions task join: {e}")))?
    .map_err(|e| IpcError::BadRequest(format!("{e:#}")))
}

/// Poll `~/.cori/sessions/` and push `sessions:changed` whenever any
/// session's (id, seq, state) signature moves. Journal writes come from
/// other processes; 2s matches the approvals watcher cadence.
pub fn spawn_watcher(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut known: Option<Vec<(String, u64, String)>> = None;
        loop {
            let signature = tokio::task::spawn_blocking(|| {
                sessions::list().map(|rows| {
                    rows.iter()
                        .map(|s| (s.session_id.clone(), s.next_seq, format!("{:?}", s.state)))
                        .collect::<Vec<_>>()
                })
            })
            .await
            .ok()
            .and_then(|r| r.ok());

            if let Some(sig) = signature
                && known.as_ref() != Some(&sig)
            {
                known = Some(sig);
                match tokio::task::spawn_blocking(session_rows).await {
                    Ok(Ok(rows)) => {
                        if let Err(e) = app.emit("sessions:changed", json!({ "sessions": rows })) {
                            warn!(error = %e, "could not emit sessions:changed");
                        }
                    }
                    Ok(Err(e)) => warn!(error = %format!("{e:#}"), "sessions watcher list failed"),
                    Err(e) => warn!(error = %e, "sessions watcher join failed"),
                }
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    });
}
