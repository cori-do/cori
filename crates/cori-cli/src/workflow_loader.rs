//! Resolves a workflow folder, compiles it (cache hit or miss), and
//! produces the run-history key. Pure functions, no shared state.
//!
//! The cache lives at `~/.cori/cache/<key>.json` where
//! `key = sha256(absolute_path + content_hash_of_folder)`, truncated to
//! 12 hex chars. Cache is automatically invalidated when any file in
//! the folder changes; path is folded in so two repos containing a
//! same-named workflow don't collide.
//!
//! The run-history key is `<folder_name>-<short_hash(absolute_path)>`
//! (8 hex chars). Path-only — survives across `cori run` invocations
//! of unchanged source.
//!
//! Garbage collection of stale cache entries is a TODO for later.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use cori_compiler::{CompileError, compile};
use cori_protocol::CompiledWorkflow;
use sha2::{Digest, Sha256};

use crate::paths;
use crate::remote::{self, RemoteRef, Resolved, WorkflowSource};

/// Outcome of [`load`]: the compiled DAG and a few derived strings the
/// run pipeline reuses.
pub struct LoadedWorkflow {
    /// Folder name == workflow id (e.g. `hello_world`).
    pub folder_name: String,
    /// Absolute, canonicalised source path.
    pub absolute_path: PathBuf,
    /// Compiled DAG.
    pub compiled: CompiledWorkflow,
    /// 16-hex-char content hash of `manifest.md` + every `steps/*` file.
    /// Carried into the run trace as `workflow_content_hash`.
    pub content_hash: String,
    /// True when the compiled DAG was loaded from `~/.cori/cache/`.
    pub from_cache: bool,
    /// Where this workflow came from (local path vs remote git ref).
    /// Recorded in the run trace's `source` field. Phase 8.
    pub source: WorkflowSource,
    /// `Some(spec)` for remote workflows; drives the run-history key
    /// override and consent gating. `None` for local workflows.
    pub remote_spec: Option<RemoteRef>,
}

/// Resolve `path` to a workflow folder, hit the cache or recompile,
/// and return the result. Writes nothing to the workflow folder
/// itself.
pub fn load(path: &Path) -> Result<LoadedWorkflow> {
    load_with_source(
        path,
        WorkflowSource::Local {
            path: path.to_string_lossy().into_owned(),
        },
        None,
    )
}

/// Resolve a `cori run` argument string, classifying it as a local
/// path or remote git ref (which is fetched into the cache before
/// loading). The `update` flag triggers re-resolution of mutable
/// remote refs. Local args ignore both flags except via classification.
pub fn resolve_arg(arg: &str, update: bool) -> Result<(Resolved, LoadedWorkflow)> {
    let resolved = remote::resolve(arg, update)?;
    let spec = resolved.remote.as_ref().map(|r| r.spec.clone());
    let loaded = load_with_source(&resolved.workflow_dir, resolved.source.clone(), spec)?;
    Ok((resolved, loaded))
}

fn load_with_source(
    path: &Path,
    source: WorkflowSource,
    remote_spec: Option<RemoteRef>,
) -> Result<LoadedWorkflow> {
    let abs = path
        .canonicalize()
        .with_context(|| format!("resolving workflow path `{}`", path.display()))?;
    if !abs.is_dir() {
        bail!("`{}` is not a directory", abs.display());
    }
    let manifest_path = abs.join("manifest.md");
    if !manifest_path.is_file() {
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

    let content_hash = hash_folder(&abs).with_context(|| "hashing workflow folder")?;
    let cache_key = cache_key(&abs, &content_hash);

    let cache_path = paths::cache_dir()?.join(format!("{cache_key}.json"));
    if let Ok(bytes) = std::fs::read(&cache_path)
        && let Ok(compiled) = serde_json::from_slice::<CompiledWorkflow>(&bytes)
    {
        return Ok(LoadedWorkflow {
            folder_name,
            absolute_path: abs,
            compiled,
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
        absolute_path: abs,
        compiled,
        content_hash,
        from_cache: false,
        source,
        remote_spec,
    })
}

/// Run-history key — `<folder_name>-<short_hash(absolute_resolved_path)>`
/// for local workflows, or the remote-specific key
/// `<repo_leaf>-<short_hash(host/repo//subpath)>` (ignoring `@ref`) for
/// remote ones.
pub fn loaded_run_history_key(loaded: &LoadedWorkflow) -> String {
    match &loaded.remote_spec {
        Some(spec) => remote::remote_run_history_key(spec),
        None => run_history_key(&loaded.absolute_path, &loaded.folder_name),
    }
}

/// `<folder_name>-<short_hash(absolute_resolved_path)>`. 8 hex chars
/// from the path; survives across runs of unchanged source. Moving the
/// folder yields a fresh run-history dir (intentional).
pub fn run_history_key(absolute_path: &Path, folder_name: &str) -> String {
    let mut h = Sha256::new();
    h.update(absolute_path.as_os_str().to_string_lossy().as_bytes());
    let digest = h.finalize();
    let short = hex::encode(&digest[..4]); // 8 hex chars
    format!("{folder_name}-{short}")
}

/// 12-hex-char cache key: sha256(absolute_path + folder content hash).
fn cache_key(absolute_path: &Path, content_hash: &str) -> String {
    let mut h = Sha256::new();
    h.update(absolute_path.as_os_str().to_string_lossy().as_bytes());
    h.update(content_hash.as_bytes());
    let digest = h.finalize();
    hex::encode(&digest[..6]) // 12 hex chars
}

/// sha256 of `manifest.md` plus every `steps/*` file, byte-for-byte,
/// sorted by filename. 16 hex chars.
fn hash_folder(root: &Path) -> Result<String> {
    let mut h = Sha256::new();
    let manifest = std::fs::read(root.join("manifest.md"))
        .with_context(|| format!("reading `{}/manifest.md`", root.display()))?;
    h.update(b"manifest.md\0");
    h.update(&manifest);

    let steps_dir = root.join("steps");
    if steps_dir.is_dir() {
        let mut entries: Vec<(String, PathBuf)> = std::fs::read_dir(&steps_dir)
            .with_context(|| format!("reading `{}`", steps_dir.display()))?
            .flatten()
            .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
            .map(|e| (e.file_name().to_string_lossy().into_owned(), e.path()))
            .collect();
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        for (name, path) in entries {
            let contents =
                std::fs::read(&path).with_context(|| format!("reading `{}`", path.display()))?;
            h.update(format!("steps/{name}\0").as_bytes());
            h.update(&contents);
        }
    }
    Ok(hex::encode(&h.finalize()[..8])) // 16 hex chars
}

fn write_cache(path: &Path, compiled: &CompiledWorkflow) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("cache path has no parent"))?;
    std::fs::create_dir_all(parent).with_context(|| format!("creating `{}`", parent.display()))?;
    let bytes = serde_json::to_vec(compiled).context("serializing compiled workflow")?;
    // tempfile + rename for atomic write.
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
