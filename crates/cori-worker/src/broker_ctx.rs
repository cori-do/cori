//! Process-wide broker context used by activity handlers.
//!
//! Activities are looked up by name at poll time, so they cannot easily
//! receive per-run state as closure arguments. Instead, the CLI / daemon
//! initializes a [`BrokerCtx`] once via [`set_broker_ctx`], and the
//! activity functions read it back via [`broker_ctx`].
//!
//! The context carries everything the broker needs that does NOT come
//! from the per-activity input payload:
//!
//! - Deno [`cori_broker::runtime::Runtime`] handle.
//! - Discovered [`cori_broker::capabilities::Capabilities`].
//! - LLM credentials wrapped in [`cori_broker::llm::LlmOptions`].
//! - The on-disk root of the workflow (`source_path`), so an activity
//!   can resolve `step.source_path` to an absolute file.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Result, anyhow};
use tokio::sync::OnceCell;

use cori_broker::capabilities::Capabilities;
use cori_broker::llm::LlmOptions;
use cori_broker::runtime::Runtime;

/// Everything the broker needs that is constant across one workflow run.
pub struct BrokerCtx {
    pub runtime: Runtime,
    pub caps: Capabilities,
    pub llm_opts: LlmOptions,
    /// Absolute path to the registered workflow directory.
    pub source_root: PathBuf,
    /// `~/.cori/credentials/` — token-store backing directory used by
    /// the OAuth subsystem for the encrypted-file fallback and the
    /// non-secret `index.json` metadata file.
    pub credentials_dir: PathBuf,
}

static BROKER_CTX: OnceCell<Arc<BrokerCtx>> = OnceCell::const_new();

/// Initialize the process-wide broker context. May only be called once
/// per process.
pub fn set_broker_ctx(ctx: BrokerCtx) -> Result<()> {
    BROKER_CTX
        .set(Arc::new(ctx))
        .map_err(|_| anyhow!("broker context was already initialized"))
}

/// Retrieve the process-wide broker context. Panics in tests / mis-wired
/// callers if [`set_broker_ctx`] was never called.
pub fn broker_ctx() -> Arc<BrokerCtx> {
    BROKER_CTX
        .get()
        .cloned()
        .expect("BrokerCtx not initialized — call set_broker_ctx() before running a workflow")
}
