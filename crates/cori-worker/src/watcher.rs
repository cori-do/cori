//! Filesystem watcher for `~/.cori/runbooks/` (and any other directories
//! the daemon cares about).
//!
//! Wraps the `notify` recommended watcher in a small debouncer so a noisy
//! editor save (which often emits several events per file write) collapses
//! into one re-register call per runbook directory.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use crossbeam_channel::{Receiver, Sender};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tracing::{debug, warn};

/// Default debounce window — most editor "save" bursts complete inside 250ms.
pub const DEFAULT_DEBOUNCE: Duration = Duration::from_millis(250);

/// One coalesced event: "this runbook directory changed; re-register it".
#[derive(Debug, Clone)]
pub struct ChangeEvent {
    /// The runbook root (first-level subdirectory of the watched root).
    pub runbook_dir: PathBuf,
}

/// Spawn a watcher rooted at `root`. Returns a receiver of coalesced events
/// and the watcher handle (drop to stop watching).
pub fn spawn(
    root: &Path,
    debounce: Duration,
) -> Result<(Receiver<ChangeEvent>, RecommendedWatcher)> {
    let (raw_tx, raw_rx) = crossbeam_channel::unbounded::<notify::Result<Event>>();
    let (out_tx, out_rx) = crossbeam_channel::unbounded::<ChangeEvent>();
    let watched_root = root.to_path_buf();

    let mut watcher: RecommendedWatcher = notify::recommended_watcher(move |res| {
        // Send into the raw channel; the debouncer thread translates.
        let _ = raw_tx.send(res);
    })
    .context("creating filesystem watcher")?;

    watcher
        .watch(root, RecursiveMode::Recursive)
        .with_context(|| format!("watching `{}`", root.display()))?;

    std::thread::Builder::new()
        .name("cori-runbook-watcher".into())
        .spawn(move || debounce_loop(raw_rx, out_tx, watched_root, debounce))
        .context("spawning runbook watcher thread")?;

    Ok((out_rx, watcher))
}

/// Coalesce raw notify events into per-runbook-directory `ChangeEvent`s.
fn debounce_loop(
    raw: Receiver<notify::Result<Event>>,
    out: Sender<ChangeEvent>,
    root: PathBuf,
    debounce: Duration,
) {
    let mut pending: HashMap<PathBuf, Instant> = HashMap::new();
    loop {
        // Block until something happens, with a short timeout so we can
        // flush pending events as their debounce window expires.
        let recv_timeout = if pending.is_empty() {
            Duration::from_secs(60)
        } else {
            debounce
        };

        match raw.recv_timeout(recv_timeout) {
            Ok(Ok(ev)) => collect_event(&ev, &root, &mut pending),
            Ok(Err(e)) => warn!(error = %e, "watcher emitted error"),
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => { /* fall through to flush */ }
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => return,
        }

        flush_ready(&mut pending, debounce, &out);
    }
}

fn collect_event(ev: &Event, root: &Path, pending: &mut HashMap<PathBuf, Instant>) {
    if !matches!(
        ev.kind,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
    ) {
        return;
    }
    let now = Instant::now();
    for path in &ev.paths {
        if let Some(rb) = runbook_dir_for(root, path) {
            debug!(path = %path.display(), runbook = %rb.display(), "watcher event");
            pending.insert(rb, now);
        }
    }
}

fn flush_ready(
    pending: &mut HashMap<PathBuf, Instant>,
    debounce: Duration,
    out: &Sender<ChangeEvent>,
) {
    let now = Instant::now();
    let ready: Vec<PathBuf> = pending
        .iter()
        .filter(|(_, t)| now.duration_since(**t) >= debounce)
        .map(|(p, _)| p.clone())
        .collect();
    for p in ready {
        pending.remove(&p);
        if out.send(ChangeEvent { runbook_dir: p }).is_err() {
            return;
        }
    }
}

/// Given the watched root and an event path, return the first-level
/// subdirectory it lives under (the runbook directory), or `None` if the
/// path is not a descendant of `root`.
fn runbook_dir_for(root: &Path, path: &Path) -> Option<PathBuf> {
    let rel = path.strip_prefix(root).ok()?;
    let first = rel.components().next()?;
    Some(root.join(first.as_os_str()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn runbook_dir_for_strips_to_first_component() {
        let root = PathBuf::from("/r");
        assert_eq!(
            runbook_dir_for(&root, Path::new("/r/wf1/steps/01.ts")),
            Some(PathBuf::from("/r/wf1"))
        );
        assert_eq!(runbook_dir_for(&root, Path::new("/other/x")), None);
    }
}
