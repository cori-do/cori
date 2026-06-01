//! Resolves Cori's local state directories.
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

/// `~/.cori/schedules/` — schedule intent store (§3.2).
pub fn schedules_dir() -> Result<PathBuf> {
    Ok(home()?.join("schedules"))
}
