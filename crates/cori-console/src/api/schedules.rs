//! Schedules CRUD.
//!
//!   * `GET    /api/schedules`            — list entries, joined with `next_fire`.
//!   * `POST   /api/schedules`            — register a new schedule for the current identity.
//!   * `PATCH  /api/schedules/:id`        — toggle `enabled`.
//!   * `DELETE /api/schedules/:id`        — remove an entry.
//!
//! Identity gating: the create endpoint records the **current**
//! identity (the Console's OS user / `--shared` pool). Mutations on an
//! existing entry require the caller's identity to match the entry's;
//! a personal-identity Console cannot disable a shared-pool schedule.

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use cori_broker::identity::{IdentitySource, OsUser};
use cori_protocol::task_queue_for;
use cori_run::{preflight, schedules};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{error::ApiError, state::AppState};

// ---------- list ----------

#[derive(Serialize)]
pub struct ScheduleDto {
    #[serde(flatten)]
    entry: schedules::ScheduleEntry,
    next_fire_at: Option<chrono::DateTime<chrono::Utc>>,
    is_self_identity: bool,
}

pub async fn list(State(_state): State<AppState>) -> Result<Json<Vec<ScheduleDto>>, ApiError> {
    let dtos = tokio::task::spawn_blocking(|| -> anyhow::Result<Vec<ScheduleDto>> {
        let me = OsUser.resolve()?;
        let my_queue = task_queue_for(&me);
        Ok(schedules::load_all()?
            .into_iter()
            .map(|entry| {
                let next_fire_at = schedules::next_fire(&entry);
                let is_self_identity = entry.identity == my_queue;
                ScheduleDto {
                    entry,
                    next_fire_at,
                    is_self_identity,
                }
            })
            .collect())
    })
    .await
    .map_err(|e| ApiError::Internal(anyhow::anyhow!("schedules join: {e}")))??;
    Ok(Json(dtos))
}

// ---------- create ----------

#[derive(Deserialize)]
pub struct CreateBody {
    pub source: String,
    /// Optional override; if `None` we use the manifest's `schedule`.
    #[serde(default)]
    pub schedule: Option<String>,
    /// Optional override; if `None` we use the manifest's `schedule_tz`.
    #[serde(default)]
    pub schedule_tz: Option<String>,
}

pub async fn create(
    State(_state): State<AppState>,
    Json(body): Json<CreateBody>,
) -> Result<Response, ApiError> {
    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<Value> {
        let me = OsUser.resolve()?;
        let identity_queue = task_queue_for(&me);

        // Resolve the workflow so we can read manifest defaults +
        // record the SHA for remote refs.
        let pre = preflight(&body.source, false, false)?;
        let manifest_cron = pre.loaded.compiled.manifest.schedule.clone();
        let manifest_tz = pre.loaded.compiled.manifest.schedule_tz.clone();

        let cron = body
            .schedule
            .or(manifest_cron)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "no `schedule` field in manifest and none provided in body"
                )
            })?;
        let tz = body.schedule_tz.or(manifest_tz);
        let resolved_sha = None; // populated by the resolver when remote
        let entry = schedules::new_entry(
            body.source.clone(),
            cron,
            tz,
            identity_queue,
            resolved_sha,
        )?;
        schedules::save(&entry)?;
        Ok(json!({
            "id": entry.id,
            "entry": entry,
            "next_fire_at": schedules::next_fire(&entry),
        }))
    })
    .await
    .map_err(|e| ApiError::Internal(anyhow::anyhow!("create join: {e}")))?;

    match result {
        Ok(body) => Ok((StatusCode::CREATED, Json(body)).into_response()),
        Err(e) => Err(ApiError::BadRequest(format!("{e:#}"))),
    }
}

// ---------- patch ----------

#[derive(Deserialize)]
pub struct PatchBody {
    pub enabled: Option<bool>,
}

pub async fn patch(
    State(_state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<PatchBody>,
) -> Result<Json<Value>, ApiError> {
    let value = tokio::task::spawn_blocking(move || -> anyhow::Result<Value> {
        let me = OsUser.resolve()?;
        let my_queue = task_queue_for(&me);

        let entry = schedules::load(&id)?
            .ok_or_else(|| anyhow::anyhow!("no schedule `{id}`"))?;
        if entry.identity != my_queue {
            return Err(anyhow::anyhow!(
                "schedule `{}` is owned by `{}`; this Console is `{}`. \
                 Open it from a `cori work` running as that identity to mutate.",
                id, entry.identity, my_queue
            ));
        }
        let updated = if let Some(enabled) = body.enabled {
            schedules::set_enabled(&id, enabled)?
        } else {
            entry
        };
        Ok(json!({
            "id": updated.id,
            "entry": updated,
            "next_fire_at": schedules::next_fire(&updated),
        }))
    })
    .await
    .map_err(|e| ApiError::Internal(anyhow::anyhow!("patch join: {e}")))?;
    match value {
        Ok(v) => Ok(Json(v)),
        Err(e) => {
            let msg = format!("{e:#}");
            if msg.contains("is owned by") {
                Err(ApiError::Forbidden(msg))
            } else if msg.contains("no schedule") {
                Err(ApiError::NotFound(msg))
            } else {
                Err(ApiError::BadRequest(msg))
            }
        }
    }
}

// ---------- delete ----------

pub async fn delete(
    State(_state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let id_clone = id.clone();
    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        let me = OsUser.resolve()?;
        let my_queue = task_queue_for(&me);

        if let Some(entry) = schedules::load(&id_clone)?
            && entry.identity != my_queue
        {
            return Err(anyhow::anyhow!(
                "schedule `{}` is owned by `{}`; cannot delete from this identity",
                id_clone, entry.identity
            ));
        }
        schedules::delete(&id_clone)?;
        Ok(())
    })
    .await
    .map_err(|e| ApiError::Internal(anyhow::anyhow!("delete join: {e}")))?;
    match result {
        Ok(()) => Ok(Json(json!({ "ok": true, "id": id }))),
        Err(e) => {
            let msg = format!("{e:#}");
            if msg.contains("owned by") {
                Err(ApiError::Forbidden(msg))
            } else {
                Err(ApiError::BadRequest(msg))
            }
        }
    }
}
