//! Local human-in-the-loop approval inbox — `~/.cori/approvals/`.
//!
//! One primitive serves every human gate in the product: MCP per-run
//! confirms, first-run trust consent, schedule re-consent after an
//! upstream sha change, and (later) step-level approval gates. Design
//! note: `cori/docs/approvals-design.md`.
//!
//! Disk-as-truth, like the rest of `~/.cori`:
//!
//! ```text
//! ~/.cori/approvals/pending/<nonce>.json   — one file per open decision
//! ~/.cori/approvals/decided/<nonce>.json   — the human's answer
//! ```
//!
//! A requester (`cori mcp`, the cron driver, a worker) writes a pending
//! item and polls for the decision; the Cori desktop app watches
//! `pending/` and is the only writer of decisions. Security invariants:
//! nonces are single-use and unguessable; expiry and every failure mode
//! decline (fail closed); the nonce carries no authority — anything that
//! merely *opens* a UI must re-read the pending item from disk.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use crate::paths;

/// What kind of human decision is being requested.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalKind {
    /// Per-run confirmation (MCP `run` tool).
    RunConfirm,
    /// First-run trust consent for an untrusted remote ref.
    TrustConsent,
    /// A schedule's pinned sha changed upstream; re-consent to resume.
    ScheduleReconsent,
    /// A workflow step marked `approval: required` is waiting.
    StepGate,
    /// A run failed because a capability lost its authentication —
    /// an *action item* ("sign in again, then retry"), not a yes/no
    /// question. "Declined" means dismissed.
    ReauthRequired,
}

/// A pending approval item, persisted to `pending/<nonce>.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalRequest {
    pub nonce: String,
    pub kind: ApprovalKind,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    /// Which surface asked — `"mcp"`, `"schedule"`, `"worker"`.
    pub requested_by: String,
    /// One-line human question, safe to render as text.
    pub message: String,
    /// Structured details for a rich decision UI (source, sha, params,
    /// step, cost estimate…). The UI must treat this as *display input*
    /// and show the compiled reality (pinned sha), never agent prose.
    pub payload: JsonValue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Approved,
    Declined,
}

/// The human's answer, persisted to `decided/<nonce>.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalDecision {
    pub nonce: String,
    pub decision: Decision,
    pub decided_at: DateTime<Utc>,
    /// Which surface decided — `"console"`, `"dialog"`.
    pub via: String,
}

pub fn pending_dir() -> Result<PathBuf> {
    Ok(paths::approvals_dir()?.join("pending"))
}

pub fn decided_dir() -> Result<PathBuf> {
    Ok(paths::approvals_dir()?.join("decided"))
}

/// Write a new pending approval. Returns the persisted request; the
/// caller then polls [`wait_decision`].
pub fn submit(
    kind: ApprovalKind,
    requested_by: &str,
    message: &str,
    payload: JsonValue,
    ttl: Duration,
) -> Result<ApprovalRequest> {
    let now = Utc::now();
    let req = ApprovalRequest {
        nonce: format!("ap_{}", uuid::Uuid::new_v4().simple()),
        kind,
        created_at: now,
        expires_at: now + chrono::Duration::from_std(ttl).unwrap_or(chrono::Duration::minutes(5)),
        requested_by: requested_by.to_string(),
        message: message.to_string(),
        payload,
    };
    let dir = pending_dir()?;
    std::fs::create_dir_all(&dir).with_context(|| format!("creating `{}`", dir.display()))?;
    // Atomic write: tmp + rename, so the watcher never sees a torn file.
    let tmp = dir.join(format!(".{}.tmp", req.nonce));
    let path = dir.join(format!("{}.json", req.nonce));
    std::fs::write(&tmp, serde_json::to_vec_pretty(&req)?)
        .with_context(|| format!("writing `{}`", tmp.display()))?;
    std::fs::rename(&tmp, &path).with_context(|| format!("renaming to `{}`", path.display()))?;
    Ok(req)
}

/// All currently pending, non-expired items (expired ones are removed).
pub fn list_pending() -> Result<Vec<ApprovalRequest>> {
    let dir = pending_dir()?;
    let mut out = Vec::new();
    if !dir.exists() {
        return Ok(out);
    }
    let now = Utc::now();
    for entry in std::fs::read_dir(&dir).with_context(|| format!("reading `{}`", dir.display()))? {
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let Ok(req) = serde_json::from_slice::<ApprovalRequest>(&bytes) else {
            continue;
        };
        if req.expires_at <= now {
            let _ = std::fs::remove_file(&path); // expiry = fail closed
            continue;
        }
        out.push(req);
    }
    out.sort_by_key(|r| r.created_at);
    Ok(out)
}

/// Record the human's decision and retire the pending item. This is the
/// only authority transfer in the system.
pub fn decide(nonce: &str, decision: Decision, via: &str) -> Result<ApprovalDecision> {
    let pending = pending_dir()?.join(format!("{nonce}.json"));
    anyhow::ensure!(pending.exists(), "no pending approval `{nonce}`");
    let dir = decided_dir()?;
    std::fs::create_dir_all(&dir).with_context(|| format!("creating `{}`", dir.display()))?;
    let dec = ApprovalDecision {
        nonce: nonce.to_string(),
        decision,
        decided_at: Utc::now(),
        via: via.to_string(),
    };
    let tmp = dir.join(format!(".{nonce}.tmp"));
    let path = dir.join(format!("{nonce}.json"));
    std::fs::write(&tmp, serde_json::to_vec_pretty(&dec)?)?;
    std::fs::rename(&tmp, &path)?;
    let _ = std::fs::remove_file(&pending);
    Ok(dec)
}

/// Most recent decisions, newest first — the inbox's history view.
pub fn list_decided(limit: usize) -> Result<Vec<ApprovalDecision>> {
    let dir = decided_dir()?;
    let mut out = Vec::new();
    if !dir.exists() {
        return Ok(out);
    }
    for entry in std::fs::read_dir(&dir).with_context(|| format!("reading `{}`", dir.display()))? {
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        if let Ok(bytes) = std::fs::read(&path)
            && let Ok(dec) = serde_json::from_slice::<ApprovalDecision>(&bytes)
        {
            out.push(dec);
        }
    }
    out.sort_by_key(|d| std::cmp::Reverse(d.decided_at));
    out.truncate(limit);
    Ok(out)
}

/// Withdraw a pending item (requester gave up / timed out).
pub fn cancel(nonce: &str) -> Result<()> {
    let _ = std::fs::remove_file(pending_dir()?.join(format!("{nonce}.json")));
    Ok(())
}

/// Poll until the item is decided, expires, or `timeout` elapses.
/// `None` means no decision was made — callers must treat it as a
/// decline. The pending file is cleaned up on the way out.
pub fn wait_decision(nonce: &str, timeout: Duration) -> Result<Option<ApprovalDecision>> {
    let decided_path = decided_dir()?.join(format!("{nonce}.json"));
    let deadline = Instant::now() + timeout;
    loop {
        if decided_path.exists()
            && let Ok(bytes) = std::fs::read(&decided_path)
            && let Ok(dec) = serde_json::from_slice::<ApprovalDecision>(&bytes)
        {
            return Ok(Some(dec));
        }
        if Instant::now() >= deadline {
            cancel(nonce)?;
            return Ok(None);
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

// ---------------------------------------------------------------------------
// Console liveness
// ---------------------------------------------------------------------------

/// `~/.cori/state/console.heartbeat` — touched by the desktop app every
/// [`HEARTBEAT_INTERVAL`]. Requesters treat the Console as alive when the
/// file is fresher than [`HEARTBEAT_STALE`].
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(20);
pub const HEARTBEAT_STALE: Duration = Duration::from_secs(60);

pub fn heartbeat_file() -> Result<PathBuf> {
    Ok(paths::state_dir()?.join("console.heartbeat"))
}

/// Touch the heartbeat (Console side).
pub fn beat_heartbeat() -> Result<()> {
    let path = heartbeat_file()?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, Utc::now().to_rfc3339())?;
    Ok(())
}

/// Is the desktop app around to surface approvals? (Requester side.)
pub fn console_alive() -> bool {
    let Ok(path) = heartbeat_file() else {
        return false;
    };
    let Ok(meta) = std::fs::metadata(&path) else {
        return false;
    };
    let Ok(modified) = meta.modified() else {
        return false;
    };
    modified
        .elapsed()
        .map(|age| age < HEARTBEAT_STALE)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn submit_list_decide_wait_and_expiry() {
        crate::test_env::with_temp_home(subtests);
    }

    fn subtests() {
        // Submit → list.
        let req = submit(
            ApprovalKind::RunConfirm,
            "mcp",
            "Run workflow X?",
            json!({ "source": "examples/x" }),
            Duration::from_secs(60),
        )
        .unwrap();
        assert!(req.nonce.starts_with("ap_"));
        let pending = list_pending().unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].nonce, req.nonce);

        // Decide → pending retired, wait_decision sees it.
        decide(&req.nonce, Decision::Approved, "console").unwrap();
        assert!(list_pending().unwrap().is_empty());
        let dec = wait_decision(&req.nonce, Duration::from_secs(2))
            .unwrap()
            .expect("decision exists");
        assert_eq!(dec.decision, Decision::Approved);
        assert_eq!(dec.via, "console");

        // Deciding twice fails (single-use nonce).
        assert!(decide(&req.nonce, Decision::Approved, "console").is_err());

        // Expired items vanish from list_pending (fail closed).
        let expired = submit(
            ApprovalKind::TrustConsent,
            "mcp",
            "Trust?",
            json!({}),
            Duration::from_secs(0),
        )
        .unwrap();
        std::thread::sleep(Duration::from_millis(10));
        assert!(list_pending().unwrap().is_empty());
        assert!(
            wait_decision(&expired.nonce, Duration::from_millis(300))
                .unwrap()
                .is_none(),
            "no decision on an expired item"
        );

        // wait_decision timeout cancels the pending item.
        let hung = submit(
            ApprovalKind::RunConfirm,
            "mcp",
            "Nobody home",
            json!({}),
            Duration::from_secs(60),
        )
        .unwrap();
        assert!(
            wait_decision(&hung.nonce, Duration::from_millis(300))
                .unwrap()
                .is_none()
        );
        assert!(
            list_pending().unwrap().is_empty(),
            "timed-out item cancelled"
        );

        // Heartbeat: absent → dead; beaten → alive.
        assert!(!console_alive());
        beat_heartbeat().unwrap();
        assert!(console_alive());
    }
}
