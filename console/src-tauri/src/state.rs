//! Shared app state — channels for tray Quit handler, the run channel
//! registry for late re-subscribers, and the current stack status.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use cori_protocol::WorkerIdentity;
use tokio::sync::oneshot;

use crate::events::StackStatus;
use crate::runs::RunChannel;

pub type RunChannelMap = Arc<Mutex<HashMap<String, RunChannel>>>;

pub struct AppState {
    #[allow(dead_code)]
    pub identity: WorkerIdentity,
    pub task_queue: String,

    /// Tray Quit handler fires these once to drain the worker, cron
    /// driver, and Temporal sidecar in order.
    pub worker_stop: Mutex<Option<oneshot::Sender<()>>>,
    pub cron_stop: Mutex<Option<oneshot::Sender<()>>>,
    pub sidecar_stop: Mutex<Option<oneshot::Sender<()>>>,

    /// Snapshot of the latest stack status — served by `get_stack_status`
    /// to cover the cold-mount case before the first `stack:status`
    /// event arrives.
    pub stack_status: Mutex<StackStatus>,

    /// Per-run replay buffer + broadcast for late subscribers. Keyed by
    /// `run_id`. Arc-wrapped so background `ProgressSink` tasks can
    /// hold a handle that outlives any single Tauri command.
    pub run_channels: RunChannelMap,

    /// The Temporal endpoint the supervisor decided to use. `None`
    /// until the supervisor publishes either an external configured
    /// target or the port it spawned its own sidecar on. Consumers
    /// (`worker::bootstrap`, `await_temporal_ready`, `get_status`) wait
    /// for this to become `Some` rather than probing the network
    /// themselves — that way no caller can race the supervisor.
    pub temporal_target: Arc<Mutex<Option<String>>>,
}

impl AppState {
    pub fn new(identity: WorkerIdentity, task_queue: String) -> Self {
        Self {
            identity,
            task_queue,
            worker_stop: Mutex::new(None),
            cron_stop: Mutex::new(None),
            sidecar_stop: Mutex::new(None),
            stack_status: Mutex::new(StackStatus::Starting),
            run_channels: Arc::new(Mutex::new(HashMap::new())),
            temporal_target: Arc::new(Mutex::new(None)),
        }
    }
}
