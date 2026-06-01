//! Cori — Tauri v2 desktop app.
//!
//! The entry point assembles the tray-resident shell, registers the
//! single-instance + shell + opener plugins, brings up the Temporal
//! sidecar supervisor + in-process worker + cron driver, and wires
//! the tray "Quit" handler to drain them in order.

mod commands;
mod error;
mod events;
mod runs;
mod state;
mod supervisor;
mod temporal;
mod trigger;
mod worker;
mod workers_schedules;

use std::sync::Mutex;

use cori_broker::identity::{IdentitySource, OsUser};
use cori_protocol::task_queue_for;
use tauri::image::Image;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, WindowEvent};
use tokio::sync::oneshot;
use tracing::{info, warn};

use crate::state::AppState;

pub fn run() {
    // tauri-plugin-single-instance MUST be the first plugin on Windows
    // for the focus-existing-window behaviour to fire on relaunch.
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.show();
                let _ = w.set_focus();
            }
        }))
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            // Resolve identity synchronously up-front so AppState has a
            // stable value; everything else moves to async background tasks.
            let identity = OsUser
                .resolve()
                .map_err(|e| -> Box<dyn std::error::Error> {
                    format!("resolving OS user identity: {e:#}").into()
                })?;
            let queue = task_queue_for(&identity);
            info!(?identity, %queue, "cori console booting");

            let app_state = AppState::new(identity, queue);
            app.manage(app_state);

            // Intercept window close → hide-to-tray.
            if let Some(window) = app.get_webview_window("main") {
                let w = window.clone();
                window.on_window_event(move |ev| {
                    if let WindowEvent::CloseRequested { api, .. } = ev {
                        api.prevent_close();
                        let _ = w.hide();
                    }
                });
            }

            // Tray icon.
            build_tray(app.handle())?;

            // Spawn the Temporal sidecar supervisor.
            let sidecar_stop = supervisor::spawn(app.handle().clone());
            if let Some(state) = app.try_state::<AppState>() {
                *state.sidecar_stop.lock().unwrap() = Some(sidecar_stop);
            }

            // Spawn the worker + cron driver in an async task once the
            // sidecar (or pre-existing Temporal) is up.
            let app_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                // Wait briefly for Temporal to come up. The supervisor
                // races the same readiness probe; we tolerate either
                // outcome and retry from the worker side if needed.
                if let Err(e) = await_temporal_ready(&app_handle).await {
                    warn!(error = %format!("{e:#}"), "temporal not ready in time");
                }

                match worker::bootstrap(app_handle.clone()).await {
                    Ok(handles) => {
                        if let Some(state) = app_handle.try_state::<AppState>() {
                            *state.worker_stop.lock().unwrap() = Some(handles.worker_stop);
                            *state.cron_stop.lock().unwrap() = Some(handles.cron_stop);
                        }
                        info!(task_queue = %handles.task_queue, "worker + cron driver online");
                    }
                    Err(e) => {
                        warn!(error = %format!("{e:#}"), "worker bootstrap failed");
                    }
                }
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_status,
            commands::list_runs,
            commands::get_run,
            commands::list_recent_workflows,
            commands::get_stack_status,
            trigger::resolve_workflow,
            trigger::start_run,
            trigger::subscribe_run,
            trigger::record_trust,
            workers_schedules::list_workers,
            workers_schedules::list_schedules,
            workers_schedules::enable_schedule,
            workers_schedules::set_schedule_enabled,
            workers_schedules::delete_schedule,
        ])
        .run(tauri::generate_context!())
        .expect("error while running cori");
}

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "Open Cori", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit Cori", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &quit])?;

    let tray_icon = Image::from_bytes(include_bytes!("../icons/tray.png"))?;

    let _tray = TrayIconBuilder::new()
        .icon(tray_icon)
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, ev| match ev.id().as_ref() {
            "show" => {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.show();
                    let _ = w.set_focus();
                }
            }
            "quit" => {
                initiate_quit(app);
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, ev| {
            if let TrayIconEvent::Click { .. } = ev {
                let app = tray.app_handle();
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.show();
                    let _ = w.set_focus();
                }
            }
        })
        .build(app)?;
    Ok(())
}

/// Tray "Quit Cori" handler. Drains the cron driver, worker,
/// and sidecar in order, then exits.
fn initiate_quit(app: &AppHandle) {
    let app_handle = app.clone();
    tauri::async_runtime::spawn(async move {
        info!("quit requested — draining background tasks");

        let (cron_tx, worker_tx, sidecar_tx, queue) = {
            let Some(state) = app_handle.try_state::<AppState>() else {
                app_handle.exit(0);
                return;
            };
            (
                take_tx(&state.cron_stop),
                take_tx(&state.worker_stop),
                take_tx(&state.sidecar_stop),
                state.task_queue.clone(),
            )
        };

        if let Some(tx) = cron_tx {
            let _ = tx.send(());
        }
        if let Some(tx) = worker_tx {
            let _ = tx.send(());
        }
        // Give worker a small grace period to drain.
        tokio::time::sleep(std::time::Duration::from_millis(800)).await;

        if let Some(tx) = sidecar_tx {
            let _ = tx.send(());
        }
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;

        // Best-effort cleanup of `~/.cori/cluster/<queue>.json`.
        worker::unpublish(&queue);

        info!("clean shutdown complete");
        app_handle.exit(0);
    });
}

fn take_tx(slot: &Mutex<Option<oneshot::Sender<()>>>) -> Option<oneshot::Sender<()>> {
    slot.lock().ok().and_then(|mut g| g.take())
}

/// Wait until the supervisor publishes a Temporal target into
/// `AppState.temporal_target` AND that target is actually reachable.
/// This is the single rendezvous between the supervisor and any
/// downstream consumer (the worker bootstrap, IPC commands) — no
/// caller probes the network independently, which is what was
/// triggering the race-spawn orphan in earlier iterations.
async fn await_temporal_ready(app: &AppHandle) -> anyhow::Result<()> {
    use std::time::Duration;
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while std::time::Instant::now() < deadline {
        let target = app
            .try_state::<AppState>()
            .and_then(|s| s.temporal_target.lock().ok().and_then(|g| g.clone()));
        if let Some(t) = target
            && cori_worker::runtime::preflight_check(&t, Duration::from_millis(500)).is_ok()
        {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(400)).await;
    }
    anyhow::bail!("Temporal not ready within 20s — supervisor did not publish a reachable target")
}
