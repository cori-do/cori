//! `list_remote_workflows` IPC — a thin pass-through to
//! [`cori_run::remote::list_workflows`]. The clone / cache / pin /
//! walk / manifest-parse logic all lives in `cori-run`; this module
//! only handles deserialization, error mapping, and async dispatch.

use cori_run::remote::{RemoteWorkflowEntry, list_workflows};
use cori_run::remote::refspec::parse_remote_ref;
use serde::Serialize;

use crate::error::{IpcError, IpcResult};

#[derive(Debug, Serialize)]
pub struct RemoteListingDto {
    /// The resolved host (e.g. `github.com`).
    pub host: String,
    /// `owner/repo` segment.
    pub repo: String,
    /// In-repo subpath the user originally targeted (may be empty —
    /// the bare-repo case). Listed workflows live under this subtree.
    pub spec_subpath: String,
    /// What the user typed after `@` (may be empty — "latest semver").
    pub ref_str: String,
    /// Resolved sha. `sha[..8]` is the breadcrumb pin.
    pub sha: String,
    pub workflows: Vec<RemoteWorkflowEntry>,
}

#[tauri::command(rename_all = "snake_case")]
pub async fn list_remote_workflows(
    ref_str: String,
    update: Option<bool>,
) -> IpcResult<RemoteListingDto> {
    let update = update.unwrap_or(false);
    // Stays on the blocking pool because the underlying `git ls-remote`
    // and `git fetch`/`git checkout` are sync and may block on network.
    tokio::task::spawn_blocking(move || -> IpcResult<RemoteListingDto> {
        let spec = parse_remote_ref(&ref_str).map_err(IpcError::Internal)?;
        let host = spec.host.clone();
        let repo = spec.repo.clone();
        let spec_subpath = spec.subpath.clone();
        let captured_ref = spec.ref_str.clone();
        let listing = list_workflows(&spec, update).map_err(IpcError::Internal)?;
        Ok(RemoteListingDto {
            host,
            repo,
            spec_subpath,
            ref_str: captured_ref,
            sha: listing.sha,
            workflows: listing.workflows,
        })
    })
    .await
    .map_err(|e| IpcError::Internal(anyhow::anyhow!("list_remote_workflows join: {e}")))?
}
