//! Temporal sidecar supervisor.
//!
//! Lifecycle rules (user-specified):
//!   * If env or `~/.cori/config.toml` configures a Temporal target,
//!     respect it as external — do not spawn, do not kill on Quit.
//!   * Else, probe `127.0.0.1:7233` with a real Temporal gRPC handshake.
//!       - If it is Temporal: use it as external. Do not spawn or kill.
//!       - If it is not Temporal: choose an OS-assigned free port and
//!         spawn our sidecar there.
//!       - If 7233 is free: spawn our sidecar on 7233.
//!   * We only ever kill the child we spawned ourselves. Force-quit
//!     orphans are tolerated (the next launch will either adopt them
//!     as external if they re-bind 7233, or ignore them).

use std::time::Duration;

use anyhow::Result;
use cori_run::paths;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_shell::ShellExt;
use tauri_plugin_shell::process::{CommandChild, CommandEvent};
use tokio::sync::oneshot;
use tracing::{debug, info, warn};

use crate::events::{EVENT_STACK_STATUS, StackStatus};
use crate::state::AppState;
use crate::temporal;

const DEFAULT_PORT: u16 = 7233;
const BACKOFF_SCHEDULE: &[u64] = &[1, 2, 4, 8, 16];

/// Set the in-memory snapshot and broadcast on `stack:status`.
pub fn announce(app: &AppHandle, status: StackStatus) {
    if let Some(state) = app.try_state::<AppState>()
        && let Ok(mut snap) = state.stack_status.lock()
    {
        *snap = status.clone();
    }
    let _ = app.emit(EVENT_STACK_STATUS, status);
}

fn publish_target(app: &AppHandle, target: &str) {
    if let Some(state) = app.try_state::<AppState>()
        && let Ok(mut slot) = state.temporal_target.lock()
    {
        *slot = Some(target.to_string());
    }
}

/// Spawn the supervisor task. Returns a oneshot Sender — fire it to
/// initiate clean teardown.
pub fn spawn(app: AppHandle) -> oneshot::Sender<()> {
    let (stop_tx, mut stop_rx) = oneshot::channel::<()>();

    tauri::async_runtime::spawn(async move {
        // Step 1: env/config-configured target → external, never spawn.
        if let Some(target) = temporal::external_target_configured() {
            info!(%target, "external Temporal target configured; will not spawn sidecar");
            publish_target(&app, &target);
            announce(&app, StackStatus::Up);
            // We don't own this; just wait for the app to quit and exit.
            let _ = stop_rx.await;
            announce(
                &app,
                StackStatus::Down {
                    reason: "app exiting".into(),
                },
            );
            return;
        }

        // Step 2: is something already serving Temporal on 7233?
        let default_target = format!("http://127.0.0.1:{DEFAULT_PORT}");
        if temporal::is_temporal_listener(&default_target).await {
            info!(%default_target, "existing Temporal detected on 7233; using as external");
            publish_target(&app, &default_target);
            announce(&app, StackStatus::Up);
            let _ = stop_rx.await;
            announce(
                &app,
                StackStatus::Down {
                    reason: "app exiting".into(),
                },
            );
            return;
        }

        // Step 3: pick a port we can bind. Prefer the well-known 7233
        // so external tooling that defaults to it can find us; fall
        // back to an OS-assigned port if 7233 is occupied by something
        // non-Temporal.
        let chosen_port = if temporal::can_bind_port(DEFAULT_PORT) {
            DEFAULT_PORT
        } else {
            match temporal::find_free_local_port() {
                Ok(p) => {
                    info!(
                        port = p,
                        "127.0.0.1:7233 is held by a non-Temporal process; spawning on dynamic port"
                    );
                    p
                }
                Err(e) => {
                    warn!(error = %e, "could not find a free local port for Temporal");
                    announce(
                        &app,
                        StackStatus::Down {
                            reason: format!("no free local port: {e}"),
                        },
                    );
                    let _ = stop_rx.await;
                    return;
                }
            }
        };

        // Step 4: supervised spawn + restart-on-exit loop, ending on
        // stop_rx (tray Quit). Only this branch owns a child we can
        // legitimately kill.
        let mut backoff_idx: usize = 0;
        let target = format!("http://127.0.0.1:{chosen_port}");
        loop {
            announce(&app, StackStatus::Starting);
            match spawn_once(&app, chosen_port).await {
                Ok(child) => {
                    backoff_idx = 0;
                    info!(port = chosen_port, "temporal sidecar spawned");
                    publish_target(&app, &target);
                    announce(&app, StackStatus::Up);

                    tokio::select! {
                        _ = &mut stop_rx => {
                            info!("sidecar shutdown requested");
                            kill_child(child).await;
                            announce(&app, StackStatus::Down { reason: "app exiting".into() });
                            return;
                        }
                        () = wait_for_exit(chosen_port) => {
                            warn!("temporal sidecar exited unexpectedly");
                            announce(&app, StackStatus::Degraded { reason: "Temporal exited; restarting".into() });
                        }
                    }
                }
                Err(e) => {
                    warn!(error = %format!("{e:#}"), "failed to spawn temporal sidecar");
                    announce(
                        &app,
                        StackStatus::Down {
                            reason: format!("spawn failed: {e}"),
                        },
                    );
                }
            }

            let secs = BACKOFF_SCHEDULE
                .get(backoff_idx)
                .copied()
                .unwrap_or(*BACKOFF_SCHEDULE.last().unwrap());
            backoff_idx = (backoff_idx + 1).min(BACKOFF_SCHEDULE.len() - 1);

            tokio::select! {
                _ = &mut stop_rx => {
                    info!("sidecar shutdown requested during backoff");
                    announce(&app, StackStatus::Down { reason: "app exiting".into() });
                    return;
                }
                _ = tokio::time::sleep(Duration::from_secs(secs)) => {}
            }
        }
    });

    stop_tx
}

async fn spawn_once(app: &AppHandle, port: u16) -> Result<CommandChild> {
    let home = tokio::task::spawn_blocking(paths::home)
        .await?
        .map_err(|e| anyhow::anyhow!("resolving ~/.cori/: {e:#}"))?;
    // One DB file per port so concurrent supervisors on different ports
    // don't collide on SQLite locks. The well-known 7233 keeps the
    // canonical filename for cross-tool compatibility (cori CLI also
    // uses this name by convention).
    let db_filename = if port == DEFAULT_PORT {
        "temporal-dev.db".to_string()
    } else {
        format!("temporal-dev-{port}.db")
    };
    let db_path = home.join(&db_filename);
    let db_str = db_path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("non-UTF8 ~/.cori path: {}", db_path.display()))?
        .to_string();

    let ui_port = port.saturating_add(1);

    let (mut rx, child) = app
        .shell()
        .sidecar("temporal")
        .map_err(|e| anyhow::anyhow!("locating temporal sidecar binary: {e}"))?
        .args([
            "server",
            "start-dev",
            "--port",
            &port.to_string(),
            "--ui-port",
            &ui_port.to_string(),
            "--headless",
            "--db-filename",
            &db_str,
            "--log-level",
            "error",
        ])
        .spawn()
        .map_err(|e| anyhow::anyhow!("spawning temporal sidecar: {e}"))?;

    tauri::async_runtime::spawn(async move {
        while let Some(ev) = rx.recv().await {
            match ev {
                CommandEvent::Stdout(line) => {
                    debug!(?line, "temporal stdout");
                }
                CommandEvent::Stderr(line) => {
                    let s = String::from_utf8_lossy(&line).to_string();
                    warn!(line = %s, "temporal stderr");
                }
                CommandEvent::Terminated(t) => {
                    warn!(?t, "temporal sidecar terminated");
                    break;
                }
                _ => {}
            }
        }
    });

    // Readiness probe — up to 10s for gRPC to accept connections.
    let target = format!("http://127.0.0.1:{port}");
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        if cori_worker::runtime::preflight_check(&target, Duration::from_millis(300)).is_ok() {
            return Ok(child);
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    anyhow::bail!("temporal sidecar did not become reachable on {target} within 10s");
}

async fn kill_child(child: CommandChild) {
    // Tauri's CommandChild.kill is SIGKILL on Unix (TerminateProcess on
    // Windows). `temporal server start-dev` is fine to hard-kill — its
    // on-disk state lives in --db-filename and survives.
    if let Err(e) = child.kill() {
        warn!(error = %e, "failed to kill temporal sidecar");
    }
}

/// Block until the sidecar exits, by polling the readiness endpoint.
async fn wait_for_exit(port: u16) {
    let target = format!("http://127.0.0.1:{port}");
    loop {
        tokio::time::sleep(Duration::from_secs(2)).await;
        if cori_worker::runtime::preflight_check(&target, Duration::from_millis(400)).is_err() {
            return;
        }
    }
}
