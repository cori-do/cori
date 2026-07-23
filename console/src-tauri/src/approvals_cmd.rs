//! Approval-inbox IPC + background watcher.
//!
//! The Console is the rich decision surface for `~/.cori/approvals/`
//! (see `cori-run::approvals` and `cori/docs/approvals-design.md`):
//! requesters (`cori mcp`, later the cron driver and workers) write
//! pending items; this module surfaces them and writes the human's
//! decision. It also owns the Console heartbeat that tells requesters
//! a rich surface is available at all.

use std::collections::BTreeSet;
use std::time::Duration;

use cori_run::approvals;
use serde_json::Value;
use tauri::{AppHandle, Emitter};
use tracing::warn;

use crate::error::{IpcError, IpcResult};

#[tauri::command(rename_all = "snake_case")]
pub async fn list_approvals() -> IpcResult<Value> {
    tokio::task::spawn_blocking(|| {
        let pending = approvals::list_pending()?;
        Ok(serde_json::to_value(pending)?)
    })
    .await
    .map_err(|e| IpcError::Internal(anyhow::anyhow!("approvals task join: {e}")))?
    .map_err(IpcError::Internal)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn list_decided_approvals() -> IpcResult<Value> {
    tokio::task::spawn_blocking(|| {
        let decided = approvals::list_decided(50)?;
        Ok(serde_json::to_value(decided)?)
    })
    .await
    .map_err(|e| IpcError::Internal(anyhow::anyhow!("approvals task join: {e}")))?
    .map_err(IpcError::Internal)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn decide_approval(nonce: String, approved: bool) -> IpcResult<()> {
    tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        // Capture the request before deciding — decide() retires it.
        let request = approvals::list_pending()?
            .into_iter()
            .find(|r| r.nonce == nonce);
        let decision = if approved {
            approvals::Decision::Approved
        } else {
            approvals::Decision::Declined
        };
        approvals::decide(&nonce, decision, "console")?;

        // Approving a schedule re-consent carries an effect beyond the
        // decision file: trust the new sha and resume the schedule.
        if approved
            && let Some(req) = request
            && req.kind == approvals::ApprovalKind::ScheduleReconsent
        {
            apply_schedule_reconsent(&req)?;
        }
        Ok(())
    })
    .await
    .map_err(|e| IpcError::Internal(anyhow::anyhow!("approvals task join: {e}")))?
    // Deciding an unknown/expired nonce is a caller mistake, not a crash.
    .map_err(|e| IpcError::BadRequest(format!("{e:#}")))
}

/// The human approved running the new upstream version of a scheduled
/// workflow: record trust for the new sha (same `trust.json` as every
/// other consent surface) and re-pin + resume the schedule.
fn apply_schedule_reconsent(req: &cori_run::approvals::ApprovalRequest) -> anyhow::Result<()> {
    let get = |k: &str| req.payload.get(k).and_then(|v| v.as_str());
    let (Some(schedule_id), Some(source), Some(new_sha)) =
        (get("schedule_id"), get("source"), get("new_sha"))
    else {
        anyhow::bail!("schedule_reconsent item is missing schedule_id/source/new_sha");
    };
    let caps: Vec<String> = req
        .payload
        .get("capabilities")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();

    if let cori_run::remote::ArgClass::Remote(spec) = cori_run::remote::classify_arg(source)? {
        cori_run::remote::trust::record_consent(&spec, new_sha, caps)?;
    }
    cori_run::schedules::repin(schedule_id, new_sha)?;
    tracing::info!(schedule_id, new_sha, "schedule re-consented and resumed");
    Ok(())
}

/// Spawn the heartbeat + pending-inbox watcher. Emits
/// `approvals:changed { pending: [...] }` whenever the set of pending
/// nonces changes, and surfaces the launcher when a *new* item arrives —
/// a human gate is worth interrupting for.
pub fn spawn_watcher(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut known: BTreeSet<String> = BTreeSet::new();
        let mut first = true;
        let mut ticks: u64 = 0;
        loop {
            // Heartbeat every ~20s (ticks are 2s apart).
            if ticks.is_multiple_of(10)
                && let Ok(Err(e)) = tokio::task::spawn_blocking(approvals::beat_heartbeat).await
            {
                warn!(error = %format!("{e:#}"), "could not write console heartbeat");
            }
            ticks += 1;

            let pending = tokio::task::spawn_blocking(approvals::list_pending)
                .await
                .ok()
                .and_then(|r| r.ok())
                .unwrap_or_default();
            let nonces: BTreeSet<String> = pending.iter().map(|p| p.nonce.clone()).collect();
            if nonces != known {
                let has_new = nonces.difference(&known).next().is_some();
                known = nonces;
                if let Err(e) = app.emit(
                    "approvals:changed",
                    serde_json::json!({ "pending": pending }),
                ) {
                    warn!(error = %e, "could not emit approvals:changed");
                }
                // Don't steal focus for items that predate this launch.
                if has_new && !first {
                    crate::focus_or_show_launcher(&app);
                }
            }
            first = false;
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    });
}

/// Remove the heartbeat so requesters immediately fall back to their
/// next confirmation channel instead of waiting out the staleness window.
pub fn clear_heartbeat() {
    if let Ok(path) = approvals::heartbeat_file() {
        let _ = std::fs::remove_file(path);
    }
}
