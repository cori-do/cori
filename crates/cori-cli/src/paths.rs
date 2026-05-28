//! Resolves Cori's local state directories.
//!
//! Phase 2 of the redesign moves Cori to the disk-as-truth layout:
//!
//! ```text
//! ~/.cori/
//! ├── config.toml      # CLI config (LLM keys, temporal.host, ...)
//! ├── cache/           # content-addressed compiled DAGs (rebuildable)
//! ├── runs/            # run-trace JSON, keyed by workflow folder path
//! ├── credentials/     # token metadata; real secrets in OS keychain
//! ├── runtime/         # bundled Deno runner (extracted lazily)
//! └── state/           # transient: dev-temporal pid, announce flags
//! ```
//!
//! Directories are created lazily on first write (`cori init` is gone).
//! The home directory can be overridden with `$CORI_HOME`, which makes
//! integration tests trivial.

use std::path::PathBuf;

use anyhow::{Context, Result};

/// Root Cori state directory. Honours `$CORI_HOME` if set, otherwise
/// `$HOME/.cori`.
pub fn home() -> Result<PathBuf> {
    if let Ok(p) = std::env::var("CORI_HOME")
        && !p.is_empty()
    {
        return Ok(PathBuf::from(p));
    }
    let home = dirs::home_dir().context("could not resolve user home directory ($HOME unset?)")?;
    Ok(home.join(".cori"))
}

pub fn config_file() -> Result<PathBuf> {
    Ok(home()?.join("config.toml"))
}

pub fn cache_dir() -> Result<PathBuf> {
    Ok(home()?.join("cache"))
}

pub fn runs_dir() -> Result<PathBuf> {
    Ok(home()?.join("runs"))
}

#[allow(dead_code)] // populated in Phase 5
pub fn credentials_dir() -> Result<PathBuf> {
    Ok(home()?.join("credentials"))
}

pub fn runtime_dir() -> Result<PathBuf> {
    Ok(home()?.join("runtime"))
}

pub fn state_dir() -> Result<PathBuf> {
    Ok(home()?.join("state"))
}

/// Worker capability reports (`~/.cori/cluster/<task_queue>.json`).
/// v1 is local-disk-only: enough for solo dev and small
/// shared-filesystem clusters. Phase 6+ replaces with a Temporal-native
/// advertising mechanism.
pub fn cluster_dir() -> Result<PathBuf> {
    Ok(home()?.join("cluster"))
}

/// Root for fetched remote workflows: `~/.cori/cache/remote/`.
pub fn remote_cache_dir() -> Result<PathBuf> {
    Ok(cache_dir()?.join("remote"))
}

/// `~/.cori/cache/remote/pins.json` — `ref → sha` map.
pub fn pins_file() -> Result<PathBuf> {
    Ok(remote_cache_dir()?.join("pins.json"))
}

/// `~/.cori/cache/remote/trust.json` — consented (repo, sha) pairs.
pub fn trust_file() -> Result<PathBuf> {
    Ok(remote_cache_dir()?.join("trust.json"))
}
