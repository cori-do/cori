//! Long-running Cori worker.
//!
//! This crate currently ships:
//!
//! - [`temporal`]: spawn and supervise the bundled `temporal server
//!   start-dev` as a child process, with a TCP health check and graceful
//!   shutdown on SIGTERM / `Drop`.
//! - [`watcher`]: a debounced filesystem watcher that emits one event per
//!   runbook directory regardless of how noisy the editor's save burst was.
//! - [`daemon`]: composes the two into the worker loop that `cori worker
//!   start --local` launches.
//!
//! Actual Temporal-driven activity polling and execution will plug into
//! this scaffolding in a follow-up — the broker keeps doing the work; the
//! daemon is the supervision contract.

pub mod activities;
pub mod broker_ctx;
pub mod daemon;
pub mod runner;
pub mod runtime;
pub mod temporal;
pub mod watcher;
pub mod workflow;

pub use daemon::{RegisterFn, RegisterOutcome, WorkerConfig, WorkerReady, run};
pub use temporal::{Source as TemporalSource, Supervisor as TemporalSupervisor};
