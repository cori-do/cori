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
//!    in a future update).
//! 3. `deno` on `PATH`.
//!
//! If none resolve, dispatch fails with [`BrokerError::RuntimeUnavailable`]
//! and a message pointing the user at `deno.land`.

use std::path::{Path, PathBuf};
use std::process::Command;

use thiserror::Error;

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

    /// Parse and type-check every workflow step without evaluating module code.
    ///
    /// Deno already provides the TypeScript runtime used by activities, so this
    /// closes the gap between the compiler's structural metadata extraction and
    /// the first activity import without introducing a second TS toolchain.
    pub fn validate_step_modules(
        &self,
        step_files: &[PathBuf],
    ) -> std::result::Result<(), StepValidationError> {
        if step_files.is_empty() {
            return Ok(());
        }

        let output = Command::new(&self.deno_bin)
            .arg("check")
            .arg("--quiet")
            .arg("--no-lock")
            .arg("--node-modules-dir=none")
            .arg("--config")
            .arg(&self.config_path)
            .args(step_files)
            .output()
            .map_err(StepValidationError::Spawn)?;

        if output.status.success() {
            return Ok(());
        }

        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let diagnostic = if stderr.trim().is_empty() {
            stdout.trim()
        } else {
            stderr.trim()
        };
        Err(StepValidationError::Failed {
            exit_code: output.status.code().unwrap_or(-1),
            diagnostic: if diagnostic.is_empty() {
                "Deno returned no diagnostic output".to_string()
            } else {
                diagnostic.to_string()
            },
        })
    }
}

/// Failure from the non-executing TypeScript validation gate.
#[derive(Debug, Error)]
pub enum StepValidationError {
    #[error("failed to start Deno workflow validation: {0}")]
    Spawn(#[source] std::io::Error),

    #[error("workflow TypeScript validation failed (Deno exit {exit_code}):\n{diagnostic}")]
    Failed { exit_code: i32, diagnostic: String },
}

fn locate_deno(runtime_root: &Path) -> crate::Result<PathBuf> {
    if let Ok(env) = std::env::var("CORI_DENO")
        && !env.is_empty()
    {
        let p = PathBuf::from(env);
        if p.is_file() {
            return Ok(p);
        }
        return Err(BrokerError::RuntimeUnavailable(format!(
            "$CORI_DENO is set to `{}` but no file exists there",
            p.display()
        )));
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

#[cfg(all(test, unix))]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn validation_surfaces_the_deno_diagnostic() {
        let temp = tempdir().expect("temporary runtime");
        let deno = temp.path().join("deno");
        fs::write(
            &deno,
            "#!/bin/sh\nprintf '%s\\n' 'error: SyntaxError: Expression expected at steps/02_bad.ts:13:9' >&2\nexit 1\n",
        )
        .expect("fake deno");
        fs::set_permissions(&deno, fs::Permissions::from_mode(0o755))
            .expect("fake deno permissions");

        let runtime = Runtime {
            deno_bin: deno,
            runner_script: temp.path().join("runner.ts"),
            config_path: temp.path().join("deno.json"),
        };
        let error = runtime
            .validate_step_modules(&[temp.path().join("steps/02_bad.ts")])
            .expect_err("validation should fail");

        assert!(matches!(
            error,
            StepValidationError::Failed { diagnostic, .. }
                if diagnostic.contains("steps/02_bad.ts:13:9")
        ));
    }

    #[test]
    fn no_step_files_need_no_subprocess() {
        let runtime = Runtime {
            deno_bin: PathBuf::from("does-not-exist"),
            runner_script: PathBuf::from("runner.ts"),
            config_path: PathBuf::from("deno.json"),
        };

        runtime
            .validate_step_modules(&[])
            .expect("empty validation should be a no-op");
    }
}
