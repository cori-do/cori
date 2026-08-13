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

use std::collections::HashSet;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use temporalio_client::{
    WorkflowCancelOptions, WorkflowDescribeOptions, WorkflowGetResultOptions, WorkflowStartOptions,
};
use temporalio_common::protos::temporal::api::{
    enums::v1::PendingActivityState, workflow::v1::PendingActivityInfo,
};
use temporalio_sdk::{Worker, WorkerOptions};
use tracing::{info, warn};

use crate::activities::CoriActivities;
use crate::runtime::CoriTemporalRuntime;
use crate::workflow::{CoriWorkflow, WorkflowInput, WorkflowOutput};

/// Receives activity lifecycle changes observed from Temporal execution state.
///
/// This deliberately lives at the runner boundary rather than in an
/// activity handler: an activity can be claimed by any worker on its
/// identity-derived queue, while the initiating client can inspect its
/// pending activity state regardless of which worker executed it.
pub trait ActivityProgressSink: Send + Sync {
    fn on_activity_started(&self, activity_id: &str);
    fn on_activity_completed(&self, activity_id: &str);
}

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
    run_workflow_once_with_progress(rt, workflow_id, input, None).await
}

/// Like [`run_workflow_once`], while also reporting activity starts and
/// successful completions as Temporal exposes them. Execution inspection is
/// worker-independent, so this must not be replaced with a process-local
/// callback.
pub async fn run_workflow_once_with_progress(
    rt: &CoriTemporalRuntime,
    workflow_id: String,
    input: WorkflowInput,
    progress: Option<Arc<dyn ActivityProgressSink>>,
) -> Result<WorkflowOutput> {
    let worker_options = WorkerOptions::new(rt.task_queue.clone())
        .register_workflow::<CoriWorkflow>()
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
            .start_workflow(CoriWorkflow::run, input, start_opts)
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
            r = await_workflow_result(&handle, progress) => {
                r
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

/// Wait for the workflow result while polling Temporal's lightweight pending
/// activity description. A 200 ms cadence keeps the Console responsive
/// without repeatedly retrieving the activity payloads held in history. The
/// final inspection closes the small race between a step transition and the
/// workflow result.
async fn await_workflow_result<Workflow>(
    handle: &temporalio_client::WorkflowHandle<temporalio_client::Client, Workflow>,
    progress: Option<Arc<dyn ActivityProgressSink>>,
) -> Result<WorkflowOutput>
where
    Workflow: temporalio_common::HasWorkflowDefinition
        + temporalio_common::WorkflowDefinition<Output = WorkflowOutput>,
{
    let Some(progress) = progress else {
        return handle
            .get_result(WorkflowGetResultOptions::default())
            .await
            .map_err(|e| anyhow::Error::new(e).context("awaiting Cori workflow result"));
    };

    let mut reporter = PendingActivityReporter::new(progress);
    let result = handle.get_result(WorkflowGetResultOptions::default());
    tokio::pin!(result);
    let mut tick = tokio::time::interval(Duration::from_millis(200));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            outcome = &mut result => {
                report_pending_activities(handle, &mut reporter).await;
                return outcome
                    .map_err(|e| anyhow::Error::new(e).context("awaiting Cori workflow result"));
            }
            _ = tick.tick() => {
                report_pending_activities(handle, &mut reporter).await;
            }
        }
    }
}

async fn report_pending_activities<Workflow>(
    handle: &temporalio_client::WorkflowHandle<temporalio_client::Client, Workflow>,
    reporter: &mut PendingActivityReporter,
) where
    Workflow: temporalio_common::HasWorkflowDefinition,
{
    match handle.describe(WorkflowDescribeOptions::default()).await {
        Ok(description) => reporter.observe(&description.raw_description.pending_activities),
        Err(error) => {
            // Progress is best effort. The normal result wait below remains
            // authoritative, so a transient observation failure must never
            // interrupt a workflow.
            warn!(error = %error, "could not inspect Temporal activity progress");
        }
    }
}

/// Advances the visual timeline from Temporal's currently pending activities.
/// Cori v1's DAG is linear, so seeing the next activity enter `Started`
/// durably proves that its predecessor completed. The final workflow trace
/// remains the authority for the last activity and every failure.
struct PendingActivityReporter {
    sink: Arc<dyn ActivityProgressSink>,
    started: HashSet<String>,
    completed: HashSet<String>,
}

impl PendingActivityReporter {
    fn new(sink: Arc<dyn ActivityProgressSink>) -> Self {
        Self {
            sink,
            started: HashSet::new(),
            completed: HashSet::new(),
        }
    }

    fn observe(&mut self, pending: &[PendingActivityInfo]) {
        for activity in pending {
            if activity.state != PendingActivityState::Started as i32
                || !self.started.insert(activity.activity_id.clone())
            {
                continue;
            }

            // The runtime executes the v1 DAG sequentially. Once a later
            // activity has actually started, every earlier in-flight activity
            // is conclusively complete. Do this before marking the new row as
            // running so the UI's completed count advances smoothly.
            let prior = self
                .started
                .iter()
                .filter(|id| **id != activity.activity_id && !self.completed.contains(*id))
                .cloned()
                .collect::<Vec<_>>();
            for activity_id in prior {
                self.completed.insert(activity_id.clone());
                self.sink.on_activity_completed(&activity_id);
            }
            self.sink.on_activity_started(&activity.activity_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use temporalio_common::protos::temporal::api::{
        enums::v1::PendingActivityState, workflow::v1::PendingActivityInfo,
    };

    use super::{ActivityProgressSink, PendingActivityReporter};

    #[derive(Default)]
    struct RecordingSink(Mutex<Vec<(String, String)>>);

    impl ActivityProgressSink for RecordingSink {
        fn on_activity_started(&self, activity_id: &str) {
            self.0
                .lock()
                .expect("recording sink lock")
                .push(("started".to_string(), activity_id.to_string()));
        }

        fn on_activity_completed(&self, activity_id: &str) {
            self.0
                .lock()
                .expect("recording sink lock")
                .push(("completed".to_string(), activity_id.to_string()));
        }
    }

    #[test]
    fn advances_the_linear_timeline_once_per_started_activity() {
        let sink = std::sync::Arc::new(RecordingSink::default());
        let mut reporter = PendingActivityReporter::new(sink.clone());
        let first = PendingActivityInfo {
            activity_id: "01_fetch_top_ids".to_string(),
            state: PendingActivityState::Started as i32,
            ..Default::default()
        };
        let second = PendingActivityInfo {
            activity_id: "02_fetch_stories".to_string(),
            state: PendingActivityState::Started as i32,
            ..Default::default()
        };

        reporter.observe(&[first.clone()]);
        reporter.observe(&[first]);
        reporter.observe(&[second.clone()]);
        reporter.observe(&[second]);

        assert_eq!(
            *sink.0.lock().expect("recording sink lock"),
            vec![
                ("started".to_string(), "01_fetch_top_ids".to_string()),
                ("completed".to_string(), "01_fetch_top_ids".to_string()),
                ("started".to_string(), "02_fetch_stories".to_string()),
            ]
        );
    }
}

/// Run a long-lived worker on `rt.task_queue` until SIGINT.
///
/// Used by `cori work`: registers the single workflow type + the four
/// activity handlers and polls forever. Returns `Ok(())` after a clean
/// shutdown triggered by Ctrl-C, or an error if worker construction
/// fails.
pub async fn serve_worker_until_signal(rt: &CoriTemporalRuntime) -> Result<()> {
    let worker_options = WorkerOptions::new(rt.task_queue.clone())
        .register_workflow::<CoriWorkflow>()
        .register_activities(CoriActivities)
        .build();
    let mut worker = Worker::new(&rt.core, (*rt.client).clone(), worker_options)
        .map_err(|e| anyhow::anyhow!("constructing Temporal worker: {e}"))?;
    let shutdown_handle = worker.shutdown_handle();
    info!(task_queue = %rt.task_queue, "cori worker polling");

    let signal_listener = async {
        if tokio::signal::ctrl_c().await.is_ok() {
            warn!("received SIGINT — shutting down worker");
            shutdown_handle();
        }
    };

    let worker_fut = async {
        if let Err(e) = worker.run().await {
            warn!(error = %e, "temporal worker exited with error");
        }
    };

    let (_, _) = tokio::join!(signal_listener, worker_fut);
    Ok(())
}

/// Like [`serve_worker_until_signal`] but driven by an injected
/// cancellation future instead of `ctrl_c`. Used by the desktop app's
/// tray "Quit" handler, where the cancellation source is a oneshot
/// channel rather than a signal.
pub async fn serve_worker_until_cancelled<F>(rt: &CoriTemporalRuntime, cancel: F) -> Result<()>
where
    F: Future<Output = ()> + Send,
{
    let worker_options = WorkerOptions::new(rt.task_queue.clone())
        .register_workflow::<CoriWorkflow>()
        .register_activities(CoriActivities)
        .build();
    let mut worker = Worker::new(&rt.core, (*rt.client).clone(), worker_options)
        .map_err(|e| anyhow::anyhow!("constructing Temporal worker: {e}"))?;
    let shutdown_handle = worker.shutdown_handle();
    info!(task_queue = %rt.task_queue, "cori worker polling (cancellable)");

    let cancel_listener = async {
        cancel.await;
        warn!("cancellation received — shutting down worker");
        shutdown_handle();
    };

    let worker_fut = async {
        if let Err(e) = worker.run().await {
            warn!(error = %e, "temporal worker exited with error");
        }
    };

    let (_, _) = tokio::join!(cancel_listener, worker_fut);
    Ok(())
}
