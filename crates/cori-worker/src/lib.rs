//! Temporal-backed Cori worker.
//!
//! After the Phase 1 strip this crate exposes only the per-run worker
//! bootstrap (`runner::run_workflow_once`), the single workflow type
//! (`workflow::CoriWorkflow`), the four activity handlers, and
//! the Temporal runtime wrapper. The long-running worker daemon,
//! filesystem watcher, and bundled-Temporal supervisor were removed
//! along with `cori start --local` and `cori worker start`; identity-
//! derived workers land in Phase 3 (`cori work`).

pub mod activities;
pub mod broker_ctx;
pub mod runner;
pub mod runtime;
pub mod workflow;
