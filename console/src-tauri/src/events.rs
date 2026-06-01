//! Global Tauri events emitted from the Rust core.
//!
//! Per-run streams use `tauri::ipc::Channel<RunEvent>` (see `runs.rs`);
//! everything else is a broadcast `app.emit(...)`.

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum StackStatus {
    Starting,
    Up,
    Degraded { reason: String },
    Down { reason: String },
}

pub const EVENT_STACK_STATUS: &str = "stack:status";
#[allow(dead_code)]
pub const EVENT_SCHEDULE_FIRED: &str = "schedule:fired";
