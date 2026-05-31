//! In-memory registry of Console-triggered runs.
//!
//! A `RunChannel` per active run holds:
//!   * a `tokio::sync::broadcast::Sender<RunEvent>` for live subscribers
//!   * a replay buffer so a client that connects mid-run still sees
//!     events it missed (plan + earlier steps)
//!
//! Push is sync (`std::sync::Mutex`) so it can be called from the
//! `ProgressSink` trait, which is also sync.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde::Serialize;
use tokio::sync::{RwLock, broadcast};

use cori_protocol::RunTrace;

/// One event on the SSE stream for a single run.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RunEvent {
    /// First event after consent + placement. Carries the planner's
    /// chosen task queue per step.
    Plan { assignments: Vec<PlanStep> },
    /// Activity transitioned to "running". Currently fires after the
    /// workflow completes (Temporal returns the whole activity list in
    /// one shot) — the schema is forward-compatible with truly live
    /// per-step streaming when that lands.
    StepStart {
        activity_id: String,
        step_name: String,
        kind: String,
        task_queue: Option<String>,
    },
    /// Activity reached a terminal state.
    StepFinish {
        activity_id: String,
        step_name: String,
        status: String,
        duration_ms: u64,
        error: Option<String>,
    },
    /// Final event on a successful run. Carries the persisted trace.
    Completed { trace: RunTrace },
    /// Final event on a run that errored before producing a trace
    /// (consent denied, missing capability, Temporal down, …).
    Failed { error: String },
}

#[derive(Debug, Clone, Serialize)]
pub struct PlanStep {
    pub activity_id: String,
    pub step_name: String,
    pub task_queue: String,
}

pub struct RunChannel {
    pub tx: broadcast::Sender<RunEvent>,
    pub buffer: Mutex<Vec<RunEvent>>,
}

impl RunChannel {
    pub fn new(capacity: usize) -> Arc<Self> {
        let (tx, _) = broadcast::channel(capacity);
        Arc::new(Self {
            tx,
            buffer: Mutex::new(Vec::new()),
        })
    }

    pub fn push(&self, ev: RunEvent) {
        if let Ok(mut b) = self.buffer.lock() {
            b.push(ev.clone());
        }
        // Ignore send error: no subscribers means the buffer is the
        // only consumer (replayed on next subscribe).
        let _ = self.tx.send(ev);
    }

    pub fn snapshot(&self) -> Vec<RunEvent> {
        self.buffer.lock().map(|b| b.clone()).unwrap_or_default()
    }
}

pub type RunRegistry = Arc<RwLock<HashMap<String, Arc<RunChannel>>>>;

pub fn new_registry() -> RunRegistry {
    Arc::new(RwLock::new(HashMap::new()))
}
