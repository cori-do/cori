//! Capability onboarding commands — the "Connect Google Workspace"
//! button.
//!
//! `list_capabilities` mirrors `cori capability list --json`;
//! `connect_capability` mirrors `cori login <id>` for managed CLIs:
//! install if missing → provision the Cori-owned OAuth client → run the
//! vendor's own browser sign-in (non-interactive stdio; the vendor CLI
//! opens the system browser itself) → re-probe. Long-running by design:
//! the promise resolves when the user finishes (or abandons) the
//! browser consent, so the UI shows a "waiting for browser" state.

use cori_broker::cli_auth::{self, AuthState, ManagedLoginOutcome, OAuthClient};
use cori_broker::install;
use cori_run::config::Config;
use serde::Serialize;

use crate::error::{IpcError, IpcResult};

#[derive(Debug, Clone, Serialize)]
pub struct CapabilityInfo {
    pub id: String,
    pub display_name: String,
    pub installed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// `null` when the probe could not run (not installed / no adapter).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authed: Option<bool>,
    /// Connect can run end-to-end from the Console (install recipe +
    /// provisionable OAuth client + managed adapter).
    pub connectable: bool,
}

#[tauri::command(rename_all = "snake_case")]
pub async fn list_capabilities() -> IpcResult<Vec<CapabilityInfo>> {
    tokio::task::spawn_blocking(|| install::REGISTRY.iter().map(info_for).collect())
        .await
        .map_err(|e| IpcError::Internal(anyhow::anyhow!("capability list join: {e}")))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn connect_capability(id: String) -> IpcResult<CapabilityInfo> {
    tokio::task::spawn_blocking(move || connect_blocking(&id))
        .await
        .map_err(|e| IpcError::Internal(anyhow::anyhow!("capability connect join: {e}")))?
}

fn connect_blocking(id: &str) -> IpcResult<CapabilityInfo> {
    let spec = install::spec_for(id)
        .ok_or_else(|| IpcError::BadRequest(format!("unknown capability `{id}`")))?;
    let adapter = cli_auth::for_binary(id)
        .ok_or_else(|| IpcError::BadRequest(format!("capability `{id}` has no managed sign-in")))?;

    // Already signed in? Refresh state and return.
    if matches!(adapter.check(), AuthState::Ok) {
        return Ok(info_for(spec));
    }

    if install::resolve_binary(id).is_none() {
        install::install(id)
            .map_err(|e| IpcError::Internal(anyhow::anyhow!("installing `{id}`: {e}")))?;
    }

    let plan = resolve_plan(id, adapter).ok_or_else(|| {
        IpcError::BadRequest(format!(
            "no Cori-provisioned OAuth client is available for `{id}` in this build — \
             set capability.{id}.oauth_client_id / oauth_client_secret with `cori config set`"
        ))
    })?;

    match cli_auth::run_managed_login(adapter, &plan, false)
        .map_err(|e| IpcError::Internal(anyhow::anyhow!("running {id} sign-in: {e}")))?
    {
        ManagedLoginOutcome::SignedIn => Ok(info_for(spec)),
        ManagedLoginOutcome::LoginFailed { detail } => Err(IpcError::BadRequest(format!(
            "{} sign-in did not complete: {detail}",
            adapter.display_name()
        ))),
    }
}

fn resolve_plan(
    id: &str,
    adapter: &dyn cli_auth::CliAuthAdapter,
) -> Option<cli_auth::ManagedLogin> {
    let cfg = Config::load().ok();
    let get = |key: String| -> Option<String> {
        cfg.as_ref()?.get(&key).and_then(|v| match v {
            toml::Value::String(s) if !s.is_empty() => Some(s.clone()),
            _ => None,
        })
    };
    let from_config = match (
        get(format!("capability.{id}.oauth_client_id")),
        get(format!("capability.{id}.oauth_client_secret")),
    ) {
        (Some(client_id), Some(client_secret)) => Some(OAuthClient {
            client_id,
            client_secret,
            project_id: get(format!("capability.{id}.oauth_project_id")),
        }),
        _ => None,
    };
    let services: Vec<String> = get(format!("capability.{id}.services"))
        .map(|s| s.split(',').map(|p| p.trim().to_string()).collect())
        .unwrap_or_default();
    cli_auth::resolve_client(id, from_config).and_then(|c| adapter.managed_login(&c, &services))
}

fn info_for(spec: &install::InstallSpec) -> CapabilityInfo {
    let path = install::resolve_binary(spec.id);
    let installed = path.is_some();
    let authed = if installed {
        match cli_auth::check_known(spec.id) {
            AuthState::Ok => Some(true),
            AuthState::NeedsReauth { .. } => Some(false),
            AuthState::Unknown => None,
        }
    } else {
        None
    };
    let connectable = cli_auth::for_binary(spec.id)
        .map(|a| resolve_plan(spec.id, a).is_some())
        .unwrap_or(false);
    CapabilityInfo {
        id: spec.id.to_string(),
        display_name: spec.display_name.to_string(),
        installed,
        path: path.map(|p| p.display().to_string()),
        authed,
        connectable,
    }
}
