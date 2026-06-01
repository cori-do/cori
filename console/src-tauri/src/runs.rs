//! Per-run channel: replay buffer + broadcast subscribers.
//!
//! Each in-flight run holds a [`RunChannel`] in `AppState`. The
//! initiating `start_run` Tauri command consumes events as they happen
//! and dual-writes them to (a) the caller's [`tauri::ipc::Channel`] and
//! (b) the replay buffer here, so a later `subscribe_run` (e.g. user
//! navigates to `/runs/live/:id`) can stream the buffered prefix
//! followed by live events.

use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use cori_protocol::trace::RunTrace;

/// What flows down a [`tauri::ipc::Channel<RunEvent>`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RunEvent {
    Plan {
        assignments: Vec<PlanStep>,
    },
    StepStart {
        activity_id: String,
        step_name: String,
        kind: String,
        task_queue: Option<String>,
    },
    StepFinish {
        activity_id: String,
        step_name: String,
        status: String,
        duration_ms: u64,
        error: Option<String>,
    },
    Completed {
        trace: Box<RunTrace>,
    },
    Failed {
        error: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanStep {
    pub activity_id: String,
    pub step_name: String,
    pub kind: String,
    pub task_queue: Option<String>,
}

const REPLAY_CAP: usize = 256;
const BROADCAST_CAP: usize = 64;

/// Per-run broadcast + replay buffer.
pub struct RunChannel {
    pub tx: broadcast::Sender<RunEvent>,
    pub replay: Vec<RunEvent>,
    pub terminated: bool,
}

impl RunChannel {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(BROADCAST_CAP);
        Self {
            tx,
            replay: Vec::new(),
            terminated: false,
        }
    }

    pub fn push(&mut self, ev: RunEvent) {
        if self.replay.len() >= REPLAY_CAP {
            self.replay.remove(0);
        }
        if matches!(ev, RunEvent::Completed { .. } | RunEvent::Failed { .. }) {
            self.terminated = true;
        }
        let _ = self.tx.send(ev.clone());
        self.replay.push(ev);
    }
}

impl Default for RunChannel {
    fn default() -> Self {
        Self::new()
    }
}
