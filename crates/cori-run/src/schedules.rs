//! Schedule store at `~/.cori/schedules/<id>.json`.
//!
//! Each entry is plain JSON, mirroring the `~/.cori/cluster/` pattern.
//! No SQLite, no index — the store is small and the disk scan is
//! cheap. The `id` is `sha256(source + schedule)[..12]`, which gives
//! us a stable identifier without depending on user-supplied names.
//!
//! Identity gating: the `identity` field records the task queue this
//! schedule belongs to (e.g. `cori.user.jean` / `cori.service.notion-pool`).
//! Only a `cori work` instance with a matching identity fires the
//! schedule; the Console refuses cross-identity mutations.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::paths;

/// One entry in the schedule store.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduleEntry {
    /// Stable id — `sha256(source + schedule)[..12]`.
    pub id: String,
    /// Workflow source — a local path or remote git ref.
    pub source: String,
    /// SHA of a remote workflow at register time, if applicable.
    /// Recorded for audit; does **not** pin the firing version
    /// (re-resolved on each fire so the schedule tracks `@v1` etc.).
    #[serde(default)]
    pub resolved_sha: Option<String>,
    /// 5- or 6-field POSIX cron expression.
    pub schedule: String,
    /// IANA timezone for the cron expression. `None` means UTC.
    #[serde(default)]
    pub schedule_tz: Option<String>,
    /// Task queue this schedule fires on — defines ownership.
    pub identity: String,
    /// Whether the cron driver should fire this entry.
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    /// When the cron driver last evaluated this entry (and possibly fired).
    #[serde(default)]
    pub last_reconciled_at: Option<DateTime<Utc>>,
    /// When this schedule last fired a workflow run.
    #[serde(default)]
    pub last_fire_at: Option<DateTime<Utc>>,
    /// `"succeeded" | "failed"` of the most recent fired run.
    #[serde(default)]
    pub last_status: Option<String>,
    /// Error from the most recent fire (run_workflow returned Err).
    #[serde(default)]
    pub last_error: Option<String>,
}

/// Derive the stable id used for the on-disk filename.
pub fn schedule_id(source: &str, schedule: &str) -> String {
    let mut h = Sha256::new();
    h.update(source.as_bytes());
    h.update(b"\n");
    h.update(schedule.as_bytes());
    let digest = h.finalize();
    hex::encode(&digest[..6])
}

/// `~/.cori/schedules/<id>.json`.
pub fn entry_path(id: &str) -> Result<PathBuf> {
    Ok(paths::schedules_dir()?.join(format!("{id}.json")))
}

pub fn load(id: &str) -> Result<Option<ScheduleEntry>> {
    let path = entry_path(id)?;
    if !path.is_file() {
        return Ok(None);
    }
    let bytes = std::fs::read(&path).with_context(|| format!("reading `{}`", path.display()))?;
    let entry: ScheduleEntry =
        serde_json::from_slice(&bytes).with_context(|| format!("parsing `{}`", path.display()))?;
    Ok(Some(entry))
}

pub fn load_all() -> Result<Vec<ScheduleEntry>> {
    let dir = paths::schedules_dir()?;
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for e in std::fs::read_dir(&dir)
        .with_context(|| format!("reading `{}`", dir.display()))?
        .flatten()
    {
        let path = e.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        if let Ok(entry) = serde_json::from_slice::<ScheduleEntry>(&bytes) {
            out.push(entry);
        }
    }
    out.sort_by_key(|e| e.created_at);
    Ok(out)
}

/// Load every enabled entry whose `identity` matches `task_queue`.
pub fn for_identity(task_queue: &str) -> Result<Vec<ScheduleEntry>> {
    Ok(load_all()?
        .into_iter()
        .filter(|e| e.identity == task_queue)
        .collect())
}

pub fn save(entry: &ScheduleEntry) -> Result<()> {
    let path = entry_path(&entry.id)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating `{}`", parent.display()))?;
    }
    let bytes = serde_json::to_vec_pretty(entry).context("serializing schedule entry")?;
    // Atomic write: tempfile + rename.
    let tmp = path.with_extension("partial");
    std::fs::write(&tmp, &bytes).with_context(|| format!("writing `{}`", tmp.display()))?;
    std::fs::rename(&tmp, &path).with_context(|| format!("renaming into `{}`", path.display()))?;
    Ok(())
}

pub fn delete(id: &str) -> Result<()> {
    let path = entry_path(id)?;
    if path.exists() {
        std::fs::remove_file(&path).with_context(|| format!("removing `{}`", path.display()))?;
    }
    Ok(())
}

/// Validate a cron expression. Returns `Ok` if `cron::Schedule` accepts
/// it; bubbles up the parser error otherwise.
pub fn validate_cron(expr: &str) -> Result<()> {
    expr.parse::<cron::Schedule>()
        .map(|_| ())
        .map_err(|e| anyhow::anyhow!("invalid cron expression `{expr}`: {e}"))?;
    Ok(())
}

/// Compute the next fire time for an entry in UTC. Returns `None`
/// if the cron parses but has no upcoming fires, or if it doesn't parse.
pub fn next_fire(entry: &ScheduleEntry) -> Option<DateTime<Utc>> {
    let schedule: cron::Schedule = entry.schedule.parse().ok()?;
    schedule.upcoming(Utc).next()
}

/// Construct a new entry with `id` derived from `source + schedule`.
/// Fails if the cron expression is invalid (matches the manifest
/// parser's behaviour — see `cori-manifest/src/lib.rs`).
pub fn new_entry(
    source: String,
    schedule: String,
    schedule_tz: Option<String>,
    identity: String,
    resolved_sha: Option<String>,
) -> Result<ScheduleEntry> {
    validate_cron(&schedule)?;
    if let Some(tz) = schedule_tz.as_deref()
        && tz.parse::<chrono_tz::Tz>().is_err()
    {
        bail!("invalid IANA timezone `{tz}`");
    }
    let id = schedule_id(&source, &schedule);
    Ok(ScheduleEntry {
        id,
        source,
        resolved_sha,
        schedule,
        schedule_tz,
        identity,
        enabled: true,
        created_at: Utc::now(),
        last_reconciled_at: None,
        last_fire_at: None,
        last_status: None,
        last_error: None,
    })
}

/// Set the `enabled` flag on an existing entry and persist.
/// Returns the updated entry.
pub fn set_enabled(id: &str, enabled: bool) -> Result<ScheduleEntry> {
    let mut entry = load(id)?.ok_or_else(|| anyhow::anyhow!("no schedule `{id}`"))?;
    entry.enabled = enabled;
    save(&entry)?;
    Ok(entry)
}

/// Update fire metadata after the cron driver has run a fire.
pub fn record_fire(id: &str, status: &str, error: Option<&str>, at: DateTime<Utc>) -> Result<()> {
    let mut entry = load(id)?.ok_or_else(|| anyhow::anyhow!("no schedule `{id}`"))?;
    entry.last_fire_at = Some(at);
    entry.last_status = Some(status.to_string());
    entry.last_error = error.map(|s| s.to_string());
    entry.last_reconciled_at = Some(at);
    save(&entry)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Tests touch the process-wide `CORI_HOME` env var — serialised
    // crate-wide via the shared helper (a module-local lock would still
    // race the other modules' env-touching tests).
    use crate::test_env::with_temp_home;

    #[test]
    fn id_is_stable_per_source_and_schedule() {
        let a = schedule_id("./x", "0 9 * * *");
        let b = schedule_id("./x", "0 9 * * *");
        let c = schedule_id("./x", "0 10 * * *");
        let d = schedule_id("./y", "0 9 * * *");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_ne!(a, d);
        assert_eq!(a.len(), 12);
    }

    #[test]
    fn invalid_cron_rejected_at_construction() {
        with_temp_home(|| {
            let r = new_entry(
                "./x".into(),
                "not a cron".into(),
                None,
                "cori.user.t".into(),
                None,
            );
            assert!(r.is_err());
        });
    }

    #[test]
    fn invalid_tz_rejected() {
        with_temp_home(|| {
            let r = new_entry(
                "./x".into(),
                "0 9 * * * *".into(),
                Some("Mars/Olympus".into()),
                "cori.user.t".into(),
                None,
            );
            assert!(r.is_err());
        });
    }

    #[test]
    fn save_load_roundtrip_and_identity_filter() {
        with_temp_home(|| {
            let alice = new_entry(
                "./alice".into(),
                "0 9 * * * *".into(),
                None,
                "cori.user.alice".into(),
                None,
            )
            .unwrap();
            let bob = new_entry(
                "./bob".into(),
                "0 10 * * * *".into(),
                None,
                "cori.user.bob".into(),
                None,
            )
            .unwrap();
            save(&alice).unwrap();
            save(&bob).unwrap();

            let all = load_all().unwrap();
            assert_eq!(all.len(), 2);

            let just_alice = for_identity("cori.user.alice").unwrap();
            assert_eq!(just_alice.len(), 1);
            assert_eq!(just_alice[0].source, "./alice");

            let toggled = set_enabled(&alice.id, false).unwrap();
            assert!(!toggled.enabled);
            let reloaded = load(&alice.id).unwrap().unwrap();
            assert!(!reloaded.enabled);

            delete(&bob.id).unwrap();
            assert_eq!(load_all().unwrap().len(), 1);
        });
    }
}
