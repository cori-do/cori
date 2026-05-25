//! Materialises the bundled Deno runtime under `~/.cori/runtime/`.
//!
//! The runner script, its `deno.json` import map, and a copy of `@cori/sdk`
//! are embedded into the `cori` binary at compile time via `include_str!`.
//! `install()` writes them to the runtime root, overwriting any existing
//! copies so a binary upgrade automatically refreshes the runtime files.
//!
//! The Deno binary itself is not bundled yet. The broker falls back to
//! `deno` on `PATH` until that changes.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

use crate::paths;

const RUNNER_TS: &str = include_str!("../../../packages/deno-runner/runner.ts");
const DENO_JSON: &str = include_str!("../../../packages/deno-runner/deno.json");
const SDK_INDEX_TS: &str = include_str!("../../../packages/sdk/src/index.ts");

/// Outcome of installing the runtime — returned so callers can print a
/// concise "created/updated/unchanged" summary.
#[derive(Debug, Clone, Copy)]
pub struct InstallReport {
    pub runner_written: bool,
    pub config_written: bool,
    pub sdk_written: bool,
}

impl InstallReport {
    pub fn any_change(&self) -> bool {
        self.runner_written || self.config_written || self.sdk_written
    }
}

/// Install the runtime files at the default location.
pub fn install() -> Result<InstallReport> {
    let root = paths::runtime_dir()?;
    install_at(&root)
}

/// Install at an explicit directory — useful for tests that want isolation.
pub fn install_at(root: &Path) -> Result<InstallReport> {
    fs::create_dir_all(root)
        .with_context(|| format!("creating runtime directory `{}`", root.display()))?;
    let sdk_dir = root.join("sdk");
    fs::create_dir_all(&sdk_dir)
        .with_context(|| format!("creating sdk directory `{}`", sdk_dir.display()))?;

    let runner_written = write_if_changed(&root.join("runner.ts"), RUNNER_TS)?;
    let config_written = write_if_changed(&root.join("deno.json"), DENO_JSON)?;
    let sdk_written = write_if_changed(&sdk_dir.join("index.ts"), SDK_INDEX_TS)?;

    Ok(InstallReport {
        runner_written,
        config_written,
        sdk_written,
    })
}

/// Returns `true` if the file was created or its contents changed.
fn write_if_changed(path: &Path, contents: &str) -> Result<bool> {
    if let Ok(existing) = fs::read_to_string(path) {
        if existing == contents {
            return Ok(false);
        }
    }
    fs::write(path, contents)
        .with_context(|| format!("writing runtime file `{}`", path.display()))?;
    Ok(true)
}
