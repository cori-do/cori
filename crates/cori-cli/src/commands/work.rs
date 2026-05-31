//! `cori work [--shared <name>]`
//!
//! Boots a long-lived Temporal worker bound to the identity-derived
//! task queue (see [`cori_protocol::task_queue_for`]).

use anyhow::{Context, Result, bail};
use cori_broker::capabilities::{self, CapabilityReport};
use cori_broker::identity::{IdentitySource, OsUser};
use cori_broker::llm::LlmOptions;
use cori_broker::{TriggerContext, runtime as broker_runtime};
use cori_protocol::{WorkerIdentity, task_queue_for, validate_identity_token};
use cori_worker::broker_ctx::{BrokerCtx, set_broker_ctx};
use cori_worker::runner::serve_worker_until_signal;
use cori_worker::runtime::{CoriTemporalRuntime, DEFAULT_NAMESPACE, preflight_check};

use crate::commands::run::resolve_llm_credentials;
use cori_run::{paths, planner, runtime as cli_runtime, temporal_endpoint};

pub fn work(shared: Option<String>) -> Result<()> {
    let identity = resolve_identity(shared.as_deref())?;
    let queue = task_queue_for(&identity);

    print_banner(&identity, &queue);

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

    let tokio_rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("starting tokio runtime for Temporal worker")?;

    let serve_result = tokio_rt.block_on(async {
        let rt = CoriTemporalRuntime::connect(endpoint.target.clone(), DEFAULT_NAMESPACE, &queue)
            .await?;
        serve_worker_until_signal(&rt).await
    });

    if let Err(e) = planner::unpublish_report(&queue) {
        tracing::warn!(error = %format!("{e:#}"), "could not remove capability report");
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

fn print_banner(identity: &WorkerIdentity, queue: &str) {
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
