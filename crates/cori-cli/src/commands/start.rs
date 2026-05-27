//! `cori start` — the solo-user entrypoint.
//!
//! Boots the local Cori stack in one terminal:
//!
//! 1. Ensures `~/.cori/` exists (idempotent `cori init --local`).
//! 2. Spawns the worker daemon on a dedicated thread. The daemon brings
//!    up Temporal (bundled or external) and fires a [`WorkerReady`]
//!    handshake when the registration sweep is done.
//! 3. Once the worker is ready, optionally starts the embedded HTTP
//!    server / web UI on another thread (unless `--no-ui`).
//! 4. Prints **one** unified "Cori is ready" banner that names every
//!    endpoint, then waits for the worker thread to finish.
//!
//! `ctrlc::set_handler` is installed inside the worker daemon and is
//! process-wide, so Ctrl-C reaches it regardless of which thread runs
//! the daemon.

use std::io::IsTerminal;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc;

use anyhow::{Context, Result, anyhow};
use cori_broker::capabilities;
use cori_worker::{RegisterOutcome, TemporalSource, WorkerConfig, WorkerReady};

use crate::{config::Config, paths, registry};

const DEFAULT_BIND: &str = "127.0.0.1:7510";

pub fn local(bind: Option<String>, insecure: bool, no_ui: bool) -> Result<()> {
    // 1. Make sure `~/.cori/` exists. Silent — verbose `cori init`
    // output would just clutter every `cori start --local`.
    crate::commands::init::ensure(true).context("initialising ~/.cori before starting")?;

    let home = paths::home()?;
    let runbooks_dir = paths::runbooks_dir()?;
    let state_dir = paths::state_dir()?.join("temporal");

    std::fs::create_dir_all(&runbooks_dir)
        .with_context(|| format!("creating runbooks directory `{}`", runbooks_dir.display()))?;

    let cfg = Config::load().ok();
    let temporal_host = cfg
        .as_ref()
        .and_then(|c| c.get("temporal.host"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // Validate the HTTP bind up-front so we fail fast instead of after
    // spawning Temporal and the worker.
    let http_addr: Option<SocketAddr> = if no_ui {
        None
    } else {
        let addr_str = bind.as_deref().unwrap_or(DEFAULT_BIND);
        let addr: SocketAddr = addr_str
            .parse()
            .with_context(|| format!("invalid bind address `{addr_str}`"))?;
        let is_loopback =
            matches!(addr.ip(), IpAddr::V4(Ipv4Addr::LOCALHOST)) || addr.ip().is_loopback();
        if !is_loopback && !insecure {
            anyhow::bail!(
                "refusing to bind to non-loopback address `{addr}` without --insecure.\n\
                 There is no authentication on the Cori HTTP API in v1."
            );
        }
        Some(addr)
    };

    // Capability discovery — enumerate registered workflows' requirements.
    let reg = registry::open()?;
    let workflows = reg.list().unwrap_or_default();
    let mut wanted_clis: Vec<String> = Vec::new();
    let mut wanted_mcp: Vec<String> = Vec::new();
    for row in &workflows {
        if let Ok(Some(detail)) = reg.get(&row.id) {
            for b in &detail.compiled.required_cli_binaries {
                if !wanted_clis.contains(b) {
                    wanted_clis.push(b.clone());
                }
            }
            for s in &detail.compiled.required_mcp_servers {
                if !wanted_mcp.contains(s) {
                    wanted_mcp.push(s.clone());
                }
            }
        }
    }
    drop(reg);
    let creds = crate::commands::run::resolve_llm_credentials().unwrap_or_default();
    let caps = capabilities::discover(&home, &wanted_clis, &creds);

    // 2. Spawn the worker daemon. It owns Temporal supervision + SIGINT.
    let (ready_tx, ready_rx) = mpsc::sync_channel::<WorkerReady>(1);
    let runbooks_for_worker = runbooks_dir.clone();
    let worker_config = WorkerConfig {
        temporal_host,
        temporal_state_dir: state_dir,
        runbooks_dir: runbooks_for_worker,
        register: Arc::new(register_runbook),
        capability_banner: Vec::new(),
        ready_signal: Some(ready_tx),
    };

    eprintln!();
    eprintln!("{}", style().dim("Starting Cori…"));

    let worker_handle = std::thread::Builder::new()
        .name("cori-worker".to_string())
        .spawn(move || cori_worker::run(worker_config))
        .context("spawning cori-worker thread")?;

    // 3. Wait for the worker to come up. If it died first, drain its
    // error instead of blocking forever.
    let ready = match ready_rx.recv() {
        Ok(r) => r,
        Err(_) => {
            return worker_handle
                .join()
                .map_err(|_| anyhow!("worker thread panicked"))?
                .and_then(|_| Err(anyhow!("worker exited before signalling ready")));
        }
    };

    // 4. Start the HTTP server (quiet — banner printed below).
    if let Some(addr) = http_addr {
        let bind_str = addr.to_string();
        std::thread::Builder::new()
            .name("cori-serve".to_string())
            .spawn(move || {
                if let Err(e) = super::serve::start(Some(bind_str), insecure) {
                    eprintln!("serve thread exited: {e}");
                }
            })
            .context("spawning cori-serve thread")?;
    }

    // 5. One unified banner.
    print_ready_banner(&ready, http_addr, &caps, workflows.len());

    // 6. Wait for the worker thread to exit (Ctrl-C → daemon → join).
    let result = worker_handle
        .join()
        .map_err(|_| anyhow!("worker thread panicked"))?;
    eprintln!();
    eprintln!("{}", style().dim("Cori stopped."));
    result
}

fn print_ready_banner(
    ready: &WorkerReady,
    http_addr: Option<SocketAddr>,
    caps: &capabilities::Capabilities,
    workflow_count: usize,
) {
    let s = style();
    let source_label = match ready.temporal_source {
        TemporalSource::Bundled => "bundled",
        TemporalSource::External => "external",
    };
    let mut temporal_line = format!(
        "{} {}",
        s.cyan(&ready.temporal_endpoint),
        s.dim(&format!("({source_label})"))
    );
    if let Some(ui) = &ready.temporal_ui_url {
        temporal_line.push_str(&s.dim(" · UI "));
        temporal_line.push_str(&s.cyan(ui));
    }

    let http_line = match http_addr {
        Some(addr) => s.cyan(&format!("http://{addr}")),
        None => s.dim("(disabled — --no-ui)"),
    };

    let clis = if caps.cli_binaries.is_empty() {
        s.dim("(none)")
    } else {
        caps.cli_binaries
            .keys()
            .cloned()
            .collect::<Vec<_>>()
            .join(", ")
    };
    let mcp = if caps.mcp_servers.is_empty() {
        s.dim("(none configured in ~/.cori/mcp-servers.json)")
    } else {
        caps.mcp_servers
            .keys()
            .cloned()
            .collect::<Vec<_>>()
            .join(", ")
    };
    let llm = if caps.llm_providers.is_empty() {
        s.dim("(none — set OPENAI_API_KEY / ANTHROPIC_API_KEY / GEMINI_API_KEY)")
    } else {
        caps.llm_providers
            .iter()
            .cloned()
            .collect::<Vec<_>>()
            .join(", ")
    };

    eprintln!();
    eprintln!("{}", s.bold_green("Cori is ready."));
    eprintln!("  {} {temporal_line}", s.bold("Temporal:   "));
    eprintln!("  {} {http_line}", s.bold("HTTP API:   "));
    eprintln!(
        "  {} {} {}",
        s.bold("Runbooks:   "),
        ready.runbooks_dir.display(),
        s.dim(&format!(
            "({} workflow{})",
            workflow_count,
            if workflow_count == 1 { "" } else { "s" }
        )),
    );
    eprintln!("  {} {clis}", s.bold("CLIs:       "));
    eprintln!("  {} {mcp}", s.bold("MCP servers:"));
    eprintln!("  {} {llm}", s.bold("LLM:        "));
    eprintln!();
    eprintln!("{}", s.dim("Press Ctrl-C to stop."));
}

/// Tiny ANSI styler. Honours `NO_COLOR` and disables itself when
/// stderr isn't a TTY (e.g. when output is piped to a log file).
struct Style {
    enabled: bool,
}

fn style() -> Style {
    let enabled = std::io::stderr().is_terminal() && std::env::var_os("NO_COLOR").is_none();
    Style { enabled }
}

impl Style {
    fn paint(&self, code: &str, s: &str) -> String {
        if self.enabled {
            format!("\x1b[{code}m{s}\x1b[0m")
        } else {
            s.to_string()
        }
    }
    fn bold(&self, s: &str) -> String {
        self.paint("1", s)
    }
    fn dim(&self, s: &str) -> String {
        self.paint("2", s)
    }
    fn cyan(&self, s: &str) -> String {
        self.paint("36", s)
    }
    fn bold_green(&self, s: &str) -> String {
        self.paint("1;32", s)
    }
}

fn register_runbook(path: &Path) -> Result<RegisterOutcome> {
    let abs = path
        .canonicalize()
        .with_context(|| format!("resolving runbook path `{}`", path.display()))?;
    let compiled = cori_compiler::compile(&abs).map_err(|errors| {
        let summary = errors
            .iter()
            .map(|e| format!("{}: {}", e.file, e.reason))
            .collect::<Vec<_>>()
            .join("; ");
        anyhow::anyhow!("compile failed: {summary}")
    })?;
    let mut reg = registry::open()?;
    let outcome = reg.register(&abs, &compiled)?;
    let workflow_id = compiled.manifest.id.clone();
    let (version, note) = match outcome {
        registry::RegisterOutcome::Created { version } => (version, "created"),
        registry::RegisterOutcome::Updated { version } => (version, "updated"),
        registry::RegisterOutcome::Unchanged { version } => (version, "unchanged"),
    };
    Ok(RegisterOutcome {
        workflow_id,
        version,
        note,
    })
}
