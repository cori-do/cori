// SPDX-License-Identifier: Apache-2.0
//! Temporal Schedules for `schedule:` workflows. One schedule per (app, workflow), id `cori.<app>.<workflow>`, whose
//! action starts the generic workflow with the input resolved at sync time (bundle, connectors, LLM bindings).
//! Synced on deploy, rollback, binding change and server start; the server-side cron of `scheduler.rs` only serves
//! workflows whose schedule could not be created on Temporal (`mode = server`).
use crate::state::AppState;
use cori_engine::workflow::CoriWorkflow;
use serde_json::json;
use sqlx::Row;
use std::sync::Arc;
use temporalio_client::schedules::{CreateScheduleOptions, DeleteScheduleOptions, ScheduleAction, ScheduleOverlapPolicy, ScheduleSpec};
use temporalio_client::RpcOptions;


/// Creates or updates the Temporal Schedules of an app's promoted version and removes stale ones. Never fails the
/// caller: problems are logged and the workflow falls back to the server cron.
pub async fn sync_app(st: &Arc<AppState>, app: &str) {
    if let Err(e) = sync_inner(st, app).await { tracing::warn!(app, error = %e, "schedule sync failed"); }
}

async fn sync_inner(st: &Arc<AppState>, app: &str) -> anyhow::Result<()> {
    let promoted = crate::api::promoted(st, app).await.map_err(|e| anyhow::anyhow!("{}", e.1))?;
    let mut keep: Vec<String> = Vec::new();
    if let Some((hash, wfs, cori_toml)) = promoted {
        for w in &wfs {
            let Some(expr) = &w.compiled.manifest.schedule else { continue };
            let tz = w.compiled.manifest.schedule_tz.clone().unwrap_or_else(|| "UTC".into());
            let id = crate::names::schedule_id(st.db.realm(), app, &w.id);
            keep.push(id.clone());
            let sa = w.compiled.manifest.schedule_as.clone().unwrap_or(json!({}));
            // who the runs act as: under builtin auth the service account's own token decides (verified now); without a
            // usable service account the workflow is not scheduled at all
            let user = match crate::identity::service_account_user(st, Some(app), &sa).await {
                Ok(u) => u,
                Err(e) => {
                    tracing::warn!(schedule = %id, error = %e, "schedule disabled: no service account identity");
                    let _ = st.temporal.get_schedule_handle(&id).delete(DeleteScheduleOptions::builder().build()).await;
                    sqlx::query("INSERT INTO schedules (id, app, workflow, cron, tz, mode, updated_at, user_json, version_hash, problem) VALUES ($1, $2, $3, $4, $5, 'disabled', $6, NULL, $7, $8) ON CONFLICT ON CONSTRAINT schedules_pkey DO UPDATE SET cron = $4, tz = $5, mode = 'disabled', updated_at = $6, user_json = NULL, version_hash = $7, problem = $8")
                        .bind(&id).bind(app).bind(&w.id).bind(expr).bind(&tz).bind(chrono::Utc::now().to_rfc3339()).bind(&hash).bind(&e).execute(&st.db).await?;
                    continue;
                }
            };
            let mut params = json!({});
            for p in &w.compiled.manifest.parameters { if let Some(d) = &p.default { params[&p.name] = d.clone(); } }
            let mode = match crate::api::build_input(st, app, &hash, &cori_toml, w, params, user.clone()).await {
                Ok((mut input, _)) => {
                    input.schedule = Some(id.clone());
                    let spec = ScheduleSpec::builder().cron_strings(vec![expr.clone()]).timezone_name(tz.clone()).build();
                    let action = ScheduleAction::start_workflow(CoriWorkflow::run, input.clone(), crate::names::run_queue(&st.cfg, st.db.realm(), app), crate::names::scheduled_run_prefix(st.db.realm(), app, &w.id));
                    match st.temporal.create_schedule(&id, CreateScheduleOptions::builder().action(action).spec(spec).overlap_policy(ScheduleOverlapPolicy::Skip).note(format!("cori {app} {} @ {hash}", w.id)).build()).await {
                        Ok(_) => { tracing::info!(schedule = %id, cron = %expr, "temporal schedule created"); "temporal" }
                        Err(e) if e.to_string().to_lowercase().contains("already") => {
                            let spec2 = ScheduleSpec::builder().cron_strings(vec![expr.clone()]).timezone_name(tz.clone()).build();
                            let action2 = ScheduleAction::start_workflow(CoriWorkflow::run, input, crate::names::run_queue(&st.cfg, st.db.realm(), app), crate::names::scheduled_run_prefix(st.db.realm(), app, &w.id));
                            match st.temporal.get_schedule_handle(&id).update(move |u| { u.set_spec(spec2); u.set_action(action2); }, RpcOptions::builder().build()).await {
                                Ok(_) => { tracing::info!(schedule = %id, "temporal schedule updated"); "temporal" }
                                Err(e) => { tracing::warn!(schedule = %id, error = %e, "temporal schedule update failed; server cron takes over"); "server" }
                            }
                        }
                        Err(e) => { tracing::warn!(schedule = %id, error = %e, "temporal schedule create failed; server cron takes over"); "server" }
                    }
                }
                Err(e) => { tracing::warn!(schedule = %id, error = %e.1, "schedule input could not be built; server cron takes over"); "server" }
            };
            sqlx::query("INSERT INTO schedules (id, app, workflow, cron, tz, mode, updated_at, user_json, version_hash, problem) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, NULL) ON CONFLICT ON CONSTRAINT schedules_pkey DO UPDATE SET cron = $4, tz = $5, mode = $6, updated_at = $7, user_json = $8, version_hash = $9, problem = NULL")
                .bind(&id).bind(app).bind(&w.id).bind(expr).bind(&tz).bind(mode).bind(chrono::Utc::now().to_rfc3339()).bind(user.to_string()).bind(&hash).execute(&st.db).await?;
        }
    }
    // stale schedules (workflow removed or no longer scheduled)
    let rows = sqlx::query("SELECT id FROM schedules WHERE app = $1").bind(app).fetch_all(&st.db).await?;
    for r in rows {
        let id: String = r.get("id");
        if keep.contains(&id) { continue; }
        let _ = st.temporal.get_schedule_handle(&id).delete(DeleteScheduleOptions::builder().build()).await;
        sqlx::query("DELETE FROM schedules WHERE id = $1").bind(&id).execute(&st.db).await?;
        tracing::info!(schedule = %id, "temporal schedule removed");
    }
    Ok(())
}

/// Pauses (or resumes) every Temporal Schedule of the realm, with `note`: a suspended organization's schedules start no run.
/// Returns how many it changed; a schedule Temporal does not know is skipped.
pub async fn set_paused(st: &AppState, paused: bool, note: &str) -> usize {
    let ids: Vec<String> = match sqlx::query_scalar("SELECT id FROM schedules WHERE mode = 'temporal'").fetch_all(&st.db).await { Ok(v) => v, Err(e) => { tracing::warn!(error = %e, "schedules not listed"); return 0 } };
    let mut n = 0;
    for id in ids {
        let h = st.temporal.get_schedule_handle(id.as_str());
        let r = if paused { h.pause(Some(note.to_string()), temporalio_client::schedules::PauseScheduleOptions::default()).await } else { h.unpause(Some(note.to_string()), temporalio_client::schedules::UnpauseScheduleOptions::default()).await };
        match r { Ok(()) => n += 1, Err(e) => tracing::warn!(schedule = %id, error = %e, paused, "schedule not changed") }
    }
    n
}

/// Every app with a promoted version, at server start (bundles or bindings may have changed while the server was down).
pub async fn sync_all(st: &Arc<AppState>) {
    let apps: Vec<String> = match sqlx::query("SELECT id FROM apps WHERE promoted_hash IS NOT NULL").fetch_all(&st.db).await { Ok(r) => r.iter().map(|r| r.get::<String, _>("id")).collect(), Err(_) => vec![] };
    for app in apps { sync_app(st, &app).await; }
}
