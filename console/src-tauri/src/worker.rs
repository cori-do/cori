//! In-process Cori worker. The Temporal SDK's worker future is `!Send`
//! so we cannot use `tauri::async_runtime::spawn` (which requires
//! `Send`). Instead the worker + cron driver live on a dedicated
//! thread that owns a single-threaded tokio runtime — the same pattern
//! `cori work` uses, just with cancellation driven by oneshot channels
//! instead of SIGINT.

use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use cori_broker::capabilities::{self, CapabilityReport};
use cori_broker::identity::{IdentitySource, OsUser};
use cori_broker::llm::LlmOptions;
use cori_broker::{TriggerContext, runtime as broker_runtime};
use cori_protocol::{WorkerIdentity, task_queue_for};
use cori_run::{paths, planner, runtime as cli_runtime};
use cori_worker::broker_ctx::{BrokerCtx, set_broker_ctx};
use cori_worker::runner::serve_worker_until_cancelled;
use cori_worker::runtime::{CoriTemporalRuntime, DEFAULT_NAMESPACE, preflight_check};
use tauri::{AppHandle, Manager};
use tokio::sync::oneshot;
use tracing::{info, warn};

use crate::events::StackStatus;
use crate::state::AppState;
use crate::supervisor::announce;

pub struct WorkerHandles {
    pub worker_stop: oneshot::Sender<()>,
    pub cron_stop: oneshot::Sender<()>,
    #[allow(dead_code)]
    pub identity: WorkerIdentity,
    pub task_queue: String,
}

/// Resolve identity, set up the broker, connect Temporal, and spawn the
/// worker + cron driver on a **dedicated OS thread** (because the
/// Temporal worker future is `!Send`). Returns the two cancellation
/// senders plus the resolved identity.
pub async fn bootstrap(app: AppHandle) -> Result<WorkerHandles> {
    // --- 1. Synchronous setup (Send-safe). -------------------------------
    let identity = tokio::task::spawn_blocking(|| OsUser.resolve())
        .await
        .context("joining identity resolver")?
        .context("resolving OS user identity")?;
    let queue = task_queue_for(&identity);
    info!(?identity, %queue, "worker identity resolved");

    tokio::task::spawn_blocking(cli_runtime::ensure_installed)
        .await
        .context("joining deno bootstrap")??;

    let runtime_root = paths::runtime_dir()?;
    let runtime = broker_runtime::Runtime::resolve(&runtime_root).map_err(|e| {
        anyhow::anyhow!(
            "{e}\n\nIf you have Deno installed, you can also point Cori at it with:\n  \
             export CORI_DENO=$(which deno)"
        )
    })?;

    let credentials = cori_run::resolve_llm_credentials();
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

    // Read the target the supervisor published into AppState.
    // `await_temporal_ready` has already gated us, so this should be
    // Some — bail with a clear error if not (rather than re-probing
    // and potentially landing on the wrong endpoint).
    let target = app
        .try_state::<AppState>()
        .and_then(|s| s.temporal_target.lock().ok().and_then(|g| g.clone()))
        .ok_or_else(|| {
            announce(
                &app,
                StackStatus::Down {
                    reason: "supervisor did not publish a Temporal target".into(),
                },
            );
            anyhow::anyhow!(
                "Temporal target unknown — supervisor task should publish it before \
                 worker bootstrap is allowed to run"
            )
        })?;
    if let Err(e) = preflight_check(&target, Duration::from_millis(500)) {
        announce(
            &app,
            StackStatus::Down {
                reason: format!("temporal not reachable: {e}"),
            },
        );
        anyhow::bail!("Temporal server unavailable at {target}: {e}");
    }

    let report = CapabilityReport::from_capabilities_with(
        identity.clone(),
        &caps,
        Some(&paths::credentials_dir()?),
    );
    if let Err(e) = planner::publish_report(&report) {
        warn!(error = %format!("{e:#}"), "could not publish capability report");
    }

    let (worker_stop_tx, worker_stop_rx) = oneshot::channel::<()>();
    let (cron_stop_tx, cron_stop_rx) = oneshot::channel::<()>();

    // --- 2. Spawn the dedicated worker thread. ---------------------------
    let identity_for_thread = identity.clone();
    let queue_for_thread = queue.clone();
    thread::Builder::new()
        .name("cori-worker".into())
        .spawn(move || {
            // Single-threaded tokio runtime so the !Send worker future is happy.
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    warn!(error = %e, "could not build worker thread runtime");
                    return;
                }
            };

            rt.block_on(async move {
                let temporal_rt = match CoriTemporalRuntime::connect(
                    target.clone(),
                    DEFAULT_NAMESPACE,
                    &queue_for_thread,
                )
                .await
                {
                    Ok(r) => r,
                    Err(e) => {
                        warn!(error = %format!("{e:#}"), "could not connect Cori Temporal runtime");
                        return;
                    }
                };

                let worker_fut = serve_worker_until_cancelled(&temporal_rt, async move {
                    let _ = worker_stop_rx.await;
                });

                let cron_fut = cori_run::cron_driver::run(identity_for_thread, async move {
                    let _ = cron_stop_rx.await;
                });

                let _ = tokio::join!(
                    async {
                        if let Err(e) = worker_fut.await {
                            warn!(error = %format!("{e:#}"), "worker exited with error");
                        }
                    },
                    cron_fut,
                );
            });
        })
        .context("spawning cori worker thread")?;

    Ok(WorkerHandles {
        worker_stop: worker_stop_tx,
        cron_stop: cron_stop_tx,
        identity,
        task_queue: queue,
    })
}

/// Best-effort unpublish on shutdown.
pub fn unpublish(queue: &str) {
    if let Err(e) = planner::unpublish_report(queue) {
        warn!(error = %format!("{e:#}"), "could not remove capability report");
    }
}
