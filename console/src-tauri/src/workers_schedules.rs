//! Workers + schedules read/CRUD. Mirrors the deleted axum
//! `/api/workers` and `/api/schedules*` endpoints.

use cori_broker::identity::{IdentitySource, OsUser};
use cori_protocol::task_queue_for;
use cori_run::{planner, preflight, schedules};
use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::error::{IpcError, IpcResult};

// ---------- list_workers ----------

#[tauri::command(rename_all = "snake_case")]
pub async fn list_workers() -> IpcResult<Value> {
    tokio::task::spawn_blocking(|| -> anyhow::Result<Value> {
        let identity = OsUser.resolve()?;
        let my_queue = task_queue_for(&identity);
        let cluster = planner::ClusterView::load().unwrap_or_default();

        let workers: Vec<Value> = cluster
            .reports
            .iter()
            .map(|r| {
                let kind = match &r.identity {
                    cori_protocol::WorkerIdentity::Person { .. } => "user",
                    cori_protocol::WorkerIdentity::Service { .. } => "shared",
                };
                json!({
                    "task_queue": r.task_queue,
                    "identity": r.identity,
                    "kind": kind,
                    "is_self": r.task_queue == my_queue,
                    "capabilities": r.capabilities,
                })
            })
            .collect();

        Ok(json!({
            "this_queue": my_queue,
            "workers": workers,
        }))
    })
    .await
    .map_err(|e| IpcError::Internal(anyhow::anyhow!("workers task join: {e}")))?
    .map_err(IpcError::Internal)
}

// ---------- list_schedules ----------

#[derive(Serialize)]
pub struct ScheduleDto {
    #[serde(flatten)]
    entry: schedules::ScheduleEntry,
    next_fire_at: Option<chrono::DateTime<chrono::Utc>>,
    is_self_identity: bool,
}

#[tauri::command(rename_all = "snake_case")]
pub async fn list_schedules() -> IpcResult<Vec<ScheduleDto>> {
    tokio::task::spawn_blocking(|| -> anyhow::Result<Vec<ScheduleDto>> {
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
    .map_err(|e| IpcError::Internal(anyhow::anyhow!("schedules task join: {e}")))?
    .map_err(IpcError::Internal)
}

// ---------- enable_schedule (create) ----------

#[tauri::command(rename_all = "snake_case")]
pub async fn enable_schedule(
    source: String,
    schedule: Option<String>,
    schedule_tz: Option<String>,
) -> IpcResult<Value> {
    tokio::task::spawn_blocking(move || -> anyhow::Result<Value> {
        let me = OsUser.resolve()?;
        let identity_queue = task_queue_for(&me);

        let pre = preflight(&source, false, false)?;
        let manifest_cron = pre.loaded.compiled.manifest.schedule.clone();
        let manifest_tz = pre.loaded.compiled.manifest.schedule_tz.clone();

        let cron = schedule
            .or(manifest_cron)
            .ok_or_else(|| anyhow::anyhow!("no `schedule` field in manifest and none provided"))?;
        let tz = schedule_tz.or(manifest_tz);
        let resolved_sha = None;
        let entry = schedules::new_entry(source, cron, tz, identity_queue, resolved_sha)?;
        schedules::save(&entry)?;
        Ok(json!({
            "id": entry.id,
            "entry": entry,
            "next_fire_at": schedules::next_fire(&entry),
        }))
    })
    .await
    .map_err(|e| IpcError::Internal(anyhow::anyhow!("create schedule task join: {e}")))?
    .map_err(|e| IpcError::BadRequest(format!("{e:#}")))
}

// ---------- set_schedule_enabled ----------

#[tauri::command(rename_all = "snake_case")]
pub async fn set_schedule_enabled(id: String, enabled: bool) -> IpcResult<Value> {
    tokio::task::spawn_blocking(move || -> anyhow::Result<Value> {
        let me = OsUser.resolve()?;
        let my_queue = task_queue_for(&me);

        let entry = schedules::load(&id)?.ok_or_else(|| anyhow::anyhow!("no schedule `{id}`"))?;
        if entry.identity != my_queue {
            anyhow::bail!(
                "schedule `{}` is owned by `{}`; this Console is `{}`. \
                 Open it from a `cori work` running as that identity to mutate.",
                id,
                entry.identity,
                my_queue
            );
        }
        let updated = schedules::set_enabled(&id, enabled)?;
        Ok(json!({
            "id": updated.id,
            "entry": updated,
            "next_fire_at": schedules::next_fire(&updated),
        }))
    })
    .await
    .map_err(|e| IpcError::Internal(anyhow::anyhow!("patch schedule task join: {e}")))?
    .map_err(|e| IpcError::BadRequest(format!("{e:#}")))
}

// ---------- delete_schedule ----------

#[tauri::command(rename_all = "snake_case")]
pub async fn delete_schedule(id: String) -> IpcResult<Map<String, Value>> {
    tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        let me = OsUser.resolve()?;
        let my_queue = task_queue_for(&me);

        let entry = schedules::load(&id)?.ok_or_else(|| anyhow::anyhow!("no schedule `{id}`"))?;
        if entry.identity != my_queue {
            anyhow::bail!(
                "schedule `{}` is owned by `{}`; this Console is `{}`.",
                id,
                entry.identity,
                my_queue
            );
        }
        schedules::delete(&id)
    })
    .await
    .map_err(|e| IpcError::Internal(anyhow::anyhow!("delete schedule task join: {e}")))?
    .map_err(|e| IpcError::BadRequest(format!("{e:#}")))?;
    Ok(Map::new())
}
