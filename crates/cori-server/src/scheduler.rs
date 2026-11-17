// SPDX-License-Identifier: Apache-2.0
//! Server-side scheduler for `schedule:` workflows.
//!
//! Temporal Schedules would be the natural home, but the Rust client's schedule action cannot carry a
//! workflow input yet; until it can, the server evaluates the cron expressions itself and starts runs with
//! idempotent ids (`sched-<app>-<workflow>-<fire time>`), so a restart never double-fires, and only the leading replica
//! runs it (`run`).
use crate::{api, state::AppState};
use chrono::{DateTime, Utc};
use serde_json::json;
use sqlx::Row;
use std::sync::Arc;

/// How often a replica that does not lead tries to take the lead.
const ELECT_EVERY: std::time::Duration = std::time::Duration::from_secs(15);

/// The cluster's background work, done by one replica: the one holding the advisory lock `cori_leader` on a connection of
/// its own (`elect`). When that replica stops, its connection closes, the lock goes, and another replica takes over
/// within `ELECT_EVERY`. Taking the lead first syncs the Temporal schedules and provisions missing live stores. Each
/// piece of work runs in every realm in turn (`AppState::realms`: the core's single one).
pub async fn run(st: Arc<AppState>) {
    loop {
        if let Some(mut lock) = elect(&st.cfg.database_url).await {
            tracing::info!("this replica runs the background work (schedules, reconcile, retention)");
            for st in realms(&st).await {
                if !st.cfg.server_scheduler { crate::schedules::sync_all(&st).await; }
                crate::stores::provision_missing_live(&st).await;
                // fronts that moved (a new fronts URL) since their clients were registered: the identity provider hears of it
                if let Err(e) = crate::client_hook::sync_origins(&st).await { tracing::warn!(error = %e, "front origins not checked"); }
            }
            lead(&st, &mut lock).await;
            tracing::warn!("this replica lost the lead (its lock connection failed): electing again");
        }
        tokio::time::sleep(ELECT_EVERY).await;
    }
}

/// The realms the background work runs in now (none when they cannot be listed: the next round tries again).
async fn realms(st: &Arc<AppState>) -> Vec<Arc<AppState>> {
    st.realms().await.unwrap_or_else(|e| { tracing::warn!(error = %e, "the realms could not be listed: background work skipped"); Vec::new() })
}

/// The lock connection when this replica takes the lead, else None (another one leads, or the database is unreachable).
pub(crate) async fn elect(database_url: &str) -> Option<sqlx::PgConnection> {
    use sqlx::Connection;
    let mut c = match sqlx::PgConnection::connect(database_url).await { Ok(c) => c, Err(e) => { tracing::warn!(error = %e, "leader election: no connection"); return None } };
    match sqlx::query_scalar::<_, bool>("SELECT pg_try_advisory_lock(hashtext('cori_leader'))").fetch_one(&mut c).await { Ok(true) => Some(c), _ => None }
}

/// How many rounds of the loop (20 s each) between two comparisons of the people (`CORI_PEOPLE_COMPARE_LOOPS`, a test's;
/// 30 rounds, 10 minutes, by default).
fn compare_every() -> u64 { std::env::var("CORI_PEOPLE_COMPARE_LOOPS").ok().and_then(|v| v.trim().parse().ok()).filter(|n| *n > 0).unwrap_or(30) }

/// The background loop of the leading replica, for as long as its lock connection answers.
async fn lead(st: &Arc<AppState>, lock: &mut sqlx::PgConnection) {
    let mut last = Utc::now();
    for n in 0u64.. {
        tokio::time::sleep(std::time::Duration::from_secs(20)).await;
        if sqlx::query("SELECT 1").execute(&mut *lock).await.is_err() { return; }
        let now = Utc::now();
        for st in realms(st).await {
            let realm = st.realm().map(|r| r.as_str().to_string()).unwrap_or_default();
            if let Err(e) = tick(&st, last, now).await { tracing::warn!(realm, error = %e, "scheduler tick failed"); }
            // what the identity provider was not told yet (each event is also tried when it is queued), and every 5 minutes
            // the roles no kept version declares any more
            if let Err(e) = crate::client_hook::deliver(&st).await { tracing::warn!(realm, error = %e, "identity events not delivered"); }
            if n % 15 == 0 { if let Err(e) = crate::client_hook::expire_requests(&st).await { tracing::warn!(realm, error = %e, "access requests not swept"); } }
            // fronts that moved (a new fronts URL, an organization's new slug): the identity provider hears of it within a minute
            if n % 3 == 0 && let Err(e) = crate::client_hook::sync_origins(&st).await { tracing::warn!(realm, error = %e, "front origins not checked"); }
            // every 10 minutes, a receiver that answers people.list but sends no notifications: who it no longer lists left
            if n % compare_every() == 0 { match api::compare_directory(&st).await { Ok(0) => {}, Ok(k) => tracing::info!(realm, people = k, "people no longer listed by the identity provider: left"), Err(e) => tracing::warn!(realm, error = %e, "people not compared") } }
            // guests whose access reached its end date (a minute late at most)
            match api::end_guest_access(&st).await { Ok(0) => {}, Ok(k) => tracing::info!(realm, guests = k, "guest access ended at its end date"), Err(e) => tracing::warn!(realm, error = %e, "guest access not ended") }
            if n % 15 == 0 { match crate::client_hook::sweep_roles(&st).await { Ok(0) => {}, Ok(k) => tracing::info!(realm, roles = k, "roles no version declares any more: role.deleted queued"), Err(e) => tracing::warn!(realm, error = %e, "role sweep failed") } }
            // right after taking the lead, then every 5 minutes: the outcome of runs whose watcher a restart took away, and the
            // realm's storage (its limit is checked against this reading)
            if n % 15 == 0 { match api::reconcile_runs(&st).await { Ok(0) => {}, Ok(k) => tracing::info!(realm, runs = k, "run outcomes recorded after a restart"), Err(e) => tracing::warn!(realm, error = %e, "run reconcile failed") } }
            if n % 15 == 0 && let Err(e) = crate::usage::measure(&st).await { tracing::warn!(realm, error = %e, "storage not measured"); }
            // every 5 minutes: uploads no run used once they expired, and files no kept run holds any more
            if n % 15 == 0 { match api::sweep_files(&st).await { Ok(0) => {}, Ok(k) => tracing::info!(realm, files = k, "files swept: expired uploads, files no run holds"), Err(e) => tracing::warn!(realm, error = %e.1, "file sweep failed") } }
            if st.cfg.retention_days > 0 && now.timestamp() % 3600 < 20 { if let Err(e) = retention(&st, now).await { tracing::warn!(realm, error = %e, "retention sweep failed"); } }
            // once an hour, the data keys a key wrapper wraps move to the newest version of their keys (a rotation in the key
            // service), so that the organization may retire the older versions
            if st.broker.wrapper().is_some() && (n == 0 || now.timestamp() % 3600 < 20) {
                match st.broker.rewrap(&st.db).await { Ok(0) => {}, Ok(k) => tracing::info!(realm, secrets = k, "data keys rewrapped under the newest key versions"), Err(e) => tracing::warn!(realm, error = %e, "rewrap failed") }
            }
        }
        last = now;
    }
}

async fn tick(st: &Arc<AppState>, from: DateTime<Utc>, to: DateTime<Utc>) -> anyhow::Result<()> {
    let apps: Vec<String> = sqlx::query("SELECT id FROM apps WHERE promoted_hash IS NOT NULL").fetch_all(&st.db).await?.iter().map(|r| r.get::<String, _>("id")).collect();
    for app in apps {
        let Some((hash, wfs, cori_toml)) = api::promoted(st, &app).await.map_err(|_| anyhow::anyhow!("promoted lookup"))? else { continue };
        for w in &wfs {
            let Some(expr) = &w.compiled.manifest.schedule else { continue };
            // Temporal Schedules own this workflow unless creation failed (mode = server) or the operator forced the server cron
            let forced = st.cfg.server_scheduler;
            if !forced { let mode: Option<String> = sqlx::query("SELECT mode FROM schedules WHERE id = $1").bind(crate::names::schedule_id(st.db.realm(), &app, &w.id)).fetch_optional(&st.db).await?.map(|r| r.get("mode")); if matches!(mode.as_deref(), Some("temporal") | Some("disabled")) { continue; } }
            // `cori check` refuses a bad expression or zone; a version compiled before it keeps the old fallbacks (UTC, skipped)
            let tz = w.compiled.manifest.schedule_tz.as_deref().and_then(|t| cori_compiler::timing::time_zone(t).ok()).unwrap_or(chrono_tz::UTC);
            let Ok(sched) = cori_compiler::timing::cron(expr) else { tracing::warn!(workflow = %w.id, "invalid cron `{expr}`"); continue };
            let from_l = from.with_timezone(&tz); let to_l = to.with_timezone(&tz);
            for fire in sched.after(&from_l).take(16) {
                if fire > to_l { break; }
                let fire_utc = fire.with_timezone(&Utc);
                let run_id = crate::names::scheduled_run_id(st.db.realm(), &app, &w.id, fire_utc);
                let exists = sqlx::query("SELECT id FROM runs WHERE id = $1").bind(&run_id).fetch_optional(&st.db).await?;
                if exists.is_some() { continue; }
                let sa = w.compiled.manifest.schedule_as.clone().unwrap_or(json!({}));
                let user = match crate::identity::service_account_user(st, Some(&app), &sa).await {
                    Ok(u) => u,
                    Err(e) => { tracing::warn!(workflow = %w.id, app = %app, error = %e, "scheduled run skipped: no service account identity"); continue; }
                };
                let account = user["sub"].as_str().unwrap_or("schedule").to_string();
                let mut params = json!({});
                for p in &w.compiled.manifest.parameters { if let Some(d) = &p.default { params[&p.name] = d.clone(); } }
                match api::launch_run(st, &app, &hash, &cori_toml, w, &run_id, params, user, &account, "schedule").await {
                    Ok(_) => tracing::info!(run = %run_id, "scheduled run started"),
                    Err(_) => tracing::warn!(run = %run_id, "scheduled run failed to start"),
                }
            }
        }
    }
    Ok(())
}

/// Hourly sweep: runs (and their events, captured LLM calls, approvals) older than `retention_days` are deleted.
async fn retention(st: &Arc<AppState>, now: DateTime<Utc>) -> anyhow::Result<()> {
    let cutoff = (now - chrono::Duration::days(st.cfg.retention_days as i64)).to_rfc3339();
    let ids: Vec<String> = sqlx::query("SELECT id FROM runs WHERE started_at < $1 AND status <> 'running'").bind(&cutoff).fetch_all(&st.db).await?.iter().map(|r| r.get::<String, _>("id")).collect();
    for id in &ids {
        sqlx::query("DELETE FROM run_events WHERE run_id = $1").bind(id).execute(&st.db).await?;
        sqlx::query("DELETE FROM llm_calls WHERE run_id = $1").bind(id).execute(&st.db).await?;
        sqlx::query("DELETE FROM approvals WHERE run_id = $1").bind(id).execute(&st.db).await?;
        // its files go with the next sweep, unless another kept run holds them
        api::forget_run_files(st, id).await?;
        sqlx::query("DELETE FROM runs WHERE id = $1").bind(id).execute(&st.db).await?;
    }
    if !ids.is_empty() { tracing::info!(deleted = ids.len(), days = st.cfg.retention_days, "retention sweep"); }
    // sandboxes of versions older than the retention window that are not promoted (they come back seeded on the next call)
    let stale: Vec<(String, String)> = sqlx::query("SELECT s.app, s.target FROM store_instances s JOIN versions v ON v.hash = s.target AND v.app = s.app WHERE v.created_at < $1 AND v.status <> 'promoted' GROUP BY s.app, s.target").bind(&cutoff).fetch_all(&st.db).await?.iter().map(|r| (r.get::<String, _>("app"), r.get::<String, _>("target"))).collect();
    for (app, hash) in stale {
        match crate::stores::drop_sandbox(st, crate::stores::Target::sandbox(&app, &hash)).await {
            Ok(names) => tracing::info!(app, version = %hash, stores = ?names, "sandbox retired by retention"),
            Err(e) => tracing::warn!(app, version = %hash, error = %e, "sandbox retirement failed"),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn one_replica_leads_and_another_takes_over_when_it_stops() {
        let Some((_db, url)) = crate::db::test_database().await else { eprintln!("CORI_STORE_DATABASE_URL not set: skipped"); return };
        let first = super::elect(&url).await.expect("the first replica leads");
        assert!(super::elect(&url).await.is_none(), "a second replica waits");
        drop(first);   // the leading replica stops: its connection closes
        let mut taken = None;
        for _ in 0..50 { taken = super::elect(&url).await; if taken.is_some() { break; } tokio::time::sleep(std::time::Duration::from_millis(100)).await; }
        assert!(taken.is_some(), "another replica takes the lead");
    }
}
