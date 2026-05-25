//! The long-running worker daemon.
//!
//! Composes [`crate::temporal`] (process supervision) with
//! [`crate::watcher`] (filesystem coalescing) and a small SIGINT/SIGTERM
//! handler. The actual workflow execution — dispatching Temporal activity
//! tasks to the broker — is layered on top of this scaffolding in a
//! subsequent change; the current focus is the supervision contract +
//! capability banner + hot-reload pipeline.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, Result};
use crossbeam_channel::{select, tick};
use tracing::{error, info, warn};

use crate::temporal::{Source, Supervisor};
use crate::watcher::{self, ChangeEvent};

/// User-supplied callback that re-validates and re-registers a runbook
/// directory. Implemented by the CLI (which owns the registry connection).
pub type RegisterFn = Arc<dyn Fn(&std::path::Path) -> Result<RegisterOutcome> + Send + Sync>;

/// What the registration callback reports back. Kept independent of the
/// CLI's `RegisterOutcome` so the worker crate doesn't depend on the
/// registry implementation.
#[derive(Debug, Clone)]
pub struct RegisterOutcome {
    pub workflow_id: String,
    pub version: u32,
    pub note: &'static str,
}

/// Daemon configuration. Anything that comes from CLI config / paths goes
/// here so this crate stays unaware of `~/.cori/config.toml`.
pub struct WorkerConfig {
    /// `Some("host:port")` from `temporal.host` in CLI config; `None`
    /// means "supervise a bundled Temporal".
    pub temporal_host: Option<String>,
    /// Where the bundled Temporal stores its SQLite DB.
    pub temporal_state_dir: PathBuf,
    /// Directory whose first-level subdirs are runbooks to keep in sync.
    pub runbooks_dir: PathBuf,
    /// Callback invoked once at startup for every existing runbook, then
    /// again whenever a runbook's files change on disk.
    pub register: RegisterFn,
    /// Human-readable lines about discovered capabilities. Printed once at
    /// startup so operators see what's available.
    pub capability_banner: Vec<String>,
}

/// Run the daemon until SIGINT/SIGTERM.
pub fn run(config: WorkerConfig) -> Result<()> {
    let mut supervisor =
        Supervisor::start(config.temporal_host.as_deref(), &config.temporal_state_dir)?;

    print_startup_banner(&supervisor, &config);

    // Initial registration sweep — pick up everything already on disk.
    if config.runbooks_dir.is_dir() {
        for runbook in list_runbook_dirs(&config.runbooks_dir) {
            handle_registration(&config.register, &runbook);
        }
    } else {
        info!(
            dir = %config.runbooks_dir.display(),
            "runbooks directory does not exist yet — it will be created on first `cori workflows register`",
        );
    }

    // Filesystem watcher.
    let (events_rx, _watcher_handle) = if config.runbooks_dir.is_dir() {
        let (rx, w) = watcher::spawn(&config.runbooks_dir, watcher::DEFAULT_DEBOUNCE)
            .context("starting runbooks file watcher")?;
        (Some(rx), Some(w))
    } else {
        (None, None)
    };

    // Signal handling. `ctrlc::set_handler` flips a flag; the main loop
    // polls it via a short-tick channel so shutdown is responsive.
    let shutdown = Arc::new(AtomicBool::new(false));
    {
        let shutdown = shutdown.clone();
        if let Err(e) = ctrlc::set_handler(move || {
            shutdown.store(true, Ordering::SeqCst);
        }) {
            warn!(error = %e, "could not install signal handler — Ctrl-C may not be graceful");
        }
    }
    let tick_rx = tick(Duration::from_millis(200));

    info!("worker is running — press Ctrl-C to stop");

    loop {
        if shutdown.load(Ordering::SeqCst) {
            info!("shutdown signal received — stopping");
            break;
        }
        match &events_rx {
            Some(rx) => {
                select! {
                    recv(rx) -> ev => match ev {
                        Ok(ChangeEvent { runbook_dir }) => {
                            handle_registration(&config.register, &runbook_dir);
                        }
                        Err(_) => {
                            warn!("file watcher channel closed");
                            break;
                        }
                    },
                    recv(tick_rx) -> _ => { /* re-check shutdown flag */ }
                }
            }
            None => {
                // No watcher (runbooks dir absent at startup) — just wait
                // on the tick channel so we still notice Ctrl-C.
                let _ = tick_rx.recv();
            }
        }
    }

    supervisor.shutdown();
    Ok(())
}

fn handle_registration(register: &RegisterFn, runbook_dir: &std::path::Path) {
    if !runbook_dir.is_dir() {
        info!(dir = %runbook_dir.display(), "runbook directory disappeared — skipping");
        return;
    }
    let label = runbook_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("<unknown>");
    match register(runbook_dir) {
        Ok(outcome) => info!(
            workflow = %outcome.workflow_id,
            version = outcome.version,
            note = outcome.note,
            "registered runbook `{label}`",
        ),
        Err(e) => error!(error = %e, "registering runbook `{label}` failed"),
    }
}

fn list_runbook_dirs(root: &std::path::Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(read) = std::fs::read_dir(root) else {
        return out;
    };
    for entry in read.flatten() {
        let p = entry.path();
        if p.is_dir() {
            out.push(p);
        }
    }
    out.sort();
    out
}

fn print_startup_banner(supervisor: &Supervisor, config: &WorkerConfig) {
    let source_label = match supervisor.source {
        Source::Bundled => "bundled (supervised child process)",
        Source::External => "external (pre-existing)",
    };
    eprintln!("Cori worker");
    eprintln!("  Temporal: {} — {}", supervisor.endpoint(), source_label);
    if let Some(ui) = supervisor.ui_url() {
        eprintln!("  Temporal UI: {ui}");
    }
    eprintln!("  Runbooks: {}", config.runbooks_dir.display());
    if config.capability_banner.is_empty() {
        eprintln!("  Capabilities: (none discovered)");
    } else {
        eprintln!("  Capabilities:");
        for line in &config.capability_banner {
            eprintln!("    · {line}");
        }
    }
}
