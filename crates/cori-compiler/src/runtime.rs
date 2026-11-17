// SPDX-License-Identifier: Apache-2.0
//! The TypeScript the binary runs under Deno (the runner, the schema exporter and the SDK), embedded at build time and
//! written once per content hash under the cache directory, so a released binary or a container image finds them
//! without a source checkout. `CORI_RUNTIME_DIR` (a folder with `runner/` and `sdk/`, e.g. `packages/`) overrides the
//! embedded copy while working on the runner; `CORI_SDK_PATH` still overrides the SDK entry point alone.
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// (path under the runtime folder, content)
const FILES: &[(&str, &str)] = &[
    ("runner/deno.json", include_str!("../../../packages/runner/deno.json")),
    ("runner/deno.lock", include_str!("../../../packages/runner/deno.lock")),
    ("runner/runner.ts", include_str!("../../../packages/runner/runner.ts")),
    ("runner/schema.ts", include_str!("../../../packages/runner/schema.ts")),
    ("runner/schemas.ts", include_str!("../../../packages/runner/schemas.ts")),
    ("sdk/deno.json", include_str!("../../../packages/sdk/deno.json")),
    ("sdk/deno.lock", include_str!("../../../packages/sdk/deno.lock")),
    ("sdk/index.ts", include_str!("../../../packages/sdk/index.ts")),
];

/// Where the runtime files are on disk.
#[derive(Debug, Clone)]
pub struct Runtime {
    /// `runner.ts`, the long-lived step evaluator.
    pub runner: PathBuf,
    /// `schemas.ts`, the output-schema exporter used by `check`.
    pub schemas: PathBuf,
    /// The folder of both (read access for Deno).
    pub runner_dir: PathBuf,
    /// The SDK entry point (`index.ts`), the target of `@cori-do/sdk` in generated `deno.json` files.
    pub sdk_index: PathBuf,
    /// The folder of the SDK (read access for Deno).
    pub sdk_dir: PathBuf,
}

static RUNTIME: OnceLock<Result<Runtime, String>> = OnceLock::new();

/// What to say when `deno` could not be started because it is not installed; `None` for any other failure.
pub fn deno_not_found(e: &std::io::Error) -> Option<String> {
    (e.kind() == std::io::ErrorKind::NotFound).then(|| "Deno 2 is not on the PATH: `cori check`, `cori run` and `cori gen client` run the steps' TypeScript with it. Install it (macOS: `brew install deno`; macOS and Linux: `curl -fsSL https://deno.land/install.sh | sh`; https://deno.com), then try again".to_string())
}

/// The runtime files, resolved once per process.
pub fn runtime() -> Result<&'static Runtime, String> {
    RUNTIME.get_or_init(resolve).as_ref().map_err(Clone::clone)
}

fn resolve() -> Result<Runtime, String> {
    let root = match std::env::var_os("CORI_RUNTIME_DIR").filter(|v| !v.is_empty()) {
        Some(dir) => PathBuf::from(dir),
        None => materialize(&cache_root())?,
    };
    let root = root.canonicalize().map_err(|e| format!("cori runtime folder {}: {e}", root.display()))?;
    let runner_dir = root.join("runner");
    let (sdk_index, sdk_dir) = match std::env::var("CORI_SDK_PATH").ok().filter(|v| !v.is_empty()) {
        Some(p) => {
            let p = PathBuf::from(p.trim_start_matches("file://"));
            let p = p.canonicalize().map_err(|e| format!("CORI_SDK_PATH {}: {e}", p.display()))?;
            let dir = p.parent().map(Path::to_path_buf).unwrap_or_else(|| p.clone());
            (p, dir)
        }
        None => (root.join("sdk/index.ts"), root.join("sdk")),
    };
    let rt = Runtime { runner: runner_dir.join("runner.ts"), schemas: runner_dir.join("schemas.ts"), runner_dir, sdk_index, sdk_dir };
    for p in [&rt.runner, &rt.schemas, &rt.sdk_index] {
        if !p.is_file() { return Err(format!("cori runtime file missing: {}", p.display())); }
    }
    Ok(rt)
}

/// Cori's own environment variables (`CORI_*`: the worker token and pool key, under `cori up` the server's master key and
/// database URLs): removed from the environment of every process Cori starts (deno, `cli` binaries, stdio MCP servers).
pub fn cori_env_vars() -> Vec<std::ffi::OsString> {
    std::env::vars_os().map(|(k, _)| k).filter(|k| k.to_string_lossy().starts_with("CORI_")).collect()
}

/// V8 heap limit of a deno process running workflow code, in MB: `CORI_RUNNER_MEMORY_MB`, default 512.
pub fn deno_memory_mb() -> u64 {
    std::env::var("CORI_RUNNER_MEMORY_MB").ok().and_then(|v| v.parse().ok()).filter(|m: &u64| *m >= 64).unwrap_or(512)
}

/// The `deno run` arguments for workflow code (the step runner, the output schema export), before the script: read
/// access to the workflow, its app (the nearest folder with cori.toml: steps may import its `lib/`) and Cori's runtime,
/// no other permission, no remote module (npm packages may still download: the runner adds `--cached-only`), a heap
/// cap, the workflow's generated config.
pub fn sandbox_run_args(workflow_dir: &Path) -> Result<Vec<String>, String> {
    let rt = runtime()?;
    let app_root = crate::app_root_of(workflow_dir);
    Ok(vec![
        "run".into(), "--quiet".into(), "--no-prompt".into(), "--no-remote".into(),
        format!("--v8-flags=--max-old-space-size={}", deno_memory_mb()),
        format!("--allow-read={},{},{},{}", workflow_dir.display(), app_root.display(), rt.sdk_dir.display(), rt.runner_dir.display()),
        "--config".into(), workflow_dir.join(".cori").join("deno.json").display().to_string(),
    ])
}

/// `CORI_CACHE_DIR`, else the platform cache folder (`~/Library/Caches/cori`, `~/.cache/cori`), else the temp folder.
fn cache_root() -> PathBuf {
    if let Some(d) = std::env::var_os("CORI_CACHE_DIR").filter(|v| !v.is_empty()) { return PathBuf::from(d); }
    dirs::cache_dir().map(|d| d.join("cori")).unwrap_or_else(|| std::env::temp_dir().join("cori"))
}

/// A short hash of every embedded file (names and contents): the folder name, so two binaries never share a stale copy.
pub fn content_hash() -> String {
    let mut h = Sha256::new();
    for (name, body) in FILES { h.update(name.as_bytes()); h.update([0]); h.update(body.as_bytes()); h.update([0]); }
    hex::encode(h.finalize())[..16].to_string()
}

/// Writes the runtime folder again from the embedded files, and nothing else in it: a sandbox starting over
/// (`cori sandbox serve`), where the programs of a previous lease could write it. A no-op with `CORI_RUNTIME_DIR`.
pub fn restore() -> Result<(), String> {
    if std::env::var_os("CORI_RUNTIME_DIR").filter(|v| !v.is_empty()).is_some() { return Ok(()); }
    let root = runtime()?.runner_dir.parent().map(Path::to_path_buf).ok_or("cori runtime folder")?;
    let _ = std::fs::remove_dir_all(&root);
    let write = || -> std::io::Result<()> {
        for (name, body) in FILES {
            let p = root.join(name);
            if let Some(parent) = p.parent() { std::fs::create_dir_all(parent)?; }
            std::fs::write(&p, body)?;
        }
        std::fs::write(root.join(".complete"), content_hash())
    };
    write().map_err(|e| format!("cori runtime cache {}: {e}", root.display()))
}

/// Writes the embedded files to `<cache>/runtime/<hash>/` unless a complete copy is there. The copy is written to a
/// temporary sibling and renamed into place, so concurrent processes (server, workers, cli) never read a partial one.
fn materialize(cache: &Path) -> Result<PathBuf, String> {
    let base = cache.join("runtime");
    let dest = base.join(content_hash());
    if dest.join(".complete").is_file() { return Ok(dest); }
    std::fs::create_dir_all(&base).map_err(|e| format!("cori runtime cache {}: {e}", base.display()))?;
    let tmp = base.join(format!(".tmp-{}-{}", std::process::id(), content_hash()));
    let _ = std::fs::remove_dir_all(&tmp);
    let write = || -> std::io::Result<()> {
        for (name, body) in FILES {
            let p = tmp.join(name);
            if let Some(parent) = p.parent() { std::fs::create_dir_all(parent)?; }
            std::fs::write(&p, body)?;
        }
        std::fs::write(tmp.join(".complete"), content_hash())
    };
    write().map_err(|e| format!("cori runtime cache {}: {e}", tmp.display()))?;
    // a copy without its marker was left by an interrupted write
    if dest.exists() && !dest.join(".complete").is_file() { let _ = std::fs::remove_dir_all(&dest); }
    match std::fs::rename(&tmp, &dest) {
        Ok(()) => Ok(dest),
        // another process won the race: its copy is identical
        Err(_) if dest.join(".complete").is_file() => { let _ = std::fs::remove_dir_all(&tmp); Ok(dest) }
        Err(e) => { let _ = std::fs::remove_dir_all(&tmp); Err(format!("cori runtime cache {}: {e}", dest.display())) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn materializes_once_per_hash() {
        let dir = std::env::temp_dir().join(format!("cori-runtime-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let a = materialize(&dir).unwrap();
        assert!(a.join("runner/runner.ts").is_file() && a.join("runner/schemas.ts").is_file() && a.join("sdk/index.ts").is_file());
        assert_eq!(a.file_name().unwrap().to_str().unwrap(), content_hash());
        // a second call reuses the complete copy
        assert_eq!(materialize(&dir).unwrap(), a);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
