//! Self-update flow — deliberately human-initiated.
//!
//! On startup (and every 6h while resident) we *check* the feed at
//! cori.do/updates/latest.json and, if a newer signed build exists,
//! emit `updater:available { version }` — the launcher shows a banner.
//! Nothing downloads or installs until the human clicks it, which
//! invokes [`install_update`]. Consistent with the product's consent
//! posture: unattended surfaces never mutate themselves silently.

use serde_json::json;
use tauri::{AppHandle, Emitter};
use tauri_plugin_updater::UpdaterExt;
use tracing::{info, warn};

use crate::error::{IpcError, IpcResult};

pub fn spawn_check(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        // Let the stack come up first; then re-check periodically —
        // the app is tray-resident for weeks at a time.
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            match check_once(&app).await {
                Ok(Some(version)) => {
                    info!(%version, "update available");
                    let _ = app.emit("updater:available", json!({ "version": version }));
                    // Stop polling once one is announced; the banner
                    // persists until installed or the app restarts.
                    break;
                }
                Ok(None) => {}
                Err(e) => warn!(error = %e, "update check failed"),
            }
            tokio::time::sleep(std::time::Duration::from_secs(6 * 3600 - 30)).await;
        }
    });
}

async fn check_once(app: &AppHandle) -> Result<Option<String>, String> {
    let updater = app.updater().map_err(|e| e.to_string())?;
    match updater.check().await {
        Ok(Some(update)) => Ok(Some(update.version.clone())),
        Ok(None) => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

/// Human clicked "Install & restart": download, verify signature (the
/// plugin enforces the pubkey), install, relaunch.
#[tauri::command(rename_all = "snake_case")]
pub async fn install_update(app: AppHandle) -> IpcResult<()> {
    let updater = app
        .updater()
        .map_err(|e| IpcError::Internal(anyhow::anyhow!("updater init: {e}")))?;
    let update = updater
        .check()
        .await
        .map_err(|e| IpcError::Internal(anyhow::anyhow!("update check: {e}")))?
        .ok_or_else(|| IpcError::BadRequest("no update available".into()))?;
    update
        .download_and_install(|_, _| {}, || {})
        .await
        .map_err(|e| IpcError::Internal(anyhow::anyhow!("update install: {e}")))?;
    app.restart();
}
