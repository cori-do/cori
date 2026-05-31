//! `cori work [--shared <name>] [--no-console] [--console] [--console-port <p>] [--console-open]`
//!
//! Boots a long-lived Temporal worker bound to the identity-derived
//! task queue (see [`cori_protocol::task_queue_for`]). By default also
//! serves Cori Console on `127.0.0.1:<port>`; the startup banner prints
//! the tokenized URL once. Under `--shared` the Console is off by
//! default (org-infra worker boxes are headless); pass `--console` to
//! enable it there.

use anyhow::{Context, Result, bail};
use chrono::Utc;
use cori_broker::capabilities::{self, CapabilityReport};
use cori_broker::identity::{IdentitySource, OsUser};
use cori_broker::llm::LlmOptions;
use cori_broker::{TriggerContext, runtime as broker_runtime};
use cori_console::{find_available_port, generate_token};
use cori_protocol::{WorkerIdentity, task_queue_for, validate_identity_token};
use cori_worker::broker_ctx::{BrokerCtx, set_broker_ctx};
use cori_worker::runner::serve_worker_until_signal;
use cori_worker::runtime::{CoriTemporalRuntime, DEFAULT_NAMESPACE, preflight_check};
use serde::Serialize;

use crate::commands::run::resolve_llm_credentials;
use cori_run::{config::Config, paths, planner, runtime as cli_runtime, temporal_endpoint};

const CONSOLE_DEFAULT_PORT: u16 = 7878;

pub struct WorkOpts {
    pub shared: Option<String>,
    pub no_console: bool,
    pub force_console: bool,
    pub console_port: Option<u16>,
    pub console_open: bool,
}

pub fn work(opts: WorkOpts) -> Result<()> {
    let identity = resolve_identity(opts.shared.as_deref())?;
    let queue = task_queue_for(&identity);

    print_worker_banner(&identity, &queue);

    cli_runtime::ensure_installed()?;
    let runtime_root = paths::runtime_dir()?;
    let runtime = broker_runtime::Runtime::resolve(&runtime_root).map_err(|e| {
        anyhow::anyhow!(
            "{e}\n\nIf you have Deno installed, you can also point Cori at it with:\n  \
             export CORI_DENO=$(which deno)"
        )
    })?;

    let credentials = resolve_llm_credentials();
    let home = paths::home()?;
    let caps = capabilities::discover(&home, &[], &credentials);

    let llm_opts = LlmOptions {
        credentials,
        trigger: Some(TriggerContext::Cli),
    };

    let cwd = std::env::current_dir().context("reading current working directory")?;
    let broker_ctx = BrokerCtx {
        runtime,
        caps: caps.clone(),
        llm_opts,
        source_root: cwd,
        credentials_dir: paths::credentials_dir()?,
    };
    let _ = set_broker_ctx(broker_ctx);

    let endpoint = temporal_endpoint::resolve()?;
    if let Err(e) = preflight_check(&endpoint.target, std::time::Duration::from_millis(500)) {
        eprintln!("✗ Temporal not reachable at {}", endpoint.target);
        for line in format!("{e:#}").lines() {
            eprintln!("  {line}");
        }
        bail!("Temporal server unavailable");
    }

    let report = CapabilityReport::from_capabilities_with(
        identity.clone(),
        &caps,
        Some(&paths::credentials_dir()?),
    );
    let publish_result = planner::publish_report(&report);
    match &publish_result {
        Ok(p) => tracing::debug!(path = %p.display(), "published capability report"),
        Err(e) => tracing::warn!(error = %format!("{e:#}"), "could not publish capability report"),
    }

    // Decide whether to serve the Console, and on which port.
    let console_cfg = resolve_console_cfg(&opts, &identity)?;
    let console_state_written = console_cfg
        .as_ref()
        .map(|c| write_console_state(c.port).is_ok())
        .unwrap_or(false);
    if let Some(cfg) = &console_cfg {
        print_console_banner(cfg);
        if opts.console_open {
            let _ = webbrowser::open(&cfg.url_with_token());
        }
    }

    let tokio_rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("starting tokio runtime for Temporal worker")?;

    let serve_result = tokio_rt.block_on(async {
        let rt = CoriTemporalRuntime::connect(endpoint.target.clone(), DEFAULT_NAMESPACE, &queue)
            .await?;
        match console_cfg {
            None => serve_worker_until_signal(&rt).await,
            Some(cfg) => {
                tokio::try_join!(
                    serve_worker_until_signal(&rt),
                    cori_console::serve(cfg.port, cfg.token, paths::home()?),
                )
                .map(|_| ())
            }
        }
    });

    if let Err(e) = planner::unpublish_report(&queue) {
        tracing::warn!(error = %format!("{e:#}"), "could not remove capability report");
    }
    if console_state_written
        && let Err(e) = remove_console_state()
    {
        tracing::warn!(error = %format!("{e:#}"), "could not remove ~/.cori/state/console.json");
    }

    serve_result?;

    println!("\nWorker stopped.");
    Ok(())
}

fn resolve_identity(shared: Option<&str>) -> Result<WorkerIdentity> {
    match shared {
        Some(name) => {
            validate_identity_token(name).with_context(|| {
                format!("invalid --shared name `{name}` (lowercase a-z, 0-9, `-`, `_`)")
            })?;
            Ok(WorkerIdentity::service(name)?)
        }
        None => OsUser.resolve().context("resolving OS user identity"),
    }
}

struct ConsoleCfg {
    port: u16,
    token: String,
}

impl ConsoleCfg {
    fn url_with_token(&self) -> String {
        format!("http://127.0.0.1:{}/?t={}", self.port, self.token)
    }
}

fn resolve_console_cfg(opts: &WorkOpts, identity: &WorkerIdentity) -> Result<Option<ConsoleCfg>> {
    // Env: CORI_NO_CONSOLE
    let env_no = std::env::var("CORI_NO_CONSOLE")
        .map(|v| matches!(v.as_str(), "1" | "true" | "yes"))
        .unwrap_or(false);

    // Config: [console] { enabled, port }
    let cfg = Config::load().ok();
    let cfg_enabled = cfg
        .as_ref()
        .and_then(|c| c.get("console.enabled"))
        .and_then(|v| v.as_bool());
    let cfg_port = cfg
        .as_ref()
        .and_then(|c| c.get("console.port"))
        .and_then(|v| v.as_integer())
        .and_then(|v| u16::try_from(v).ok());

    // Decide enabled.
    let is_shared = matches!(identity, WorkerIdentity::Service { .. });
    let enabled = if opts.no_console || env_no {
        false
    } else if is_shared {
        // Default off for service workers; require explicit opt-in.
        opts.force_console || cfg_enabled.unwrap_or(false)
    } else {
        cfg_enabled.unwrap_or(true)
    };

    if !enabled {
        return Ok(None);
    }

    // Port resolution: flag > env > config > default. Then look for a
    // free port (preserves the requested port if available).
    let preferred = opts
        .console_port
        .or_else(|| {
            std::env::var("CORI_CONSOLE_PORT")
                .ok()
                .and_then(|v| v.parse::<u16>().ok())
        })
        .or(cfg_port)
        .unwrap_or(CONSOLE_DEFAULT_PORT);

    let port = find_available_port(preferred)
        .context("finding an available local port for Cori Console")?;

    Ok(Some(ConsoleCfg {
        port,
        token: generate_token(),
    }))
}

#[derive(Serialize)]
struct ConsoleStateFile {
    port: u16,
    started_at: chrono::DateTime<chrono::Utc>,
    pid: u32,
}

fn write_console_state(port: u16) -> Result<()> {
    let path = paths::console_state_file()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating `{}`", parent.display()))?;
    }
    let body = ConsoleStateFile {
        port,
        started_at: Utc::now(),
        pid: std::process::id(),
    };
    let bytes = serde_json::to_vec_pretty(&body).context("serializing console.json")?;
    std::fs::write(&path, bytes).with_context(|| format!("writing `{}`", path.display()))?;
    Ok(())
}

fn remove_console_state() -> Result<()> {
    let path = paths::console_state_file()?;
    if path.exists() {
        std::fs::remove_file(&path)
            .with_context(|| format!("removing `{}`", path.display()))?;
    }
    Ok(())
}

fn print_worker_banner(identity: &WorkerIdentity, queue: &str) {
    match identity {
        WorkerIdentity::Person { user_id } => {
            println!("Running as user '{user_id}'. Task queue: {queue}");
        }
        WorkerIdentity::Service { pool } => {
            println!("⚠ Running as SHARED service '{pool}'.");
            println!(
                "  Capabilities here are usable by ANY authorized user whose run routes here."
            );
            println!("  Task queue: {queue}");
        }
    }
}

fn print_console_banner(cfg: &ConsoleCfg) {
    println!();
    println!("Cori Console: {}", cfg.url_with_token());
    println!("  (open in your browser; the token is shown once and not stored on disk.)");
}
