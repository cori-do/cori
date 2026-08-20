//! MCP authoring sessions — `~/.cori/sessions/`.
//!
//! An authoring session is an append-only journal of file operations one
//! agent performs against one workflow folder (spec:
//! `docs/mcp-authoring-design.md` §1). It exists so the Console can
//! attribute every write to an agent, render a live diff, and undo
//! exactly — and so a bridge that can write can also *delete* (a
//! write-only bridge orphaned a renamed step file in the field, see
//! `docs/field-notes-authoring-frictions.md` friction #3).
//!
//! Disk-as-truth, like `approvals`:
//!
//! ```text
//! ~/.cori/sessions/<session_id>/session.json    — owner, target folder, state, seq
//! ~/.cori/sessions/<session_id>/journal.jsonl   — one event per line, append-only
//! ~/.cori/sessions/<session_id>/blobs/<sha256>  — content snapshots backing rewind
//! ```
//!
//! Invariants:
//! - Every mutation of the workflow folder goes through this module and
//!   is journalled with before/after content hashes, so [`rewind`] is
//!   exact, not git-approximate. Rewind itself journals the inverse
//!   operations it performs — the journal stays a faithful linear
//!   history of what happened on disk, always.
//! - Relative paths are confined to the session's workflow folder: no
//!   absolute paths, no `..`, never `.git/`.
//! - A stopped session refuses every mutation, carrying the stop reason
//!   back to the agent.
//! - Sessions are Cori state, pruned after [`SESSION_TTL_DAYS`]; the
//!   workflow folder itself stays clean.

use std::collections::BTreeSet;
use std::io::Write as _;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::paths;

/// Sessions untouched for this long are pruned (best-effort, on create).
pub const SESSION_TTL_DAYS: i64 = 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    /// Open for mutations.
    Writing,
    /// The agent submitted its work for human review (see [`crate::proposals`]);
    /// mutations are refused until the human accepts or rejects.
    Proposed,
    /// Terminated by the human (or the agent); mutations are refused.
    Stopped,
}

/// One authoring session, persisted to `session.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub session_id: String,
    /// MCP client identity (`clientInfo.name`), for Console attribution.
    pub agent: String,
    /// Absolute path of the workflow folder this session edits.
    pub workflow_dir: PathBuf,
    /// Manifest version the session opened on; `None` = new workflow.
    pub base_version: Option<u32>,
    pub state: SessionState,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// Why the session was stopped (human note), echoed to the agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<String>,
    /// Next journal sequence number to assign.
    pub next_seq: u64,
    /// Human approvals granted to this session and not yet consumed
    /// (`publish` takes its grant; nothing ships on stale consent).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub granted_approvals: Vec<GrantedApproval>,
}

/// One granted `request_approval`, waiting to be consumed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrantedApproval {
    /// The approved action — `publish`, `new_external_effect`, ….
    pub action: String,
    /// Approval-inbox nonce (the audit handle), when the grant came
    /// through the inbox; `None` for elicitation/dialog grants.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nonce: Option<String>,
    /// Which surface granted — `console`, `elicitation`, `dialog`.
    pub by: String,
    pub granted_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    SessionStarted,
    FileWritten,
    FileDeleted,
    FileRenamed,
    /// Marker preceding the journalled inverse operations of a rewind.
    Rewind,
    /// The agent submitted the session's work as a reviewable proposal.
    ProposalSubmitted,
    /// A human accepted the proposal (the note carries the published version).
    ProposalAccepted,
    /// A human rejected the proposal (the note carries their reason).
    ProposalRejected,
    SessionStopped,
    /// A human granted a `request_approval` for this session (the note
    /// carries the action; the grant itself lives on the session and is
    /// consumed by `publish`).
    ApprovalGranted,
}

/// One journal line. File content lives in the blob store, referenced by
/// sha256; `None` means "absent" (created / deleted).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JournalEvent {
    pub seq: u64,
    pub ts: DateTime<Utc>,
    pub kind: EventKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rel_path: Option<String>,
    /// Rename destination.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_rel_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before_sha: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_sha: Option<String>,
    /// Human-facing annotation (stop reason, rewind target).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Outcome of [`write_file`]: written, or refused because the file
/// changed under the agent (optimistic concurrency).
#[derive(Debug)]
pub enum WriteResult {
    Written {
        sha256: String,
        lines: usize,
    },
    Stale {
        /// `None` = the file no longer exists.
        current_sha: Option<String>,
        current_content: Option<String>,
    },
}

// ---------------------------------------------------------------------------
// Store layout
// ---------------------------------------------------------------------------

pub(crate) fn session_dir(session_id: &str) -> Result<PathBuf> {
    // The id lands in a path — accept only ids this module could have
    // minted, so a hostile "session id" can't traverse anywhere.
    let ok = session_id.starts_with("ses_")
        && session_id.len() > 4
        && session_id[4..].chars().all(|c| c.is_ascii_alphanumeric());
    if !ok {
        bail!("invalid session id `{session_id}`");
    }
    Ok(paths::sessions_dir()?.join(session_id))
}

fn journal_path(session_id: &str) -> Result<PathBuf> {
    Ok(session_dir(session_id)?.join("journal.jsonl"))
}

fn sha_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn store_blob(session_id: &str, content: &[u8]) -> Result<String> {
    let sha = sha_hex(content);
    let dir = session_dir(session_id)?.join("blobs");
    std::fs::create_dir_all(&dir).with_context(|| format!("creating `{}`", dir.display()))?;
    let path = dir.join(&sha);
    if !path.exists() {
        let tmp = dir.join(format!(".{sha}.tmp"));
        std::fs::write(&tmp, content).with_context(|| format!("writing `{}`", tmp.display()))?;
        std::fs::rename(&tmp, &path)?;
    }
    Ok(sha)
}

fn read_blob(session_id: &str, sha: &str) -> Result<Vec<u8>> {
    if !sha.chars().all(|c| c.is_ascii_hexdigit()) {
        bail!("invalid blob reference");
    }
    let path = session_dir(session_id)?.join("blobs").join(sha);
    std::fs::read(&path).with_context(|| format!("reading blob `{sha}`"))
}

pub(crate) fn save(session: &Session) -> Result<()> {
    let dir = session_dir(&session.session_id)?;
    std::fs::create_dir_all(&dir)?;
    let tmp = dir.join(".session.json.tmp");
    let path = dir.join("session.json");
    std::fs::write(&tmp, serde_json::to_vec_pretty(session)?)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

pub(crate) fn append_event(session: &mut Session, mut ev: JournalEvent) -> Result<u64> {
    ev.seq = session.next_seq;
    ev.ts = Utc::now();
    session.next_seq += 1;
    session.updated_at = ev.ts;
    let path = journal_path(&session.session_id)?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("opening `{}`", path.display()))?;
    writeln!(f, "{}", serde_json::to_string(&ev)?)?;
    Ok(ev.seq)
}

pub(crate) fn event(kind: EventKind) -> JournalEvent {
    JournalEvent {
        seq: 0,
        ts: Utc::now(),
        kind,
        rel_path: None,
        to_rel_path: None,
        before_sha: None,
        after_sha: None,
        note: None,
    }
}

// ---------------------------------------------------------------------------
// Lifecycle
// ---------------------------------------------------------------------------

pub fn create(agent: &str, workflow_dir: &Path, base_version: Option<u32>) -> Result<Session> {
    prune_expired();
    if !workflow_dir.is_absolute() {
        bail!("workflow_dir must be absolute");
    }
    let now = Utc::now();
    let mut session = Session {
        session_id: format!("ses_{}", uuid::Uuid::new_v4().simple()),
        agent: agent.to_string(),
        workflow_dir: workflow_dir.to_path_buf(),
        base_version,
        state: SessionState::Writing,
        created_at: now,
        updated_at: now,
        stop_reason: None,
        next_seq: 1,
        granted_approvals: Vec::new(),
    };
    let mut ev = event(EventKind::SessionStarted);
    ev.note = Some(format!("agent: {agent}"));
    append_event(&mut session, ev)?;
    save(&session)?;
    Ok(session)
}

pub fn load(session_id: &str) -> Result<Session> {
    let path = session_dir(session_id)?.join("session.json");
    let bytes = std::fs::read(&path).map_err(|_| anyhow!("unknown session `{session_id}`"))?;
    let mut session: Session =
        serde_json::from_slice(&bytes).with_context(|| format!("parsing `{}`", path.display()))?;
    // Crash resilience: the journal is appended before session.json is
    // rewritten, so reconcile the counter from the journal's tail.
    if let Ok(evs) = events(session_id)
        && let Some(last) = evs.last()
        && last.seq >= session.next_seq
    {
        session.next_seq = last.seq + 1;
    }
    Ok(session)
}

/// Every stored session, newest first — the Console's sessions rail.
pub fn list() -> Result<Vec<Session>> {
    let root = paths::sessions_dir()?;
    let mut out = Vec::new();
    if !root.exists() {
        return Ok(out);
    }
    for entry in std::fs::read_dir(&root)? {
        let Ok(entry) = entry else { continue };
        let Some(id) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if let Ok(s) = load(&id) {
            out.push(s);
        }
    }
    out.sort_by_key(|s| std::cmp::Reverse(s.updated_at));
    Ok(out)
}

pub fn events(session_id: &str) -> Result<Vec<JournalEvent>> {
    let path = journal_path(session_id)?;
    let mut out = Vec::new();
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Ok(out);
    };
    for line in text.lines() {
        if let Ok(ev) = serde_json::from_str::<JournalEvent>(line) {
            out.push(ev);
        }
    }
    Ok(out)
}

/// Terminate a session. Idempotent; later mutations return the reason.
pub fn stop(session_id: &str, reason: Option<&str>) -> Result<Session> {
    let mut session = load(session_id)?;
    if session.state == SessionState::Stopped {
        return Ok(session);
    }
    session.state = SessionState::Stopped;
    session.stop_reason = reason.map(str::to_string);
    let mut ev = event(EventKind::SessionStopped);
    ev.note = reason.map(str::to_string);
    append_event(&mut session, ev)?;
    save(&session)?;
    Ok(session)
}

fn ensure_active(session: &Session) -> Result<()> {
    match session.state {
        SessionState::Writing => Ok(()),
        SessionState::Proposed => {
            bail!("session_proposed: this session's work is awaiting human review in the Console")
        }
        SessionState::Stopped => bail!(
            "session_stopped: {}",
            session
                .stop_reason
                .as_deref()
                .unwrap_or("stopped by the user")
        ),
    }
}

/// Record a granted `request_approval` on the session (journalled).
pub fn record_approval(
    session_id: &str,
    action: &str,
    by: &str,
    nonce: Option<&str>,
) -> Result<()> {
    let mut session = load(session_id)?;
    ensure_active(&session)?;
    session.granted_approvals.push(GrantedApproval {
        action: action.to_string(),
        nonce: nonce.map(str::to_string),
        by: by.to_string(),
        granted_at: Utc::now(),
    });
    let mut ev = event(EventKind::ApprovalGranted);
    ev.note = Some(format!("{action} granted by {by}"));
    append_event(&mut session, ev)?;
    save(&session)?;
    Ok(())
}

/// Consume one granted approval for `action`, if any. Single-use: a
/// grant authorizes exactly one consuming operation.
pub fn take_approval(session_id: &str, action: &str) -> Result<Option<GrantedApproval>> {
    let mut session = load(session_id)?;
    ensure_active(&session)?;
    let Some(idx) = session
        .granted_approvals
        .iter()
        .position(|g| g.action == action)
    else {
        return Ok(None);
    };
    let grant = session.granted_approvals.remove(idx);
    save(&session)?;
    Ok(Some(grant))
}

/// Best-effort removal of sessions untouched for [`SESSION_TTL_DAYS`].
pub fn prune_expired() {
    let Ok(root) = paths::sessions_dir() else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(&root) else {
        return;
    };
    let cutoff = Utc::now() - chrono::Duration::days(SESSION_TTL_DAYS);
    for entry in entries.flatten() {
        let Some(id) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if let Ok(s) = load(&id)
            && s.updated_at < cutoff
        {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

// ---------------------------------------------------------------------------
// Path confinement
// ---------------------------------------------------------------------------

/// Resolve a journal-relative path against the session's workflow folder.
/// Purely lexical: rejects absolute paths, `..`, and `.git/`.
pub fn resolve_rel(session: &Session, rel: &str) -> Result<PathBuf> {
    let p = Path::new(rel);
    if p.is_absolute() {
        bail!("`{rel}` is absolute; paths are relative to the workflow folder");
    }
    let mut clean = PathBuf::new();
    for comp in p.components() {
        match comp {
            Component::Normal(c) => clean.push(c),
            Component::CurDir => {}
            _ => bail!("`{rel}` escapes the workflow folder"),
        }
    }
    if clean.as_os_str().is_empty() {
        bail!("empty path");
    }
    if clean.components().next() == Some(Component::Normal(".git".as_ref())) {
        bail!("the workflow's git metadata is not editable through a session");
    }
    Ok(session.workflow_dir.join(clean))
}

// ---------------------------------------------------------------------------
// Journalled file operations
// ---------------------------------------------------------------------------

/// Atomic write inside the workflow folder (tmp + rename; the dotfile
/// tmp name never matches the compiler's `*.ts` sweep).
fn write_atomic(abs: &Path, content: &[u8]) -> Result<()> {
    let parent = abs.parent().context("path has no parent")?;
    std::fs::create_dir_all(parent)?;
    let name = abs.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let tmp = parent.join(format!(".{name}.cori-tmp"));
    std::fs::write(&tmp, content).with_context(|| format!("writing `{}`", tmp.display()))?;
    std::fs::rename(&tmp, abs).with_context(|| format!("renaming to `{}`", abs.display()))?;
    Ok(())
}

fn do_write(session: &mut Session, rel: &str, content: &[u8]) -> Result<String> {
    let abs = resolve_rel(session, rel)?;
    let before = match std::fs::read(&abs) {
        Ok(bytes) => Some(store_blob(&session.session_id, &bytes)?),
        Err(_) => None,
    };
    let after = store_blob(&session.session_id, content)?;
    write_atomic(&abs, content)?;
    let mut ev = event(EventKind::FileWritten);
    ev.rel_path = Some(rel.to_string());
    ev.before_sha = before;
    ev.after_sha = Some(after.clone());
    append_event(session, ev)?;
    Ok(after)
}

fn do_delete(session: &mut Session, rel: &str) -> Result<()> {
    let abs = resolve_rel(session, rel)?;
    let bytes =
        std::fs::read(&abs).with_context(|| format!("no such file `{rel}` in the workflow"))?;
    let before = store_blob(&session.session_id, &bytes)?;
    std::fs::remove_file(&abs).with_context(|| format!("deleting `{rel}`"))?;
    let mut ev = event(EventKind::FileDeleted);
    ev.rel_path = Some(rel.to_string());
    ev.before_sha = Some(before);
    append_event(session, ev)?;
    Ok(())
}

fn do_rename(session: &mut Session, from: &str, to: &str) -> Result<()> {
    let from_abs = resolve_rel(session, from)?;
    let to_abs = resolve_rel(session, to)?;
    let bytes =
        std::fs::read(&from_abs).with_context(|| format!("no such file `{from}` to rename"))?;
    let sha = store_blob(&session.session_id, &bytes)?;
    if to_abs.exists() {
        bail!("rename destination `{to}` already exists");
    }
    if let Some(parent) = to_abs.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::rename(&from_abs, &to_abs).with_context(|| format!("renaming `{from}` to `{to}`"))?;
    let mut ev = event(EventKind::FileRenamed);
    ev.rel_path = Some(from.to_string());
    ev.to_rel_path = Some(to.to_string());
    ev.before_sha = Some(sha.clone());
    ev.after_sha = Some(sha);
    append_event(session, ev)?;
    Ok(())
}

/// Write one file, with optional optimistic concurrency: when
/// `expect_sha` no longer matches the file on disk (the human took the
/// pen, another session wrote), nothing is written and the current
/// state comes back instead.
pub fn write_file(
    session_id: &str,
    rel: &str,
    content: &str,
    expect_sha: Option<&str>,
) -> Result<WriteResult> {
    let mut session = load(session_id)?;
    ensure_active(&session)?;
    if let Some(expected) = expect_sha {
        let abs = resolve_rel(&session, rel)?;
        let current = std::fs::read(&abs).ok();
        let current_sha = current.as_deref().map(sha_hex);
        if current_sha.as_deref() != Some(expected) {
            return Ok(WriteResult::Stale {
                current_sha,
                current_content: current.map(|b| String::from_utf8_lossy(&b).into_owned()),
            });
        }
    }
    let sha256 = do_write(&mut session, rel, content.as_bytes())?;
    save(&session)?;
    Ok(WriteResult::Written {
        sha256,
        lines: content.lines().count(),
    })
}

pub fn delete_file(session_id: &str, rel: &str) -> Result<()> {
    let mut session = load(session_id)?;
    ensure_active(&session)?;
    do_delete(&mut session, rel)?;
    save(&session)?;
    Ok(())
}

/// Apply a batch of renames atomically-enough: every source is moved to
/// a temp name first, then every temp to its destination — so swapping
/// step numbers can never collide mid-batch. On a destination conflict
/// the batch rolls back and nothing is journalled.
pub fn apply_renames(session_id: &str, moves: &[(String, String)]) -> Result<()> {
    let mut session = load(session_id)?;
    ensure_active(&session)?;

    // Validate everything before touching disk.
    let mut planned = Vec::new();
    for (i, (from, to)) in moves.iter().enumerate() {
        let from_abs = resolve_rel(&session, from)?;
        let to_abs = resolve_rel(&session, to)?;
        if !from_abs.is_file() {
            bail!("no such file `{from}` to rename");
        }
        let parent = from_abs.parent().context("no parent")?.to_path_buf();
        let tmp = parent.join(format!(".cori-mv-{i}"));
        planned.push((from.clone(), to.clone(), from_abs, to_abs, tmp));
    }
    let sources: BTreeSet<&String> = moves.iter().map(|(f, _)| f).collect();
    for (_, to, _, to_abs, _) in &planned {
        // A destination may exist only if this same batch vacates it.
        if to_abs.exists() && !sources.contains(to) {
            bail!("rename destination `{to}` already exists and is not moved by this rename");
        }
    }

    // Phase 1: vacate all sources.
    for (_, _, from_abs, _, tmp) in &planned {
        std::fs::rename(from_abs, tmp)
            .with_context(|| format!("staging `{}`", from_abs.display()))?;
    }
    // Phase 2: land all destinations (roll back on conflict).
    for (idx, (_, to, _, to_abs, tmp)) in planned.iter().enumerate() {
        if to_abs.exists() {
            // Best-effort rollback: landed entries return from their
            // destination, staged ones from their temp name.
            for (_, _, from_abs, landed_abs, _) in planned.iter().take(idx) {
                let _ = std::fs::rename(landed_abs, from_abs);
            }
            for (_, _, from_abs, _, staged_tmp) in planned.iter().skip(idx) {
                let _ = std::fs::rename(staged_tmp, from_abs);
            }
            bail!("rename destination `{to}` already exists");
        }
        std::fs::rename(tmp, to_abs).with_context(|| format!("landing `{to}`"))?;
    }

    for (from, to, _, to_abs, _) in &planned {
        let bytes = std::fs::read(to_abs)?;
        let sha = store_blob(&session.session_id, &bytes)?;
        let mut ev = event(EventKind::FileRenamed);
        ev.rel_path = Some(from.clone());
        ev.to_rel_path = Some(to.clone());
        ev.before_sha = Some(sha.clone());
        ev.after_sha = Some(sha);
        append_event(&mut session, ev)?;
    }
    save(&session)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Rewind
// ---------------------------------------------------------------------------

/// Undo every file operation with `seq > to_seq`, newest first. Each
/// inverse is itself journalled (after a `Rewind` marker), so the
/// journal remains a faithful linear history and rewinds compose.
/// Returns the touched rel paths and the new current seq.
pub fn rewind(session_id: &str, to_seq: u64) -> Result<(Vec<String>, u64)> {
    let mut session = load(session_id)?;
    ensure_active(&session)?;
    let all = events(session_id)?;
    let to_undo: Vec<JournalEvent> = all
        .into_iter()
        .filter(|e| {
            e.seq > to_seq
                && matches!(
                    e.kind,
                    EventKind::FileWritten | EventKind::FileDeleted | EventKind::FileRenamed
                )
        })
        .collect();
    if to_undo.is_empty() {
        let seq = session.next_seq - 1;
        return Ok((Vec::new(), seq));
    }

    let mut marker = event(EventKind::Rewind);
    marker.note = Some(format!("rewind to seq {to_seq}"));
    append_event(&mut session, marker)?;

    let mut touched = BTreeSet::new();
    for ev in to_undo.iter().rev() {
        match ev.kind {
            EventKind::FileWritten => {
                let rel = ev.rel_path.clone().context("journal event missing path")?;
                match &ev.before_sha {
                    Some(sha) => {
                        let bytes = read_blob(session_id, sha)?;
                        do_write(&mut session, &rel, &bytes)?;
                    }
                    None => do_delete(&mut session, &rel)?,
                }
                touched.insert(rel);
            }
            EventKind::FileDeleted => {
                let rel = ev.rel_path.clone().context("journal event missing path")?;
                let sha = ev
                    .before_sha
                    .clone()
                    .context("journal event missing blob")?;
                let bytes = read_blob(session_id, &sha)?;
                do_write(&mut session, &rel, &bytes)?;
                touched.insert(rel);
            }
            EventKind::FileRenamed => {
                let from = ev.rel_path.clone().context("journal event missing path")?;
                let to = ev
                    .to_rel_path
                    .clone()
                    .context("journal event missing path")?;
                do_rename(&mut session, &to, &from)?;
                touched.insert(from);
                touched.insert(to);
            }
            _ => {}
        }
    }
    save(&session)?;
    let seq = session.next_seq - 1;
    Ok((touched.into_iter().collect(), seq))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn journal_write_rename_delete_rewind_stop() {
        crate::test_env::with_temp_home(subtests);
    }

    fn subtests() {
        let wf = tempfile::tempdir().expect("workflow dir");
        let wf_path = wf.path().canonicalize().unwrap();

        // Create.
        let s = create("test-agent", &wf_path, None).unwrap();
        assert!(s.session_id.starts_with("ses_"));
        assert_eq!(s.state, SessionState::Writing);
        let id = s.session_id.clone();

        // Write a new file.
        let r = write_file(&id, "steps/01_a.ts", "// a\n", None).unwrap();
        let sha_a = match r {
            WriteResult::Written { sha256, lines } => {
                assert_eq!(lines, 1);
                sha256
            }
            _ => panic!("expected write"),
        };
        assert!(wf_path.join("steps/01_a.ts").is_file());

        // Optimistic concurrency: matching sha writes, stale sha refuses.
        match write_file(&id, "steps/01_a.ts", "// a2\n", Some(&sha_a)).unwrap() {
            WriteResult::Written { .. } => {}
            _ => panic!("matching sha must write"),
        }
        match write_file(&id, "steps/01_a.ts", "// a3\n", Some(&sha_a)).unwrap() {
            WriteResult::Stale {
                current_sha,
                current_content,
            } => {
                assert!(current_sha.is_some());
                assert_eq!(current_content.as_deref(), Some("// a2\n"));
            }
            _ => panic!("stale sha must refuse"),
        }

        // Path confinement.
        assert!(write_file(&id, "../escape.ts", "x", None).is_err());
        assert!(write_file(&id, "/abs.ts", "x", None).is_err());
        assert!(write_file(&id, ".git/config", "x", None).is_err());

        // Rename (batch, swap-safe).
        write_file(&id, "steps/02_b.ts", "// b\n", None).unwrap();
        apply_renames(
            &id,
            &[
                ("steps/01_a.ts".into(), "steps/02_a.ts".into()),
                ("steps/02_b.ts".into(), "steps/01_b.ts".into()),
            ],
        )
        .unwrap();
        assert!(wf_path.join("steps/02_a.ts").is_file());
        assert!(wf_path.join("steps/01_b.ts").is_file());
        assert!(!wf_path.join("steps/01_a.ts").exists());

        // Delete.
        delete_file(&id, "steps/01_b.ts").unwrap();
        assert!(!wf_path.join("steps/01_b.ts").exists());
        assert!(delete_file(&id, "steps/01_b.ts").is_err());

        // Rewind to just after the first write: content back to "// a\n",
        // renames undone, deleted file restored.
        let first_write_seq = events(&id)
            .unwrap()
            .iter()
            .find(|e| e.kind == EventKind::FileWritten)
            .unwrap()
            .seq;
        let (restored, seq) = rewind(&id, first_write_seq).unwrap();
        assert!(!restored.is_empty());
        assert!(seq > first_write_seq);
        assert_eq!(
            std::fs::read_to_string(wf_path.join("steps/01_a.ts")).unwrap(),
            "// a\n"
        );
        assert!(!wf_path.join("steps/02_a.ts").exists());
        assert!(!wf_path.join("steps/02_b.ts").exists());
        assert!(!wf_path.join("steps/01_b.ts").exists());

        // Stop: mutations refused with the reason; stop is idempotent.
        stop(&id, Some("human pressed Stop")).unwrap();
        let err = write_file(&id, "steps/01_a.ts", "x", None).unwrap_err();
        assert!(err.to_string().contains("session_stopped"));
        assert!(err.to_string().contains("human pressed Stop"));
        stop(&id, None).unwrap();

        // Load reconciles; list sees the session.
        let loaded = load(&id).unwrap();
        assert_eq!(loaded.state, SessionState::Stopped);
        assert!(list().unwrap().iter().any(|s| s.session_id == id));
    }
}
