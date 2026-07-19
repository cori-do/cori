//! Materialises the bundled Deno runtime under `~/.cori/runtime/`.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use cori_broker::runtime as broker_runtime;
use cori_protocol::CompiledWorkflow;

use crate::paths;

const RUNNER_TS: &str = include_str!("../../../packages/runner/runner.ts");
const SCHEMA_TS: &str = include_str!("../../../packages/runner/schema.ts");
const DENO_JSON: &str = include_str!("../../../packages/runner/deno.json");
const SDK_INDEX_TS: &str = include_str!("../../../packages/sdk/src/index.ts");

/// Install the runtime at `~/.cori/runtime/`. Idempotent.
pub fn ensure_installed() -> Result<()> {
    let root = paths::runtime_dir()?;
    install_at(&root)
}

/// Materialise and resolve the Deno runtime used by both preflight and runs.
pub fn resolve() -> Result<broker_runtime::Runtime> {
    ensure_installed()?;
    let runtime_root = paths::runtime_dir()?;
    broker_runtime::Runtime::resolve(&runtime_root).map_err(|error| {
        anyhow::anyhow!(
            "{error}\n\nIf you have Deno installed, you can also point Cori at it with:\n  \
             export CORI_DENO=$(which deno)"
        )
    })
}

/// Validate every compiled step as TypeScript without executing module code.
pub fn validate_workflow_sources(
    runtime: &broker_runtime::Runtime,
    workflow_root: &Path,
    compiled: &CompiledWorkflow,
) -> Result<()> {
    let step_files = compiled
        .steps
        .iter()
        .map(|step| workflow_root.join(&step.source_path))
        .collect::<Vec<_>>();
    runtime
        .validate_step_modules(&step_files)
        .context("validating workflow TypeScript modules")
}

fn install_at(root: &Path) -> Result<()> {
    fs::create_dir_all(root)
        .with_context(|| format!("creating runtime directory `{}`", root.display()))?;
    let sdk_dir = root.join("sdk");
    fs::create_dir_all(&sdk_dir)
        .with_context(|| format!("creating sdk directory `{}`", sdk_dir.display()))?;

    write_if_changed(&root.join("runner.ts"), RUNNER_TS)?;
    write_if_changed(&root.join("schema.ts"), SCHEMA_TS)?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installs_every_bundled_runtime_file() {
        let temp = tempfile::tempdir().unwrap();
        install_at(temp.path()).unwrap();

        let expected = [
            ("runner.ts", RUNNER_TS),
            ("schema.ts", SCHEMA_TS),
            ("deno.json", DENO_JSON),
            ("sdk/index.ts", SDK_INDEX_TS),
        ];
        for (relative, contents) in expected {
            let path = temp.path().join(relative);
            assert!(path.is_file(), "{} should be installed", path.display());
            assert_eq!(fs::read_to_string(path).unwrap(), contents);
        }
    }
}
