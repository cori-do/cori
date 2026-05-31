//! Materialises the bundled Deno runtime under `~/.cori/runtime/`.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

use crate::paths;

const RUNNER_TS: &str = include_str!("../../../packages/runner/runner.ts");
const DENO_JSON: &str = include_str!("../../../packages/runner/deno.json");
const SDK_INDEX_TS: &str = include_str!("../../../packages/sdk/src/index.ts");

/// Install the runtime at `~/.cori/runtime/`. Idempotent.
pub fn ensure_installed() -> Result<()> {
    let root = paths::runtime_dir()?;
    install_at(&root)
}

fn install_at(root: &Path) -> Result<()> {
    fs::create_dir_all(root)
        .with_context(|| format!("creating runtime directory `{}`", root.display()))?;
    let sdk_dir = root.join("sdk");
    fs::create_dir_all(&sdk_dir)
        .with_context(|| format!("creating sdk directory `{}`", sdk_dir.display()))?;

    write_if_changed(&root.join("runner.ts"), RUNNER_TS)?;
    write_if_changed(&root.join("deno.json"), DENO_JSON)?;
    write_if_changed(&sdk_dir.join("index.ts"), SDK_INDEX_TS)?;
    Ok(())
}

fn write_if_changed(path: &Path, contents: &str) -> Result<()> {
    if let Ok(existing) = fs::read_to_string(path)
        && existing == contents
    {
        return Ok(());
    }
    fs::write(path, contents)
        .with_context(|| format!("writing runtime file `{}`", path.display()))?;
    Ok(())
}
