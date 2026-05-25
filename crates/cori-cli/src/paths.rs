//! Resolves Cori's local state directories.
//!
//! v1 stores everything under `~/.cori/`:
//!
//! ```text
//! ~/.cori/
//! ├── config.toml      # CLI config (LLM providers, temporal.host, ...)
//! ├── registry.db      # SQLite registry (workflows, runs)
//! ├── runbooks/        # cached copies of registered runbooks
//! ├── state/           # transient state (locks, pids, ...)
//! └── logs/            # worker/serve log files
//! ```
//!
//! The home directory can be overridden with the `CORI_HOME` environment
//! variable, which makes integration tests trivial.

use std::path::PathBuf;

use anyhow::{Context, Result};

/// Root Cori state directory. Honours `$CORI_HOME` if set, otherwise
/// `$HOME/.cori`.
pub fn home() -> Result<PathBuf> {
    if let Ok(p) = std::env::var("CORI_HOME") {
        if !p.is_empty() {
            return Ok(PathBuf::from(p));
        }
    }
    let home = dirs::home_dir().context("could not resolve user home directory ($HOME unset?)")?;
    Ok(home.join(".cori"))
}

pub fn runbooks_dir() -> Result<PathBuf> {
    Ok(home()?.join("runbooks"))
}

pub fn state_dir() -> Result<PathBuf> {
    Ok(home()?.join("state"))
}

pub fn logs_dir() -> Result<PathBuf> {
    Ok(home()?.join("logs"))
}

pub fn registry_db() -> Result<PathBuf> {
    Ok(home()?.join("registry.db"))
}

pub fn config_file() -> Result<PathBuf> {
    Ok(home()?.join("config.toml"))
}

/// Root of the bundled Deno runtime (`~/.cori/runtime/`).
///
/// The current runtime populates this with the runner script, its
/// `deno.json` import map, and a copy of `@cori/sdk`. The Deno binary is
/// either picked up from `PATH` or installed into this directory in a
/// future update (see
/// `cori-broker::runtime`).
pub fn runtime_dir() -> Result<PathBuf> {
    Ok(home()?.join("runtime"))
}
