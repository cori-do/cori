//! `GET /api/workflow?source=<path-or-ref>&update=<bool>` —
//! resolve + compile (cache hit allowed) + capability assessment.
//!
//! Mirrors `cori check`'s preflight semantics. Used by the Console's
//! `/run` screen to render the params form and the pre-run readiness
//! banner.

use axum::{Json, extract::Query};
use cori_run::preflight;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::error::ApiError;

#[derive(Deserialize)]
pub struct WorkflowQuery {
    pub source: String,
    #[serde(default)]
    pub update: Option<bool>,
}

pub async fn handler(Query(q): Query<WorkflowQuery>) -> Result<Json<Value>, ApiError> {
    let body = tokio::task::spawn_blocking(move || -> anyhow::Result<Value> {
        let p = preflight(&q.source, q.update.unwrap_or(false), false)?;
        let manifest = &p.loaded.compiled.manifest;
        let steps: Vec<Value> = p
            .loaded
            .compiled
            .steps
            .iter()
            .map(|s| {
                json!({
                    "activity_id": s.activity_id,
                    "name": s.name,
                    "kind": kind_label(s.kind),
                    "description": s.description,
                    "placement": placement_value(&s.placement),
                })
            })
            .collect();

        let consent_required = p.consent_required.as_ref().map(|cr| {
            let declared = cori_run::remote::trust::declared_capability_strings(&p.loaded.compiled);
            json!({
                "host": cr.spec.host,
                "repo": cr.spec.repo,
                "subpath": cr.spec.subpath,
                "ref": cr.spec.ref_str,
                "sha": cr.sha,
                "url": format!(
                    "https://{}/{}{}",
                    cr.spec.host,
                    cr.spec.repo,
                    if cr.spec.subpath.is_empty() { String::new() } else { format!("/tree/{}/{}", cr.sha, cr.spec.subpath) }
                ),
                "declared_capabilities": declared,
            })
        });

        let has_builtin = p
            .loaded
            .compiled
            .steps
            .iter()
            .any(|s| matches!(s.kind, cori_protocol::StepKind::Builtin));

        Ok(json!({
            "manifest": manifest,
            "content_hash": p.loaded.content_hash,
            "absolute_path": p.loaded.absolute_path.display().to_string(),
            "steps": steps,
            "required_cli_binaries": p.loaded.compiled.required_cli_binaries,
            "required_mcp_servers": p.loaded.compiled.required_mcp_servers,
            "required_llm_providers": p.loaded.compiled.required_llm_providers,
            "capabilities": p.cap_report.capabilities,
            "missing_capabilities": p.missing_caps,
            "ready": p.missing_caps.is_empty(),
            "has_builtin_step": has_builtin,
            "consent_required": consent_required,
        }))
    })
    .await
    .map_err(|e| ApiError::Internal(anyhow::anyhow!("workflow task join: {e}")))??;

    Ok(Json(body))
}

fn placement_value(p: &cori_protocol::Placement) -> Value {
    match p {
        cori_protocol::Placement::Anywhere => json!({"type": "anywhere"}),
        cori_protocol::Placement::RequiresLocalFs => json!({"type": "local_fs"}),
        cori_protocol::Placement::RequiresCapability { id } => {
            json!({"type": "capability", "id": id})
        }
    }
}

fn kind_label(kind: cori_protocol::StepKind) -> &'static str {
    match kind {
        cori_protocol::StepKind::Cli => "cli",
        cori_protocol::StepKind::McpTool => "mcp_tool",
        cori_protocol::StepKind::Code => "code",
        cori_protocol::StepKind::Llm => "llm",
        cori_protocol::StepKind::Builtin => "builtin",
    }
}
