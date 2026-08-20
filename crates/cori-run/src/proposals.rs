//! Workflow proposals — the reviewable end state of an authoring session.
//!
//! A proposal is the frozen, human-auditable summary of everything one
//! authoring session did to a workflow folder: one row per compiled step
//! (kind, target, effect access, external reach) joined with the net
//! file change the journal attributes to it, plus the non-step files
//! touched and the lint warnings `check` would raise. It is materialized
//! by the agent's `propose` MCP tool and resolved by the human in the
//! Console: accept publishes the next version, reject stops the session
//! (optionally rewinding the folder to its pre-session state).
//!
//! Disk-as-truth, like the session journal it summarizes:
//!
//! ```text
//! ~/.cori/sessions/<session_id>/proposal.json
//! ```
//!
//! Invariants:
//! - A proposal only exists for a folder that compiles; effects come
//!   from the compiled DAG (`cori_compiler::effects`), never from the
//!   agent's own claims.
//! - While a session is `proposed`, every mutation through the session
//!   is refused — the folder the human reviews is the folder that ships.
//! - Accept re-verifies the gates (compile + capability preflight) at
//!   decision time; a folder edited out-of-band since `propose` fails
//!   closed, not open.
//! - Resolution is journalled (`ProposalAccepted` / `ProposalRejected`)
//!   and recorded on the proposal, so the ledger tells the whole story.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use cori_compiler::effects::{EffectAccess, compute_effects};
use cori_protocol::CompiledWorkflow;
use serde::{Deserialize, Serialize};

use crate::sessions::{self, EventKind, Session, SessionState};
use crate::versions;

/// Net effect of the session on one file, replayed from the journal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    Added,
    Modified,
    Renamed,
    Deleted,
    /// Untouched by this session (pre-existing file of an opened workflow).
    Unchanged,
}

/// One file the session touched, with its net change.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileChange {
    pub rel_path: String,
    pub change: ChangeKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub renamed_from: Option<String>,
}

/// One workflow step as the reviewer sees it: compiled identity, proven
/// effect, and what this session did to its source file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProposalStep {
    pub activity_id: String,
    pub index: u32,
    pub name: String,
    pub description: String,
    /// `cli` | `mcp_tool` | `code` | `llm` | `builtin`.
    pub kind: String,
    /// Human-facing effect target: `gh` · `gws · sheets.update` · `AI provider`.
    pub target: String,
    pub access: EffectAccess,
    /// May touch the world beyond this machine's workflow folder.
    pub external: bool,
    pub source_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_sha256: Option<String>,
    pub change: ChangeKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub renamed_from: Option<String>,
}

/// Effect counts across all steps — the one-line consent summary.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EffectsRollup {
    /// Provably pure compute.
    pub pure: usize,
    pub reads: usize,
    pub prompts: usize,
    pub may_writes: usize,
    /// Steps that may reach beyond this machine.
    pub external: usize,
}

/// How the human resolved the proposal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Resolution {
    /// `accepted` | `rejected`.
    pub decision: String,
    /// Which surface decided — `console`, `cli`.
    pub by: String,
    pub at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published_version: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Rejection rewound the folder to its pre-session state.
    #[serde(default)]
    pub discarded_changes: bool,
}

/// The reviewable object, persisted as `proposal.json` in the session dir.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Proposal {
    pub session_id: String,
    pub agent: String,
    pub workflow_dir: PathBuf,
    pub workflow_name: String,
    /// The manifest's description — what the workflow is for.
    pub description: String,
    /// The agent's one-sentence summary of this change.
    pub summary: String,
    /// Manifest version the session opened on; `None` = new workflow.
    pub base_version: Option<u32>,
    pub manifest_version: u32,
    pub proposed_at: DateTime<Utc>,
    pub steps: Vec<ProposalStep>,
    /// Every file the session touched, steps included.
    pub files: Vec<FileChange>,
    pub rollup: EffectsRollup,
    /// The advisory lints `check` raises (compile errors gate `propose`,
    /// so these are always warnings, never errors).
    pub warnings: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution: Option<Resolution>,
}

/// Outcome of an accepted proposal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcceptOutcome {
    pub version: u32,
    pub previous_version: u32,
    /// Schedules that will pick the new version up.
    pub schedules: Vec<String>,
}

fn proposal_path(session_id: &str) -> Result<PathBuf> {
    Ok(sessions::session_dir(session_id)?.join("proposal.json"))
}

fn save_proposal(proposal: &Proposal) -> Result<()> {
    let path = proposal_path(&proposal.session_id)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(proposal)?)
        .with_context(|| format!("writing `{}`", tmp.display()))?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// The stored proposal of a session, if it ever submitted one.
pub fn load(session_id: &str) -> Result<Option<Proposal>> {
    let path = proposal_path(session_id)?;
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(_) => return Ok(None),
    };
    let proposal =
        serde_json::from_slice(&bytes).with_context(|| format!("parsing `{}`", path.display()))?;
    Ok(Some(proposal))
}

/// Replay the journal into net per-file changes — the same fold the
/// Console's live diff renders, computed once and frozen.
fn net_changes(session_id: &str) -> Result<BTreeMap<String, FileChange>> {
    let mut net: BTreeMap<String, FileChange> = BTreeMap::new();
    for ev in sessions::events(session_id)? {
        match ev.kind {
            EventKind::FileWritten => {
                let Some(rel) = ev.rel_path else { continue };
                let change = match net.get(&rel) {
                    Some(prior) if prior.change == ChangeKind::Added => ChangeKind::Added,
                    Some(_) => ChangeKind::Modified,
                    // First sight: created if the write had no prior content.
                    None if ev.before_sha.is_none() => ChangeKind::Added,
                    None => ChangeKind::Modified,
                };
                let renamed_from = net.remove(&rel).and_then(|p| p.renamed_from);
                net.insert(
                    rel.clone(),
                    FileChange {
                        rel_path: rel,
                        change,
                        renamed_from,
                    },
                );
            }
            EventKind::FileDeleted => {
                let Some(rel) = ev.rel_path else { continue };
                // A file created by this session and deleted again is a
                // wash, not a deletion the reviewer must weigh.
                if net.get(&rel).map(|p| p.change) == Some(ChangeKind::Added) {
                    net.remove(&rel);
                } else {
                    net.insert(
                        rel.clone(),
                        FileChange {
                            rel_path: rel,
                            change: ChangeKind::Deleted,
                            renamed_from: None,
                        },
                    );
                }
            }
            EventKind::FileRenamed => {
                let (Some(from), Some(to)) = (ev.rel_path, ev.to_rel_path) else {
                    continue;
                };
                let prior = net.remove(&from);
                let change = match prior.as_ref().map(|p| p.change) {
                    Some(ChangeKind::Added) => ChangeKind::Added,
                    Some(ChangeKind::Modified) => ChangeKind::Modified,
                    _ => ChangeKind::Renamed,
                };
                let renamed_from = prior.and_then(|p| p.renamed_from).or(Some(from));
                net.insert(
                    to.clone(),
                    FileChange {
                        rel_path: to,
                        change,
                        renamed_from,
                    },
                );
            }
            _ => {}
        }
    }
    Ok(net)
}

/// Build the reviewable object from a compiled workflow and the session
/// journal. Pure assembly — no state transition, nothing persisted.
fn build(
    session: &Session,
    compiled: &CompiledWorkflow,
    summary: &str,
    warnings: Vec<String>,
) -> Result<Proposal> {
    let effects = compute_effects(compiled);
    let by_activity: BTreeMap<&str, &cori_compiler::effects::StepEffect> = effects
        .iter()
        .map(|e| (e.activity_id.as_str(), e))
        .collect();
    let net = net_changes(&session.session_id)?;

    let mut rollup = EffectsRollup::default();
    let steps: Vec<ProposalStep> = compiled
        .steps
        .iter()
        .map(|s| {
            let effect = by_activity.get(s.activity_id.as_str());
            let access = effect.map(|e| e.access).unwrap_or(EffectAccess::MayWrite);
            let external = effect.map(|e| e.external).unwrap_or(true);
            match access {
                EffectAccess::None => rollup.pure += 1,
                EffectAccess::Read => rollup.reads += 1,
                EffectAccess::Prompt => rollup.prompts += 1,
                EffectAccess::MayWrite => rollup.may_writes += 1,
            }
            if external {
                rollup.external += 1;
            }
            let file = net.get(&s.source_path);
            ProposalStep {
                activity_id: s.activity_id.clone(),
                index: s.index,
                name: s.name.clone(),
                description: s.description.clone(),
                kind: effect
                    .map(|e| e.kind.clone())
                    .unwrap_or_else(|| format!("{:?}", s.kind).to_lowercase()),
                target: effect.map(|e| e.target.clone()).unwrap_or_default(),
                access,
                external,
                source_path: s.source_path.clone(),
                source_sha256: s.source_sha256.clone(),
                change: file.map(|f| f.change).unwrap_or(ChangeKind::Unchanged),
                renamed_from: file.and_then(|f| f.renamed_from.clone()),
            }
        })
        .collect();

    Ok(Proposal {
        session_id: session.session_id.clone(),
        agent: session.agent.clone(),
        workflow_dir: session.workflow_dir.clone(),
        workflow_name: compiled.manifest.name.clone(),
        description: compiled.manifest.description.clone(),
        summary: summary.to_string(),
        base_version: session.base_version,
        manifest_version: compiled.manifest.version,
        proposed_at: Utc::now(),
        steps,
        files: net.into_values().collect(),
        rollup,
        warnings,
        resolution: None,
    })
}

/// Submit the session's work for human review: freeze the proposal and
/// move the session to `proposed`, refusing further mutations. The
/// caller compiles the folder first — a proposal only exists for a
/// folder that compiles (structured compile errors stay in the MCP
/// layer, where the agent can act on them).
pub fn submit(
    session_id: &str,
    summary: &str,
    compiled: &CompiledWorkflow,
    warnings: Vec<String>,
) -> Result<Proposal> {
    let mut session = sessions::load(session_id)?;
    match session.state {
        SessionState::Writing => {}
        SessionState::Proposed => {
            bail!("session_proposed: this session already has a pending proposal")
        }
        SessionState::Stopped => bail!(
            "session_stopped: {}",
            session
                .stop_reason
                .as_deref()
                .unwrap_or("stopped by the user")
        ),
    }
    let proposal = build(&session, compiled, summary, warnings)?;
    save_proposal(&proposal)?;
    session.state = SessionState::Proposed;
    let mut ev = sessions::event(EventKind::ProposalSubmitted);
    ev.note = Some(summary.to_string());
    sessions::append_event(&mut session, ev)?;
    sessions::save(&session)?;
    Ok(proposal)
}

fn load_proposed(session_id: &str) -> Result<(Session, Proposal)> {
    let session = sessions::load(session_id)?;
    if session.state != SessionState::Proposed {
        bail!(
            "session `{session_id}` has no pending proposal (state: {:?})",
            session.state
        );
    }
    let proposal = load(session_id)?
        .with_context(|| format!("session `{session_id}` lost its proposal.json"))?;
    Ok((session, proposal))
}

/// Exactly-one `version: N` line, bumped. Line-based on purpose: the
/// version line lives in the manifest frontmatter and must be unique —
/// two matches (or zero) is a folder the human should look at.
fn bump_version_line(src: &str, next: u32) -> Result<String> {
    let is_version_line = |line: &str| {
        line.strip_prefix("version:")
            .map(|rest| {
                let t = rest.trim();
                !t.is_empty() && t.chars().all(|c| c.is_ascii_digit())
            })
            .unwrap_or(false)
    };
    let count = src.lines().filter(|l| is_version_line(l)).count();
    if count != 1 {
        bail!("manifest.md must carry exactly one `version: N` line (found {count})");
    }
    let out: Vec<String> = src
        .lines()
        .map(|l| {
            if is_version_line(l) {
                format!("version: {next}")
            } else {
                l.to_string()
            }
        })
        .collect();
    let mut joined = out.join("\n");
    if src.ends_with('\n') {
        joined.push('\n');
    }
    Ok(joined)
}

/// Schedules whose source folder is this workflow — they pick up
/// whatever the folder holds, so acceptance reports them.
fn schedules_watching(dir: &Path) -> Vec<String> {
    crate::schedules::load_all()
        .unwrap_or_default()
        .into_iter()
        .filter(|s| {
            Path::new(&s.source)
                .canonicalize()
                .map(|p| p == dir)
                .unwrap_or(false)
        })
        .map(|s| s.id)
        .collect()
}

/// The human accepts: re-verify the gates, publish the next version
/// (manifest bump journalled through the session, snapshot kept for
/// `revert`), record the resolution, and stop the session.
pub fn accept(session_id: &str, by: &str, notes: Option<&str>) -> Result<AcceptOutcome> {
    let (mut session, mut proposal) = load_proposed(session_id)?;
    let dir = session.workflow_dir.clone();

    // Gate 1 — the folder still compiles. `propose` verified this, but
    // nothing stops out-of-band edits while the proposal sat in review.
    let compiled = cori_compiler::compile(&dir).map_err(|errors| {
        anyhow::anyhow!(
            "the workflow no longer compiles: {}",
            errors
                .iter()
                .map(|e| {
                    let line = e.line.map(|l| format!(" (line {l})")).unwrap_or_default();
                    format!("{}: {}{}", e.file, e.reason, line)
                })
                .collect::<Vec<_>>()
                .join("; ")
        )
    })?;
    drop(compiled);

    // Gate 2 — every declared capability resolves on this machine.
    let pf = crate::preflight(&dir.display().to_string(), false, false)?;
    if !pf.missing_caps.is_empty() {
        bail!(
            "declared capabilities are not ready on this machine: {}",
            pf.missing_caps.join(", ")
        );
    }

    let manifest_rel = "manifest.md";
    let manifest_src = std::fs::read_to_string(dir.join(manifest_rel))?;
    let current_version = match cori_manifest::parse_manifest(&manifest_src) {
        Ok(m) => m.version,
        Err(errors) => bail!(
            "manifest.md does not parse: {}",
            errors
                .iter()
                .map(|e| e.to_string())
                .collect::<Vec<_>>()
                .join("; ")
        ),
    };
    let next_version = current_version + 1;
    let bumped = bump_version_line(&manifest_src, next_version)?;

    // First publish of a pre-existing folder snapshots its pre-bump
    // state so revert always has a floor.
    if !versions::exists(&dir, current_version) {
        versions::snapshot(&dir, current_version)?;
    }

    // Reopen for the one journalled write the bump needs, then publish.
    session.state = SessionState::Writing;
    sessions::save(&session)?;
    sessions::write_file(session_id, manifest_rel, &bumped, None)?;
    let snapshot_dir = versions::snapshot(&dir, next_version)?;

    // Version metadata lives beside the snapshot, never inside it.
    let meta = serde_json::json!({
        "published_at": Utc::now().to_rfc3339(),
        "session_id": session_id,
        "agent": session.agent,
        "approved_by": by,
        "via": "proposal",
        "notes": notes,
    });
    if let Some(parent) = snapshot_dir.parent() {
        let _ = std::fs::write(
            parent.join(format!("v{next_version}.meta.json")),
            serde_json::to_vec_pretty(&meta)?,
        );
    }

    // Journal the decision (write_file above advanced the journal on
    // disk — reload before appending so seq stays consistent).
    let mut session = sessions::load(session_id)?;
    let mut ev = sessions::event(EventKind::ProposalAccepted);
    ev.note = Some(format!("published v{next_version}, accepted by {by}"));
    sessions::append_event(&mut session, ev)?;
    sessions::save(&session)?;

    proposal.resolution = Some(Resolution {
        decision: "accepted".into(),
        by: by.to_string(),
        at: Utc::now(),
        published_version: Some(next_version),
        reason: None,
        discarded_changes: false,
    });
    save_proposal(&proposal)?;

    sessions::stop(
        session_id,
        Some(&format!("proposal accepted — published v{next_version}")),
    )?;

    Ok(AcceptOutcome {
        version: next_version,
        previous_version: current_version,
        schedules: schedules_watching(&dir),
    })
}

/// The human rejects: journal the decision, optionally rewind the
/// folder to its pre-session state (exact, via the journal), record the
/// resolution, and stop the session with the reason for the agent.
pub fn reject(
    session_id: &str,
    by: &str,
    reason: Option<&str>,
    discard_changes: bool,
) -> Result<()> {
    let (mut session, mut proposal) = load_proposed(session_id)?;

    // Reopen so the rejection (and any rewind) can be journalled.
    session.state = SessionState::Writing;
    let mut ev = sessions::event(EventKind::ProposalRejected);
    ev.note = Some(match reason {
        Some(r) => format!("rejected by {by}: {r}"),
        None => format!("rejected by {by}"),
    });
    sessions::append_event(&mut session, ev)?;
    sessions::save(&session)?;

    if discard_changes {
        // Everything after the session's first event is this session's
        // work; rewinding to it restores the pre-session folder.
        let first_seq = sessions::events(session_id)?
            .first()
            .map(|e| e.seq)
            .unwrap_or(0);
        sessions::rewind(session_id, first_seq)?;
    }

    proposal.resolution = Some(Resolution {
        decision: "rejected".into(),
        by: by.to_string(),
        at: Utc::now(),
        published_version: None,
        reason: reason.map(str::to_string),
        discarded_changes: discard_changes,
    });
    save_proposal(&proposal)?;

    sessions::stop(
        session_id,
        Some(&match reason {
            Some(r) => format!("proposal rejected: {r}"),
            None => "proposal rejected".to_string(),
        }),
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST: &str = "---\nid: counter\nname: Counter\ndescription: count things\ncreated: 2026-08-19\nversion: 1\n---\n# Goal\ncount things\n";

    const CODE_STEP: &str = "import { step } from \"@cori-do/sdk\";\nexport default step.code({\n  description: \"Count one thing\",\n  run: () => ({ n: 1 }),\n});\n";

    fn open_session_with_workflow() -> (String, PathBuf) {
        let wf = tempfile::tempdir().expect("workflow dir");
        let wf_path = wf.path().canonicalize().unwrap();
        // Leak the tempdir so the folder outlives this helper; the
        // temp CORI_HOME is cleaned up by with_temp_home regardless.
        std::mem::forget(wf);
        let s = sessions::create("test-agent", &wf_path, None).unwrap();
        let id = s.session_id.clone();
        sessions::write_file(&id, "manifest.md", MANIFEST, None).unwrap();
        sessions::write_file(&id, "steps/01_count.ts", CODE_STEP, None).unwrap();
        (id, wf_path)
    }

    fn submit_now(id: &str, dir: &Path) -> Proposal {
        let compiled = cori_compiler::compile(dir).expect("workflow compiles");
        submit(
            id,
            "Add a counting workflow.",
            &compiled,
            vec!["advisory".into()],
        )
        .unwrap()
    }

    #[test]
    fn propose_freezes_steps_and_refuses_mutations() {
        crate::test_env::with_temp_home(|| {
            let (id, dir) = open_session_with_workflow();
            let p = submit_now(&id, &dir);

            assert_eq!(p.summary, "Add a counting workflow.");
            assert_eq!(p.steps.len(), 1);
            let step = &p.steps[0];
            assert_eq!(step.activity_id, "01_count");
            assert_eq!(step.kind, "code");
            assert_eq!(step.access, EffectAccess::None);
            assert!(!step.external);
            assert_eq!(step.change, ChangeKind::Added);
            assert_eq!(p.rollup.pure, 1);
            assert_eq!(p.rollup.external, 0);
            assert!(p.files.iter().any(|f| f.rel_path == "manifest.md"));
            assert_eq!(p.warnings, vec!["advisory".to_string()]);

            // State moved and is persisted; mutations are refused.
            assert_eq!(sessions::load(&id).unwrap().state, SessionState::Proposed);
            let err = sessions::write_file(&id, "steps/01_count.ts", "x", None).unwrap_err();
            assert!(err.to_string().contains("session_proposed"));

            // A second submit refuses too.
            let compiled = cori_compiler::compile(&dir).unwrap();
            assert!(submit(&id, "again", &compiled, vec![]).is_err());

            // The stored proposal round-trips.
            let loaded = load(&id).unwrap().expect("proposal.json");
            assert_eq!(loaded.steps.len(), 1);
            assert!(loaded.resolution.is_none());
        });
    }

    #[test]
    fn accept_publishes_and_stops() {
        crate::test_env::with_temp_home(|| {
            let (id, dir) = open_session_with_workflow();
            submit_now(&id, &dir);

            let out = accept(&id, "console", Some("looks good")).unwrap();
            assert_eq!(out.version, 2);
            assert_eq!(out.previous_version, 1);

            let manifest = std::fs::read_to_string(dir.join("manifest.md")).unwrap();
            assert!(
                manifest.contains("version: 2"),
                "manifest bumped: {manifest}"
            );
            assert!(versions::exists(&dir, 1), "pre-bump floor kept");
            assert!(versions::exists(&dir, 2), "new version snapshotted");

            let s = sessions::load(&id).unwrap();
            assert_eq!(s.state, SessionState::Stopped);
            assert!(s.stop_reason.unwrap().contains("published v2"));

            let p = load(&id).unwrap().unwrap();
            let r = p.resolution.expect("resolved");
            assert_eq!(r.decision, "accepted");
            assert_eq!(r.published_version, Some(2));

            // Accepting again: no pending proposal.
            assert!(accept(&id, "console", None).is_err());
        });
    }

    #[test]
    fn reject_with_discard_rewinds_and_stops() {
        crate::test_env::with_temp_home(|| {
            let (id, dir) = open_session_with_workflow();
            submit_now(&id, &dir);

            reject(&id, "console", Some("not like this"), true).unwrap();

            assert!(!dir.join("steps/01_count.ts").exists(), "work discarded");
            assert!(!dir.join("manifest.md").exists(), "work discarded");

            let s = sessions::load(&id).unwrap();
            assert_eq!(s.state, SessionState::Stopped);
            assert!(s.stop_reason.unwrap().contains("not like this"));

            let r = load(&id).unwrap().unwrap().resolution.expect("resolved");
            assert_eq!(r.decision, "rejected");
            assert!(r.discarded_changes);
        });
    }

    #[test]
    fn bump_version_line_is_exact() {
        let src = "---\nid: x\nversion: 3\n---\nbody mentions version: 9 inline\n";
        let bumped = bump_version_line(src, 4).unwrap();
        assert!(bumped.contains("version: 4"));
        assert!(bumped.contains("version: 9 inline"), "prose untouched");
        // "version: 9 inline" is not a version line (trailing text), so
        // exactly one line matched. Two literal version lines refuse:
        let dup = "version: 1\nversion: 2\n";
        assert!(bump_version_line(dup, 3).is_err());
        assert!(bump_version_line("no version here\n", 2).is_err());
    }
}
