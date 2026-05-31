//! `POST /api/trust` — record first-run consent for a remote workflow.
//!
//! Body mirrors the `consent_required` shape returned by
//! `GET /api/workflow` / `POST /api/runs` 409:
//! `{ host, repo, subpath, ref, sha, declared_capabilities }`.

use axum::{Json, extract::State};
use cori_run::remote::refspec::{RemoteRef, RemoteRefKind, Transport};
use cori_run::remote::trust;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{error::ApiError, state::AppState};

#[derive(Deserialize)]
pub struct TrustBody {
    pub host: String,
    pub repo: String,
    #[serde(default)]
    pub subpath: String,
    #[serde(rename = "ref", default)]
    pub ref_str: String,
    pub sha: String,
    #[serde(default)]
    pub declared_capabilities: Vec<String>,
}

pub async fn handler(
    State(_state): State<AppState>,
    Json(body): Json<TrustBody>,
) -> Result<Json<Value>, ApiError> {
    tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        let spec = RemoteRef {
            host: body.host,
            repo: body.repo,
            subpath: body.subpath,
            ref_str: body.ref_str,
            kind: RemoteRefKind::ExactSha,
            explicit_split: false,
            transport: Transport::Https,
        };
        trust::record_consent(&spec, &body.sha, body.declared_capabilities)?;
        Ok(())
    })
    .await
    .map_err(|e| ApiError::Internal(anyhow::anyhow!("trust task join: {e}")))??;

    Ok(Json(json!({ "ok": true })))
}
