//! `cori demo`.
//!
//! The canonical "first 60 seconds" walkthrough. Each step prints a
//! `[N/5]` header so the user can follow along, and the whole flow
//! works in a single terminal on a fresh install — `cori demo` boots
//! its own Temporal + worker in the background, runs the embedded
//! `hello_world` workflow against it, then shuts everything down.
//!
//! Why extract `hello_world` to `~/.cori/runbooks/` instead of running
//! from a temp directory? Two reasons:
//! 1. `cori workflows show hello_world` / `cori runs show ...` after
//!    the demo should print something useful; the registry has to keep
//!    pointing at a live source path.
//! 2. By convention, runbooks live at `~/.cori/runbooks/<id>/` — the
//!    demo is a runbook like any other.

use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use serde_json::json;

use cori_worker::{WorkerConfig, WorkerReady};

use crate::{commands, config::Config, embedded, paths, runtime};

pub fn run() -> Result<()> {
    // [1/4] -----------------------------------------------------------
    header(1, "Initialising ~/.cori/", "cori init --local");
    crate::commands::init::ensure(true).context("initialising ~/.cori before running demo")?;
    // Deno runtime is installed silently as part of `init::ensure`.
    let _ = runtime::install().context("installing Deno runtime")?;

    // [2/4] -----------------------------------------------------------
    header(
        2,
        "Registering hello_world workflow",
        "cori workflows register ~/.cori/runbooks/hello_world",
    );
    let runbooks = paths::runbooks_dir()?;
    let dest = runbooks.join("hello_world");
    let count = embedded::extract(embedded::hello_world::HELLO_WORLD_FILES, &dest)
        .context("extracting embedded hello_world runbook")?;
    eprintln!(
        "    {} {count} files to {}",
        dim("extracted"),
        dim(&dest.display().to_string())
    );
    commands::workflows::register(&dest).context("registering hello_world")?;

    // [3/4] -----------------------------------------------------------
    header(3, "Starting Temporal + worker", "cori start --local --no-ui");
    let cfg = Config::load().ok();
    let temporal_host = cfg
        .as_ref()
        .and_then(|c| c.get("temporal.host"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let state_dir = paths::state_dir()?.join("temporal");
    let runbooks_for_worker = runbooks.clone();

    let (ready_tx, ready_rx) = mpsc::sync_channel::<WorkerReady>(1);
    let (shutdown_tx, shutdown_rx) = mpsc::channel::<()>();

    let worker_config = WorkerConfig {
        temporal_host,
        temporal_state_dir: state_dir,
        runbooks_dir: runbooks_for_worker,
        register: Arc::new(commands::start::register_runbook),
        capability_banner: Vec::new(),
        ready_signal: Some(ready_tx),
        shutdown_signal: Some(shutdown_rx),
    };

    let worker_handle = std::thread::Builder::new()
        .name("cori-demo-worker".to_string())
        .spawn(move || cori_worker::run(worker_config))
        .context("spawning cori-demo-worker thread")?;

    let ready = match ready_rx.recv() {
        Ok(r) => r,
        Err(_) => {
            return worker_handle
                .join()
                .map_err(|_| anyhow!("worker thread panicked"))?
                .and_then(|_| Err(anyhow!("worker exited before signalling ready")));
        }
    };
    eprintln!(
        "    {} {} {}",
        dim("Temporal ready at"),
        cyan(&ready.temporal_endpoint),
        dim(&format!(
            "({})",
            match ready.temporal_source {
                cori_worker::TemporalSource::Bundled => "bundled",
                cori_worker::TemporalSource::External => "external",
            }
        ))
    );

    // [4/4] -----------------------------------------------------------
    header(4, "Executing hello_world", "cori run hello_world");
    let run_result =
        commands::run::execute_workflow("hello_world", json!({}), false, true, None)
            .context("running hello_world");

    // Tear down the worker. Best-effort — the demo's exit code reflects
    // the workflow result, not the shutdown.
    let _ = shutdown_tx.send(());
    // The daemon polls shutdown_signal on a 200ms tick, so give it a
    // brief grace period before joining.
    let join_deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !worker_handle.is_finished() && std::time::Instant::now() < join_deadline {
        std::thread::sleep(Duration::from_millis(100));
    }
    if worker_handle.is_finished() {
        if let Ok(Err(e)) = worker_handle.join() {
            eprintln!("    {} {e}", dim("worker shutdown reported:"));
        }
    } else {
        eprintln!(
            "    {}",
            dim("worker did not stop within 5s — leaving it to Ctrl-C")
        );
    }

    run_result?;

    eprintln!();
    eprintln!("{}", bold_green("Demo complete."));
    eprintln!(
        "{} {} to wire up your agent,",
        dim("Next:"),
        cmd("cori skill install --agent claude-code")
    );
    eprintln!(
        "      {} {} to keep the stack running.",
        dim("or"),
        cmd("cori start --local")
    );
    Ok(())
}

fn header(step: u8, label: &str, command: &str) {
    eprintln!();
    eprintln!(
        "{} {}  {}  {}",
        brand(&format!("[{step}/4]")),
        bold(label),
        dim("→"),
        cmd(command)
    );
}

fn ansi_enabled() -> bool {
    use std::io::IsTerminal;
    std::io::stderr().is_terminal() && std::env::var_os("NO_COLOR").is_none()
}

fn paint(code: &str, s: &str) -> String {
    if ansi_enabled() {
        format!("\x1b[{code}m{s}\x1b[0m")
    } else {
        s.to_string()
    }
}

/// `cori …` command rendered inline — bold cyan when colored, backticks otherwise.
fn cmd(s: &str) -> String {
    if ansi_enabled() {
        format!("\x1b[1;36m{s}\x1b[0m")
    } else {
        format!("`{s}`")
    }
}

fn bold(s: &str) -> String {
    paint("1", s)
}
fn dim(s: &str) -> String {
    paint("2", s)
}
fn cyan(s: &str) -> String {
    paint("36", s)
}
fn bold_green(s: &str) -> String {
    paint("1;32", s)
}
fn brand(s: &str) -> String {
    // 256-color purple, matches the splash logo.
    paint("1;38;5;99", s)
}
