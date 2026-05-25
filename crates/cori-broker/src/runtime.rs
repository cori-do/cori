//! Resolves the Deno binary and runner script paths.
//!
//! The runner script and its `deno.json` import map are installed by
//! `cori init --local` into `~/.cori/runtime/`. The broker takes the
//! runtime root as a parameter so tests can point it at a temporary
//! directory.
//!
//! The Deno binary lookup falls back through three candidates, in order:
//!
//! 1. The `CORI_DENO` environment variable.
//! 2. `<runtime>/deno` (where `cori init` would download the pinned binary
//!    in a future phase — see roadmap §3.2).
//! 3. `deno` on `PATH`.
//!
//! If none resolve, dispatch fails with [`BrokerError::RuntimeUnavailable`]
//! and a message pointing the user at `deno.land`.

use std::path::{Path, PathBuf};

use crate::BrokerError;

/// Resolved paths needed to spawn the runner.
#[derive(Debug, Clone)]
pub struct Runtime {
    pub deno_bin: PathBuf,
    pub runner_script: PathBuf,
    pub config_path: PathBuf,
}

impl Runtime {
    /// Resolve from a runtime root, returning [`BrokerError::RuntimeUnavailable`]
    /// if any required file (or the Deno binary) is missing.
    pub fn resolve(runtime_root: &Path) -> crate::Result<Self> {
        let runner_script = runtime_root.join("runner.ts");
        let config_path = runtime_root.join("deno.json");

        if !runner_script.is_file() {
            return Err(BrokerError::RuntimeUnavailable(format!(
                "runner script missing at `{}` — re-run `cori init --local`",
                runner_script.display()
            )));
        }
        if !config_path.is_file() {
            return Err(BrokerError::RuntimeUnavailable(format!(
                "Deno config missing at `{}` — re-run `cori init --local`",
                config_path.display()
            )));
        }

        let deno_bin = locate_deno(runtime_root)?;
        Ok(Self {
            deno_bin,
            runner_script,
            config_path,
        })
    }
}

fn locate_deno(runtime_root: &Path) -> crate::Result<PathBuf> {
    if let Ok(env) = std::env::var("CORI_DENO") {
        if !env.is_empty() {
            let p = PathBuf::from(env);
            if p.is_file() {
                return Ok(p);
            }
            return Err(BrokerError::RuntimeUnavailable(format!(
                "$CORI_DENO is set to `{}` but no file exists there",
                p.display()
            )));
        }
    }

    let bundled = runtime_root.join(if cfg!(windows) { "deno.exe" } else { "deno" });
    if bundled.is_file() {
        return Ok(bundled);
    }

    if let Some(found) = which("deno") {
        return Ok(found);
    }

    Err(BrokerError::RuntimeUnavailable(
        "no `deno` binary found on PATH and none installed at the runtime root".to_string(),
    ))
}

/// Minimal cross-platform PATH lookup — avoids adding a `which` dependency
/// just for this. Returns `None` if nothing matches.
fn which(name: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    let exe_suffixes: &[&str] = if cfg!(windows) {
        &["", ".exe", ".cmd", ".bat"]
    } else {
        &[""]
    };
    for dir in std::env::split_paths(&path_var) {
        for suffix in exe_suffixes {
            let candidate = dir.join(format!("{name}{suffix}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}
