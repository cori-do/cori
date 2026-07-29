//! Materialises the bundled Deno runtime under `~/.cori/runtime/`.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use cori_broker::runtime as broker_runtime;
use cori_protocol::CompiledWorkflow;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::paths;
use crate::remote::git::FileLock;

const RUNNER_TS: &str = include_str!("../../../packages/runner/runner.ts");
const SCHEMA_TS: &str = include_str!("../../../packages/runner/schema.ts");
const DENO_JSON: &str = include_str!("../../../packages/runner/deno.json");
const DENO_LOCK: &str = include_str!("../../../packages/runner/deno.lock");
const SDK_INDEX_TS: &str = include_str!("../../../packages/sdk/src/index.ts");

/// Install the runtime at `~/.cori/runtime/`. Idempotent.
pub fn ensure_installed() -> Result<()> {
    let root = versioned_runtime_root()?;
    install_version_atomically(&root)
}

/// Materialise and resolve the Deno runtime used by both preflight and runs.
pub fn resolve() -> Result<broker_runtime::Runtime> {
    ensure_installed()?;
    let runtime_root = versioned_runtime_root()?;
    let runtime = broker_runtime::Runtime::resolve(&runtime_root).map_err(|error| {
        anyhow::anyhow!(
            "{error}\n\nIf you have Deno installed, you can also point Cori at it with:\n  \
             export CORI_DENO=$(which deno)"
        )
    })?;
    let _cache_lock = FileLock::acquire(&runtime_root.join(".cache.lock"))?;
    runtime
        .cache_locked_dependencies()
        .context("preparing locked Deno runtime dependencies")?;
    Ok(runtime)
}

fn runtime_content_id() -> String {
    let mut hasher = Sha256::new();
    for (name, contents) in bundled_files() {
        hasher.update(name.as_bytes());
        hasher.update([0]);
        hasher.update(contents.as_bytes());
        hasher.update([0]);
    }
    hex::encode(hasher.finalize())
}

fn versioned_runtime_root() -> Result<PathBuf> {
    Ok(paths::runtime_dir()?
        .join("versions")
        .join(runtime_content_id()))
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
        .validate_step_modules(&step_files, workflow_root)
        .context("validating workflow TypeScript modules")
}

fn install_at(root: &Path) -> Result<()> {
    fs::create_dir_all(root)
        .with_context(|| format!("creating runtime directory `{}`", root.display()))?;
    let sdk_dir = root.join("sdk");
    fs::create_dir_all(&sdk_dir)
        .with_context(|| format!("creating sdk directory `{}`", sdk_dir.display()))?;

    for (relative, contents) in bundled_files() {
        write_if_changed(&root.join(relative), contents)?;
    }
    Ok(())
}

fn bundled_files() -> [(&'static str, &'static str); 5] {
    [
        ("runner.ts", RUNNER_TS),
        ("schema.ts", SCHEMA_TS),
        ("deno.json", DENO_JSON),
        ("deno.lock", DENO_LOCK),
        ("sdk/index.ts", SDK_INDEX_TS),
    ]
}

fn install_version_atomically(root: &Path) -> Result<()> {
    let versions = root
        .parent()
        .ok_or_else(|| anyhow::anyhow!("versioned runtime path has no parent"))?;
    fs::create_dir_all(versions)
        .with_context(|| format!("creating runtime versions `{}`", versions.display()))?;
    let _lock = FileLock::acquire(&versions.join(".install.lock"))?;
    if runtime_is_complete(root) {
        return Ok(());
    }
    if root.exists() {
        let quarantine = versions.join(format!(
            ".{}.incomplete-{}",
            runtime_content_id(),
            Uuid::new_v4().simple()
        ));
        fs::rename(root, &quarantine).with_context(|| {
            format!(
                "quarantining incomplete runtime `{}` to `{}`",
                root.display(),
                quarantine.display()
            )
        })?;
    }

    let partial = versions.join(format!(
        ".{}.partial-{}-{}",
        runtime_content_id(),
        std::process::id(),
        Uuid::new_v4().simple()
    ));
    fs::create_dir(&partial)
        .with_context(|| format!("creating runtime staging `{}`", partial.display()))?;
    let installed = install_at(&partial).and_then(|()| {
        fs::write(partial.join(".complete"), runtime_content_id())
            .with_context(|| format!("writing runtime completion marker `{}`", partial.display()))
    });
    if let Err(error) = installed {
        let _ = fs::remove_dir_all(&partial);
        return Err(error);
    }
    if !runtime_is_complete(&partial) {
        let _ = fs::remove_dir_all(&partial);
        anyhow::bail!(
            "staged Deno runtime `{}` failed content verification",
            partial.display()
        );
    }
    fs::rename(&partial, root).with_context(|| {
        format!(
            "committing versioned Deno runtime `{}` to `{}`",
            partial.display(),
            root.display()
        )
    })?;
    Ok(())
}

fn runtime_is_complete(root: &Path) -> bool {
    bundled_files().iter().all(|(relative, contents)| {
        fs::read_to_string(root.join(relative)).is_ok_and(|actual| actual == *contents)
    }) && fs::read_to_string(root.join(".complete"))
        .is_ok_and(|value| value == runtime_content_id())
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
            ("deno.lock", DENO_LOCK),
            ("sdk/index.ts", SDK_INDEX_TS),
        ];
        for (relative, contents) in expected {
            let path = temp.path().join(relative);
            assert!(path.is_file(), "{} should be installed", path.display());
            assert_eq!(fs::read_to_string(path).unwrap(), contents);
        }
    }

    #[test]
    fn versioned_runtime_install_is_atomic_and_content_addressed() {
        let temp = tempfile::tempdir().expect("temporary runtime parent");
        let content_id = runtime_content_id();
        let root = temp.path().join("versions").join(&content_id);
        install_version_atomically(&root).expect("versioned install");
        assert!(runtime_is_complete(&root));
        assert_eq!(
            root.file_name().and_then(|name| name.to_str()),
            Some(content_id.as_str())
        );
        install_version_atomically(&root).expect("idempotent versioned install");
    }

    #[test]
    fn shipped_translation_workflow_passes_deno_preflight() {
        let runtime_dir = tempfile::tempdir().expect("temporary runtime");
        install_at(runtime_dir.path()).expect("install bundled runtime");
        let runtime = match broker_runtime::Runtime::resolve(runtime_dir.path()) {
            Ok(runtime) => runtime,
            Err(error) => {
                eprintln!("skipping shipped-example Deno validation: {error}");
                return;
            }
        };

        let workflow_root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/translate_product_sheets_fr");
        let compiled = cori_compiler::compile(&workflow_root).expect("compile shipped workflow");

        validate_workflow_sources(&runtime, &workflow_root, &compiled)
            .expect("extensionless local imports should match activity module resolution");
    }
}
