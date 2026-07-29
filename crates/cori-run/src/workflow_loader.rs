//! Resolves a workflow folder, compiles it (cache hit or miss), and
//! produces the run-history key. Pure functions, no shared state.

use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

use anyhow::{Context, Result, anyhow, bail};
use cori_compiler::{CompileError, compile, workflow_content_hash};
use cori_protocol::{CompiledWorkflow, WorkflowSource};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;
use walkdir::{DirEntry, WalkDir};

use crate::paths;
use crate::remote::{RemoteRef, Resolved};

/// Bump whenever compiler validation or output semantics change. Cache keys
/// remain path/content-derived; the envelope makes rebuildable entries from
/// an older compiler safely miss without changing that locked key shape.
const COMPILER_CACHE_FORMAT_VERSION: u32 = 2;

#[derive(Deserialize)]
struct CacheEntry {
    format_version: u32,
    compiled: CompiledWorkflow,
}

#[derive(Serialize)]
struct CacheEntryRef<'a> {
    format_version: u32,
    compiled: &'a CompiledWorkflow,
}

/// Outcome of [`load`]: the compiled DAG and a few derived strings the
/// run pipeline reuses.
pub struct LoadedWorkflow {
    pub folder_name: String,
    /// Canonical user-owned source path used for provenance, history keys, and
    /// consent UI. Cori never writes into this folder.
    pub absolute_path: PathBuf,
    /// Filesystem root activities execute from. This initially matches
    /// `absolute_path`; `run_workflow` replaces it with a verified,
    /// content-addressed snapshot after the consent gate.
    pub execution_root: PathBuf,
    pub compiled: CompiledWorkflow,
    pub content_hash: String,
    pub from_cache: bool,
    pub source: WorkflowSource,
    pub remote_spec: Option<RemoteRef>,
}

pub fn load(path: &Path) -> Result<LoadedWorkflow> {
    load_with_source(
        path,
        WorkflowSource::Local {
            path: path.to_string_lossy().into_owned(),
        },
        None,
    )
}

pub fn resolve_arg(arg: &str, update: bool) -> Result<(Resolved, LoadedWorkflow)> {
    let resolved = crate::remote::resolve(arg, update)?;
    let spec = resolved.remote.as_ref().map(|r| r.spec.clone());
    let loaded = load_with_source(&resolved.workflow_dir, resolved.source.clone(), spec)?;
    Ok((resolved, loaded))
}

fn load_with_source(
    path: &Path,
    mut source: WorkflowSource,
    remote_spec: Option<RemoteRef>,
) -> Result<LoadedWorkflow> {
    let abs = path
        .canonicalize()
        .with_context(|| format!("resolving workflow path `{}`", path.display()))?;
    if !abs.is_dir() {
        bail!("`{}` is not a directory", abs.display());
    }
    // Persist local sources as absolute paths. A relative path (e.g. a bare
    // `harmonize_product_sizes`) recorded in the run history can't be re-opened
    // from the launcher: its working directory differs, so the path is
    // misclassified as a remote git ref. Anchoring to the canonical path keeps
    // recents re-runnable from anywhere.
    if let WorkflowSource::Local { path: p } = &mut source {
        *p = abs.to_string_lossy().into_owned();
    }
    let manifest_path = abs.join("manifest.md");
    let manifest_metadata = std::fs::symlink_metadata(&manifest_path).ok();
    if manifest_metadata
        .as_ref()
        .is_some_and(|metadata| metadata.file_type().is_symlink())
    {
        bail!(
            "`{}` is a symlink; workflow manifests must be regular files inside the workflow folder",
            manifest_path.display()
        );
    }
    if !manifest_metadata.is_some_and(|metadata| metadata.is_file()) {
        bail!(
            "no `manifest.md` in `{}` — this does not look like a Cori workflow folder",
            abs.display()
        );
    }
    let folder_name = abs
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| anyhow!("workflow path has no folder name"))?
        .to_string();

    let content_hash = workflow_content_hash(&abs).with_context(|| "hashing workflow folder")?;
    let cache_key = cache_key(&abs, &content_hash);

    let cache_path = paths::cache_dir()?.join(format!("{cache_key}.json"));
    if let Ok(bytes) = std::fs::read(&cache_path)
        && let Ok(entry) = serde_json::from_slice::<CacheEntry>(&bytes)
        && cache_entry_is_current(&entry)
    {
        return Ok(LoadedWorkflow {
            folder_name,
            execution_root: abs.clone(),
            absolute_path: abs,
            compiled: entry.compiled,
            content_hash,
            from_cache: true,
            source,
            remote_spec,
        });
    }

    let compiled = compile(&abs).map_err(format_compile_errors)?;
    write_cache(&cache_path, &compiled)?;

    Ok(LoadedWorkflow {
        folder_name,
        execution_root: abs.clone(),
        absolute_path: abs,
        compiled,
        content_hash,
        from_cache: false,
        source,
        remote_spec,
    })
}

/// Copy the exact compiled workflow tree into an atomic, content-addressed
/// cache entry and switch activity execution to that snapshot.
///
/// This happens only after remote-workflow consent. It closes the editor
/// hash-to-import race: Deno imports and every subsequent activity read the
/// same immutable-by-construction bytes that produced `compiled`.
pub fn materialize_execution_snapshot(loaded: &mut LoadedWorkflow) -> Result<()> {
    let sources = paths::source_cache_dir()?;
    materialize_execution_snapshot_in(loaded, &sources)
}

fn materialize_execution_snapshot_in(
    loaded: &mut LoadedWorkflow,
    sources_dir: &Path,
) -> Result<()> {
    create_private_dir_all(sources_dir)
        .with_context(|| format!("creating `{}`", sources_dir.display()))?;
    let target = sources_dir.join(&loaded.content_hash);

    if target.exists() {
        validate_execution_snapshot(&target, loaded)?;
        make_tree_owner_read_only(&target)?;
        loaded.execution_root = target
            .canonicalize()
            .with_context(|| format!("resolving workflow snapshot `{}`", target.display()))?;
        return Ok(());
    }

    let partial = sources_dir.join(format!(
        ".{}.partial-{}-{}",
        loaded.content_hash,
        std::process::id(),
        Uuid::new_v4().simple()
    ));
    create_private_dir(&partial)
        .with_context(|| format!("creating workflow snapshot `{}`", partial.display()))?;

    let copied = copy_workflow_tree(&loaded.absolute_path, &partial)
        .and_then(|()| validate_execution_snapshot(&partial, loaded));
    if let Err(error) = copied {
        let _ = std::fs::remove_dir_all(&partial);
        return Err(error);
    }

    match std::fs::rename(&partial, &target) {
        Ok(()) => {}
        // Another process may have materialized the same digest between the
        // existence check and rename. Accept it only after full verification.
        Err(rename_error) if target.exists() => {
            let validation = validate_execution_snapshot(&target, loaded);
            let _ = std::fs::remove_dir_all(&partial);
            validation.with_context(|| {
                format!(
                    "workflow snapshot `{}` appeared concurrently but was invalid after rename failed: {rename_error}",
                    target.display()
                )
            })?;
        }
        Err(error) => {
            let _ = std::fs::remove_dir_all(&partial);
            return Err(error).with_context(|| {
                format!(
                    "committing workflow snapshot `{}` to `{}`",
                    partial.display(),
                    target.display()
                )
            });
        }
    }

    validate_execution_snapshot(&target, loaded)?;
    make_tree_owner_read_only(&target)?;
    loaded.execution_root = target
        .canonicalize()
        .with_context(|| format!("resolving workflow snapshot `{}`", target.display()))?;
    Ok(())
}

fn copy_workflow_tree(source: &Path, destination: &Path) -> Result<()> {
    fn include_entry(entry: &DirEntry) -> bool {
        entry.depth() == 0 || entry.file_name() != ".git"
    }

    for entry in WalkDir::new(source)
        .follow_links(false)
        .into_iter()
        .filter_entry(include_entry)
    {
        let entry =
            entry.with_context(|| format!("walking workflow source `{}`", source.display()))?;
        if entry.depth() == 0 {
            continue;
        }
        let relative = entry.path().strip_prefix(source).with_context(|| {
            format!(
                "deriving snapshot path for `{}` under `{}`",
                entry.path().display(),
                source.display()
            )
        })?;
        let target = destination.join(relative);
        let file_type = entry.file_type();
        if file_type.is_symlink() {
            bail!(
                "workflow tree contains symlink `{}`; symlinks are not allowed in executable workflow input",
                entry.path().display()
            );
        } else if file_type.is_dir() {
            create_private_dir(&target)
                .with_context(|| format!("creating snapshot directory `{}`", target.display()))?;
        } else if file_type.is_file() {
            std::fs::copy(entry.path(), &target).with_context(|| {
                format!(
                    "copying workflow file `{}` to `{}`",
                    entry.path().display(),
                    target.display()
                )
            })?;
        } else {
            bail!(
                "workflow tree contains unsupported special file `{}`",
                entry.path().display()
            );
        }
    }
    Ok(())
}

fn create_private_dir_all(path: &Path) -> std::io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    builder.mode(0o700);
    builder.create(path)?;
    #[cfg(unix)]
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    Ok(())
}

fn create_private_dir(path: &Path) -> std::io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    builder.mode(0o700);
    builder.create(path)
}

fn make_tree_owner_read_only(root: &Path) -> Result<()> {
    for entry in WalkDir::new(root).contents_first(true) {
        let entry =
            entry.with_context(|| format!("walking workflow snapshot `{}`", root.display()))?;
        let metadata = entry
            .metadata()
            .with_context(|| format!("reading snapshot metadata `{}`", entry.path().display()))?;
        #[cfg(unix)]
        let permissions =
            std::fs::Permissions::from_mode(if metadata.is_dir() { 0o500 } else { 0o400 });
        #[cfg(not(unix))]
        let permissions = {
            let mut permissions = metadata.permissions();
            permissions.set_readonly(true);
            permissions
        };
        std::fs::set_permissions(entry.path(), permissions).with_context(|| {
            format!(
                "restricting workflow snapshot permissions `{}`",
                entry.path().display()
            )
        })?;
    }
    Ok(())
}

fn validate_execution_snapshot(snapshot: &Path, loaded: &LoadedWorkflow) -> Result<()> {
    let actual_tree_hash = workflow_content_hash(snapshot)
        .with_context(|| format!("hashing workflow snapshot `{}`", snapshot.display()))?;
    if actual_tree_hash != loaded.content_hash {
        bail!(
            "workflow changed while Cori was freezing it (expected content hash {}, got {}); no activity was started",
            loaded.content_hash,
            actual_tree_hash
        );
    }

    for step in &loaded.compiled.steps {
        let expected = step.source_sha256.as_deref().ok_or_else(|| {
            anyhow!(
                "compiled step `{}` is missing its source digest",
                step.activity_id
            )
        })?;
        let step_path = snapshot.join(&step.source_path);
        let source = std::fs::read(&step_path)
            .with_context(|| format!("reading frozen step `{}`", step_path.display()))?;
        let actual = cori_compiler::source_sha256(&source);
        if actual != expected {
            bail!(
                "frozen step `{}` does not match the compiled source digest (expected {}, got {})",
                step.activity_id,
                expected,
                actual
            );
        }
    }
    Ok(())
}

pub fn loaded_run_history_key(loaded: &LoadedWorkflow) -> String {
    match &loaded.remote_spec {
        Some(spec) => crate::remote::remote_run_history_key(spec),
        None => run_history_key(&loaded.absolute_path, &loaded.folder_name),
    }
}

pub fn run_history_key(absolute_path: &Path, folder_name: &str) -> String {
    let mut h = Sha256::new();
    h.update(absolute_path.as_os_str().to_string_lossy().as_bytes());
    let digest = h.finalize();
    let short = hex::encode(&digest[..4]);
    format!("{folder_name}-{short}")
}

fn cache_key(absolute_path: &Path, content_hash: &str) -> String {
    let mut h = Sha256::new();
    h.update(absolute_path.as_os_str().to_string_lossy().as_bytes());
    h.update(content_hash.as_bytes());
    let digest = h.finalize();
    hex::encode(&digest[..6])
}

fn cache_entry_is_current(entry: &CacheEntry) -> bool {
    entry.format_version == COMPILER_CACHE_FORMAT_VERSION
        && entry
            .compiled
            .steps
            .iter()
            .all(|step| step.source_sha256.is_some())
}

fn write_cache(path: &Path, compiled: &CompiledWorkflow) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("cache path has no parent"))?;
    std::fs::create_dir_all(parent).with_context(|| format!("creating `{}`", parent.display()))?;
    let bytes = serde_json::to_vec(&CacheEntryRef {
        format_version: COMPILER_CACHE_FORMAT_VERSION,
        compiled,
    })
    .context("serializing compiled workflow cache entry")?;
    let mut tmp = parent.join(format!(
        ".tmp-{}-{}",
        std::process::id(),
        path.file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default(),
    ));
    tmp.set_extension("partial");
    std::fs::write(&tmp, &bytes).with_context(|| format!("writing `{}`", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("renaming into `{}`", path.display()))?;
    Ok(())
}

fn format_compile_errors(errors: Vec<CompileError>) -> anyhow::Error {
    let mut s = String::from("compile errors:\n");
    for e in errors {
        s.push_str(&format!("  · {e}\n"));
    }
    anyhow!(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn make_test_workflow(root: &Path) {
        std::fs::create_dir(root.join("steps")).expect("steps directory");
        std::fs::write(
            root.join("manifest.md"),
            "---\nid: cache_test\nname: Cache test\ndescription: cache envelope\ncreated: 2026-07-15\nversion: 1\ntools_required: [echo]\n---\n",
        )
        .expect("manifest");
        std::fs::write(
            root.join("steps/01_echo.ts"),
            "import { step } from \"@cori-do/sdk\";\nexport default step.cli({ description: \"echo\", command: () => [\"echo\", \"ok\"] });\n",
        )
        .expect("step");
    }

    fn loaded_without_global_cache(root: &Path) -> LoadedWorkflow {
        let absolute_path = root.canonicalize().expect("canonical workflow");
        let compiled = compile(&absolute_path).expect("compiled workflow");
        let content_hash = workflow_content_hash(&absolute_path).expect("workflow hash");
        LoadedWorkflow {
            folder_name: "cache_test".to_string(),
            execution_root: absolute_path.clone(),
            absolute_path: absolute_path.clone(),
            compiled,
            content_hash,
            from_cache: false,
            source: WorkflowSource::Local {
                path: absolute_path.display().to_string(),
            },
            remote_spec: None,
        }
    }

    #[test]
    fn cache_envelope_rejects_legacy_unversioned_compiled_workflow() {
        let temp = tempdir().expect("temporary workflow directory");
        make_test_workflow(temp.path());
        let compiled = compile(temp.path()).expect("compiled workflow");

        let legacy = serde_json::to_vec(&compiled).expect("legacy cache bytes");
        assert!(serde_json::from_slice::<CacheEntry>(&legacy).is_err());

        let current = serde_json::to_vec(&CacheEntryRef {
            format_version: COMPILER_CACHE_FORMAT_VERSION,
            compiled: &compiled,
        })
        .expect("versioned cache bytes");
        let decoded = serde_json::from_slice::<CacheEntry>(&current).expect("cache entry");
        assert_eq!(decoded.format_version, COMPILER_CACHE_FORMAT_VERSION);
        assert_eq!(decoded.compiled, compiled);
        assert!(cache_entry_is_current(&decoded));

        let mut stale = decoded;
        stale.compiled.steps[0].source_sha256 = None;
        assert!(
            !cache_entry_is_current(&stale),
            "fresh runs must not reuse cached DAGs without source digests"
        );
    }

    #[test]
    fn execution_snapshot_preserves_compiled_bytes_after_source_changes() {
        let source = tempdir().expect("temporary workflow directory");
        let cache = tempdir().expect("temporary snapshot cache");
        make_test_workflow(source.path());
        std::fs::write(source.path().join("types.ts"), "export const value = 1;\n")
            .expect("helper");
        let mut loaded = loaded_without_global_cache(source.path());

        materialize_execution_snapshot_in(&mut loaded, cache.path()).expect("snapshot");
        assert_ne!(loaded.execution_root, loaded.absolute_path);
        assert_eq!(
            workflow_content_hash(&loaded.execution_root).expect("snapshot hash"),
            loaded.content_hash
        );
        #[cfg(unix)]
        {
            assert_eq!(
                std::fs::metadata(&loaded.execution_root)
                    .expect("snapshot metadata")
                    .permissions()
                    .mode()
                    & 0o777,
                0o500
            );
            assert_eq!(
                std::fs::metadata(loaded.execution_root.join("manifest.md"))
                    .expect("snapshot file metadata")
                    .permissions()
                    .mode()
                    & 0o777,
                0o400
            );
        }

        std::fs::write(source.path().join("types.ts"), "export const value = 2;\n")
            .expect("mutate source");
        assert_eq!(
            std::fs::read_to_string(loaded.execution_root.join("types.ts")).expect("frozen helper"),
            "export const value = 1;\n"
        );
    }

    #[test]
    fn execution_snapshot_rejects_corrupt_existing_cache_entry() {
        let source = tempdir().expect("temporary workflow directory");
        let cache = tempdir().expect("temporary snapshot cache");
        make_test_workflow(source.path());
        let mut loaded = loaded_without_global_cache(source.path());

        materialize_execution_snapshot_in(&mut loaded, cache.path()).expect("snapshot");
        let frozen_step = loaded.execution_root.join("steps/01_echo.ts");
        #[cfg(unix)]
        std::fs::set_permissions(&frozen_step, std::fs::Permissions::from_mode(0o600))
            .expect("make corruption fixture writable");
        #[cfg(not(unix))]
        {
            let mut permissions = std::fs::metadata(&frozen_step)
                .expect("frozen step metadata")
                .permissions();
            permissions.set_readonly(false);
            std::fs::set_permissions(&frozen_step, permissions)
                .expect("make corruption fixture writable");
        }
        std::fs::write(&frozen_step, "export default {};\n").expect("corrupt snapshot");

        let mut second = loaded_without_global_cache(source.path());
        let error = materialize_execution_snapshot_in(&mut second, cache.path())
            .expect_err("corrupt content-addressed cache must fail closed");
        assert!(error.to_string().contains("expected content hash"));
    }
}
