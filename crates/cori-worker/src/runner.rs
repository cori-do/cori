//! Per-run worker bootstrap.
//!
//! `cori run` spawns a fresh worker for each invocation: register the
//! single workflow type + the four activities, start polling, start the
//! workflow, wait for the result, tear down. This keeps the CLI
//! self-contained and lets the long-running `cori start --local` daemon
//! reuse the same module for the workflows it picks up from the
//! filesystem watcher.
//!
//! ⚠️ The Temporal `Worker::run` future is `!Send` (workflows must run
//! on a single thread). We therefore drive worker + starter
//! concurrently on the *current* task via `tokio::join!` instead of
//! `tokio::spawn`. Callers must invoke this from a runtime where the
//! current task can block for the workflow's lifetime — the per-run
//! CLI pattern handles this by giving each `cori run` its own runtime.

use anyhow::Result;
use temporalio_client::{WorkflowCancelOptions, WorkflowGetResultOptions, WorkflowStartOptions};
use temporalio_sdk::{Worker, WorkerOptions};
use tracing::{info, warn};

use crate::activities::CoriActivities;
use crate::runtime::CoriTemporalRuntime;
use crate::workflow::{CoriRunbookWorkflow, WorkflowInput, WorkflowOutput};

/// Spin up a worker + start one workflow + await its result.
///
/// Installs a Ctrl-C listener: the first SIGINT sends a workflow
/// cancellation request, then we wait (up to 5s) for the workflow to
/// observe the cancel and clean up; a second SIGINT terminates the
/// process by letting the parent CLI exit.
pub async fn run_workflow_once(
    rt: &CoriTemporalRuntime,
    workflow_id: String,
    input: WorkflowInput,
) -> Result<WorkflowOutput> {
    let worker_options = WorkerOptions::new(rt.task_queue.clone())
        .register_workflow::<CoriRunbookWorkflow>()
        .register_activities(CoriActivities)
        .build();
    let mut worker = Worker::new(&rt.core, (*rt.client).clone(), worker_options)
        .map_err(|e| anyhow::anyhow!("constructing Temporal worker: {e}"))?;
    let shutdown_handle = worker.shutdown_handle();
    info!(task_queue = %rt.task_queue, "temporal worker registered");

    let starter = async {
        let start_opts =
            WorkflowStartOptions::new(rt.task_queue.clone(), workflow_id.clone()).build();
        let handle = rt
            .client
            .start_workflow(CoriRunbookWorkflow::run, input, start_opts)
            .await
            .map_err(|e| anyhow::Error::new(e).context("starting Cori workflow"))?;
        info!(run_id = ?handle.run_id(), "workflow started");

        let cancel_listener = async {
            if tokio::signal::ctrl_c().await.is_ok() {
                warn!("received SIGINT — requesting workflow cancellation");
                let opts = WorkflowCancelOptions::builder()
                    .reason("user cancelled via SIGINT".to_string())
                    .build();
                if let Err(e) = handle.cancel(opts).await {
                    warn!(error = %e, "failed to send cancel request");
                }
            }
        };

        let result = tokio::select! {
            r = handle.get_result(WorkflowGetResultOptions::default()) => {
                r.map_err(|e| anyhow::Error::new(e).context("awaiting Cori workflow result"))
            }
            _ = cancel_listener => {
                // After cancel request, give the workflow up to 5s to
                // complete before forcing shutdown.
                match tokio::time::timeout(
                    std::time::Duration::from_secs(5),
                    handle.get_result(WorkflowGetResultOptions::default()),
                )
                .await
                {
                    Ok(r) => r
                        .map_err(|e| anyhow::Error::new(e).context("awaiting cancelled workflow")),
                    Err(_) => Err(anyhow::anyhow!(
                        "workflow did not stop within 5s of cancel request"
                    )),
                }
            }
        };

        // Tell the worker to stop polling so `worker.run()` returns.
        shutdown_handle();
        result
    };

    let worker_fut = async {
        if let Err(e) = worker.run().await {
            warn!(error = %e, "temporal worker exited with error");
        }
    };

    let (result, _) = tokio::join!(starter, worker_fut);
    result
}
