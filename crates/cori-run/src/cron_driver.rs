//! Cron driver — fires schedules from the `~/.cori/schedules/` store.
//!
//! `cori work` spawns one instance per worker. The driver scans the
//! store every `SCAN_INTERVAL`, computes the cron fire times that fell
//! between the previous tick and now, and dispatches one
//! `run_workflow` call per fire on a dedicated OS thread (Temporal SDK
//! futures aren't `Send`, same workaround as the Console trigger).
//!
//! Identity gating: a driver only fires entries whose `identity`
//! matches its own task queue. A user-owned worker won't fire a shared
//! pool's schedules and vice versa — that's the contract enforced by
//! the Console's create endpoint.

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use cori_protocol::{WorkerIdentity, task_queue_for};
use serde_json::Value as JsonValue;

use crate::{ConsentCallback, NoopSink, RunRequest, Trigger, run_workflow, schedules};

const SCAN_INTERVAL: Duration = Duration::from_secs(30);

/// Run the cron driver until cancelled. Returns only when the
/// `shutdown` future resolves — the caller is responsible for
/// dropping it on process exit.
pub async fn run<F: std::future::Future<Output = ()>>(identity: WorkerIdentity, shutdown: F) {
    let queue = task_queue_for(&identity);
    let mut last_check = Utc::now();
    tracing::info!(task_queue = %queue, "cron driver started");

    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            _ = &mut shutdown => break,
            _ = tokio::time::sleep(SCAN_INTERVAL) => {}
        }

        let now = Utc::now();
        let entries = match schedules::for_identity(&queue) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(error = %format!("{e:#}"), "schedule store read failed");
                continue;
            }
        };
        for entry in entries {
            if !entry.enabled {
                continue;
            }
            let due = due_fires(&entry, last_check, now);
            for fire_at in due {
                tracing::info!(
                    id = %entry.id,
                    source = %entry.source,
                    fire_at = %fire_at,
                    "firing scheduled run",
                );
                spawn_fire(entry.clone(), fire_at);
            }
        }
        last_check = now;
    }
    tracing::info!("cron driver stopped");
}

/// Cron fire times in `(prev, now]`. Up to 16 per scan to bound work.
fn due_fires(
    entry: &schedules::ScheduleEntry,
    prev: DateTime<Utc>,
    now: DateTime<Utc>,
) -> Vec<DateTime<Utc>> {
    let Ok(schedule) = entry.schedule.parse::<cron::Schedule>() else {
        return Vec::new();
    };
    schedule
        .after(&prev)
        .take_while(|t| *t <= now)
        .take(16)
        .collect()
}

/// Spin up a dedicated thread + current-thread tokio runtime, run the
/// workflow synchronously, then update the store with the result.
/// Same pattern as the Console trigger endpoint — Temporal SDK
/// futures hold non-`Send` state.
fn spawn_fire(entry: schedules::ScheduleEntry, fire_at: DateTime<Utc>) {
    let entry_id = entry.id.clone();
    let _ = std::thread::Builder::new()
        .name(format!("cori-schedule-{}", entry.id))
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    tracing::warn!(id = %entry_id, error = %e, "schedule fire: tokio build failed");
                    return;
                }
            };
            let req = RunRequest {
                source: entry.source.clone(),
                params: JsonValue::Object(Default::default()),
                dry_run: false,
                update: false,
                trigger: Trigger::Schedule,
                run_id: None,
            };
            let result = rt.block_on(run_workflow(
                req,
                ConsentCallback::AssumeYes,
                Arc::new(NoopSink),
            ));
            match result {
                Ok(trace) => {
                    let _ = schedules::record_fire(
                        &entry_id,
                        &trace.status,
                        trace.error.as_deref(),
                        fire_at,
                    );
                }
                Err(e) => {
                    let _ = schedules::record_fire(
                        &entry_id,
                        "failed",
                        Some(&format!("{e:#}")),
                        fire_at,
                    );
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry_with_cron(expr: &str) -> schedules::ScheduleEntry {
        schedules::ScheduleEntry {
            id: "x".into(),
            source: "./x".into(),
            resolved_sha: None,
            schedule: expr.to_string(),
            schedule_tz: None,
            identity: "cori.user.x".into(),
            enabled: true,
            created_at: Utc::now(),
            last_reconciled_at: None,
            last_fire_at: None,
            last_status: None,
            last_error: None,
        }
    }

    #[test]
    fn due_fires_skips_invalid_cron() {
        let e = entry_with_cron("not a cron");
        let now = Utc::now();
        let prev = now - chrono::Duration::seconds(60);
        assert!(due_fires(&e, prev, now).is_empty());
    }

    #[test]
    fn due_fires_counts_minute_ticks_in_window() {
        // 6-field POSIX-with-seconds: every 30 seconds.
        let e = entry_with_cron("*/30 * * * * *");
        let now = Utc::now();
        let prev = now - chrono::Duration::seconds(120);
        let fires = due_fires(&e, prev, now);
        // Window of 120s with 30s cadence → between 3 and 5 fires
        // depending on alignment. Just bound generously.
        assert!(!fires.is_empty() && fires.len() <= 5, "got {}", fires.len());
    }
}
