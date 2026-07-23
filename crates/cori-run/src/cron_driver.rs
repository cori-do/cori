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
use serde_json::{Value as JsonValue, json};

use crate::{
    ConsentCallback, ConsentDecision, NoopSink, RunRequest, Trigger, approvals, remote,
    run_workflow, schedules, workflow_loader,
};

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
                // Concurrent drivers (`cori work` + the desktop app) both
                // scan; only the first to claim a (schedule, fire) runs it.
                match schedules::claim_fire(&entry.id, fire_at) {
                    Ok(true) => {}
                    Ok(false) => {
                        tracing::debug!(id = %entry.id, fire_at = %fire_at, "fire already claimed");
                        continue;
                    }
                    Err(e) => {
                        tracing::warn!(id = %entry.id, error = %format!("{e:#}"), "claim failed");
                        continue;
                    }
                }
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

/// Cron fire times in `(prev, now]`, evaluated on the wall clock of the
/// entry's `schedule_tz` (UTC when unset — but a plain-language picker
/// promising "every Monday 9am" must fire at 9am *local*, so the tz is
/// consulted, not just validated). Up to 16 per scan to bound work.
fn due_fires(
    entry: &schedules::ScheduleEntry,
    prev: DateTime<Utc>,
    now: DateTime<Utc>,
) -> Vec<DateTime<Utc>> {
    let Ok(schedule) = entry.schedule.parse::<cron::Schedule>() else {
        return Vec::new();
    };
    match entry
        .schedule_tz
        .as_deref()
        .and_then(|t| t.parse::<chrono_tz::Tz>().ok())
    {
        Some(tz) => schedule
            .after(&prev.with_timezone(&tz))
            .map(|t| t.with_timezone(&Utc))
            .take_while(|t| *t <= now)
            .take(16)
            .collect(),
        None => schedule
            .after(&prev)
            .take_while(|t| *t <= now)
            .take(16)
            .collect(),
    }
}

/// Rewrite a remote source to fire at an exact consented sha. Local
/// paths (and unparseable sources) return `None` — no pin semantics.
fn pinned_source(source: &str, sha: &str) -> Option<String> {
    match remote::classify_arg(source).ok()? {
        remote::ArgClass::Remote(mut spec) => {
            spec.ref_str = sha.to_string();
            Some(spec.display())
        }
        remote::ArgClass::Local(_) => None,
    }
}

/// Decide what this fire should actually run, enforcing the Q5 pin:
/// - local source → run as-is;
/// - remote, upstream still at the pin (or unreachable) → run the pin;
/// - remote, upstream moved → pause the schedule, file a
///   `schedule_reconsent` approval item, run nothing.
///
/// Returns `Some(source_to_run)` or `None` when the fire must be skipped.
fn resolve_fire_source(entry: &schedules::ScheduleEntry) -> Option<String> {
    if !matches!(
        remote::classify_arg(&entry.source),
        Ok(remote::ArgClass::Remote(_))
    ) {
        return Some(entry.source.clone());
    }

    // Re-resolve the (possibly mutable) upstream ref to detect drift.
    let current_sha = workflow_loader::resolve_arg(&entry.source, true)
        .ok()
        .and_then(|(resolved, loaded)| {
            resolved.remote.as_ref().map(|rr| {
                (
                    rr.sha.clone(),
                    remote::trust::declared_capability_strings(&loaded.compiled),
                )
            })
        });

    match (&entry.resolved_sha, current_sha) {
        // Upstream moved past the consented pin: pause + ask the human.
        (Some(pin), Some((sha, caps))) if *pin != sha => {
            pause_for_reconsent(entry, pin, &sha, caps);
            None
        }
        // Upstream unchanged, or unreachable (offline): the consented
        // pin is always safe to run.
        (Some(pin), _) => {
            Some(pinned_source(&entry.source, pin).unwrap_or_else(|| entry.source.clone()))
        }
        // Legacy entry with no pin: adopt the current sha as the pin
        // (this is exactly what would have fired under the old
        // behaviour) so future drift pauses instead of silently running.
        (None, Some((sha, _))) => {
            if let Err(e) = schedules::repin(&entry.id, &sha) {
                tracing::warn!(id = %entry.id, error = %format!("{e:#}"), "could not adopt pin");
            }
            Some(pinned_source(&entry.source, &sha).unwrap_or_else(|| entry.source.clone()))
        }
        // No pin and upstream unreachable: nothing consented to run.
        (None, None) => {
            tracing::warn!(id = %entry.id, "unpinned remote schedule and upstream unreachable — skipping fire");
            None
        }
    }
}

fn pause_for_reconsent(
    entry: &schedules::ScheduleEntry,
    pin: &str,
    new_sha: &str,
    capabilities: Vec<String>,
) {
    let reason = format!(
        "upstream moved from {} to {} — approve the new version to resume",
        &pin[..12.min(pin.len())],
        &new_sha[..12.min(new_sha.len())],
    );
    if let Err(e) = schedules::pause(&entry.id, &reason) {
        tracing::warn!(id = %entry.id, error = %format!("{e:#}"), "could not pause schedule");
    }

    // One open item per schedule is enough.
    let already = approvals::list_pending()
        .map(|items| {
            items.iter().any(|i| {
                i.kind == approvals::ApprovalKind::ScheduleReconsent
                    && i.payload.get("schedule_id").and_then(|v| v.as_str())
                        == Some(entry.id.as_str())
            })
        })
        .unwrap_or(false);
    if already {
        return;
    }
    let message = format!(
        "Schedule for `{source}` is paused: the workflow changed upstream \
         ({old} → {new}). Approve to trust the new version and resume the \
         schedule; decline to keep it paused.",
        source = entry.source,
        old = &pin[..12.min(pin.len())],
        new = &new_sha[..12.min(new_sha.len())],
    );
    let payload = json!({
        "schedule_id": entry.id,
        "source": entry.source,
        "pinned_sha": pin,
        "new_sha": new_sha,
        "capabilities": capabilities,
        "schedule": entry.schedule,
        "schedule_tz": entry.schedule_tz,
    });
    if let Err(e) = approvals::submit(
        approvals::ApprovalKind::ScheduleReconsent,
        "schedule",
        &message,
        payload,
        Duration::from_secs(7 * 24 * 3600),
    ) {
        tracing::warn!(id = %entry.id, error = %format!("{e:#}"), "could not file reconsent item");
    }
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
            // Q5: fire the consented pin (or pause + re-consent on
            // upstream drift). `None` = this fire must not run.
            let Some(source) = resolve_fire_source(&entry) else {
                let _ = schedules::record_fire(
                    &entry_id,
                    "skipped",
                    Some("paused pending re-consent (or upstream unreachable for an unpinned schedule)"),
                    fire_at,
                );
                return;
            };
            let req = RunRequest {
                source,
                params: JsonValue::Object(Default::default()),
                dry_run: false,
                update: false,
                trigger: Trigger::Schedule,
                run_id: None,
            };
            // Never AssumeYes on an unattended surface: the pin was
            // consented at create; anything untrusted fails closed with
            // `consent_required` in the fire record.
            let result = rt.block_on(run_workflow(
                req,
                ConsentCallback::Prompt(Box::new(|_| ConsentDecision::Defer)),
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
            paused_reason: None,
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

    #[test]
    fn due_fires_honours_schedule_tz() {
        // Daily 09:30:00 wall clock. In July, Europe/Paris is UTC+2, so
        // the Paris schedule fires at 07:30Z — inside our 07:00–08:00Z
        // window — while the same expression in UTC fires at 09:30Z,
        // outside it. This is exactly the "silently fires at 9am UTC"
        // bug the roadmap flags as blocking.
        let prev: DateTime<Utc> = "2026-07-24T07:00:00Z".parse().unwrap();
        let now: DateTime<Utc> = "2026-07-24T08:00:00Z".parse().unwrap();

        let mut paris = entry_with_cron("0 30 9 * * *");
        paris.schedule_tz = Some("Europe/Paris".into());
        let fires = due_fires(&paris, prev, now);
        assert_eq!(fires.len(), 1, "Paris 09:30 must fire at 07:30Z");
        assert_eq!(
            fires[0],
            "2026-07-24T07:30:00Z".parse::<DateTime<Utc>>().unwrap()
        );

        let utc = entry_with_cron("0 30 9 * * *");
        assert!(
            due_fires(&utc, prev, now).is_empty(),
            "UTC 09:30 must not fire inside 07:00–08:00Z"
        );
    }

    #[test]
    fn pinned_source_rewrites_remote_refs_only() {
        assert_eq!(
            pinned_source("github.com/acme/flows/report@v1", "abc123def456").as_deref(),
            Some("github.com/acme/flows/report@abc123def456")
        );
        assert_eq!(
            pinned_source("github.com/acme/flows", "abc123def456").as_deref(),
            Some("github.com/acme/flows@abc123def456")
        );
        assert!(pinned_source("./local/folder", "abc123def456").is_none());
    }

    #[test]
    fn claim_fire_dedupes_concurrent_drivers() {
        crate::test_env::with_temp_home(|| {
            let at: DateTime<Utc> = "2026-07-24T07:30:00Z".parse().unwrap();
            assert!(
                schedules::claim_fire("sched1", at).unwrap(),
                "first claim wins"
            );
            assert!(
                !schedules::claim_fire("sched1", at).unwrap(),
                "second claim loses"
            );
            // Different fire time or schedule: independent claims.
            assert!(schedules::claim_fire("sched1", at + chrono::Duration::hours(1)).unwrap());
            assert!(schedules::claim_fire("sched2", at).unwrap());
        });
    }

    #[test]
    fn pause_and_repin_roundtrip() {
        crate::test_env::with_temp_home(|| {
            let entry = schedules::new_entry(
                "github.com/acme/flows@v1".into(),
                "0 30 9 * * *".into(),
                None,
                "cori.user.t".into(),
                Some("oldsha0000".into()),
            )
            .unwrap();
            schedules::save(&entry).unwrap();

            let paused = schedules::pause(&entry.id, "upstream moved").unwrap();
            assert!(!paused.enabled);
            assert_eq!(paused.paused_reason.as_deref(), Some("upstream moved"));

            let resumed = schedules::repin(&entry.id, "newsha1111").unwrap();
            assert!(resumed.enabled);
            assert!(resumed.paused_reason.is_none());
            assert_eq!(resumed.resolved_sha.as_deref(), Some("newsha1111"));
        });
    }
}
