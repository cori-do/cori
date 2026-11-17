// SPDX-License-Identifier: Apache-2.0
//! Sandboxed Deno runner processes, one per run of a workflow version: the steps of a run share one, so a module's state
//! (a global, a cache, a patched builtin) never reaches another run, another user, or another app.
//!
//! Starting one (Deno, the runner and its schema library) takes tens of milliseconds, more under gVisor. A process that
//! serves runs for a while (a worker, a sandbox: `keep_spares`) keeps one started ahead for each workflow it ran lately,
//! and the next run of that workflow takes it. A spare has run no step's code, so the run starts as fresh as in a process
//! of its own.
use anyhow::{Context, Result, anyhow};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex;

pub struct Runner { child: Child, stdin: ChildStdin, stdout: BufReader<ChildStdout>, next_id: u64, root: PathBuf, nonce: String }

struct Pooled { runner: Arc<Mutex<Runner>>, used: Instant }

static POOL: StdMutex<Option<HashMap<String, Pooled>>> = StdMutex::new(None);

/// A runner started for the next run of a workflow (by its folder): on its way, or ready since `at`.
enum Spare { Starting, Ready { runner: Box<Runner>, at: Instant } }

static SPARES: StdMutex<Option<HashMap<PathBuf, Spare>>> = StdMutex::new(None);

/// Counts `stop_all` calls: a spare started before one is not kept.
static EPOCH: AtomicU64 = AtomicU64::new(0);

static KEEP_SPARES: AtomicBool = AtomicBool::new(false);

/// This process serves runs for a while (a worker, a sandbox): it keeps a runner started ahead for each workflow it ran
/// lately. A one-off run (`cori run`) does not.
pub fn keep_spares() { KEEP_SPARES.store(true, Ordering::Relaxed); }

/// A run's runner is stopped once no step of the run has used it for this long: the run ended, or moved to another worker.
/// A spare goes once its workflow has had no run for as long.
const IDLE: Duration = Duration::from_secs(60);

/// Drops the runners idle for `IDLE` that no call holds; dropping the last handle kills the process (`kill_on_drop`).
fn sweep(pool: &mut HashMap<String, Pooled>, now: Instant) {
    pool.retain(|_, p| now.duration_since(p.used) < IDLE || Arc::strong_count(&p.runner) > 1);
}

/// Drops the spares that waited `IDLE` for a run.
fn sweep_spares(spares: &mut HashMap<PathBuf, Spare>, now: Instant) {
    spares.retain(|_, s| match s { Spare::Starting => true, Spare::Ready { at, .. } => now.duration_since(*at) < IDLE });
}

/// Stops every runner of this process, the spares too (a sandbox starting over: no call is running). A spare on its way
/// is stopped once it is up.
pub fn stop_all() {
    EPOCH.fetch_add(1, Ordering::SeqCst);
    SPARES.lock().unwrap().take();
    POOL.lock().unwrap().take();
}

/// `runner.ts`, from the runtime files embedded in the binary (`cori_compiler::runtime`).
pub fn runner_script() -> Result<PathBuf> { Ok(cori_compiler::runtime::runtime().map_err(|e| anyhow!(e))?.runner.clone()) }

/// How long one runner call (a step's code, bind, parse…) may take before the process is killed and replaced:
/// `CORI_STEP_TIMEOUT_SECS`, default 120. A sandbox holds a `cli` step's program to it too.
pub fn call_timeout() -> std::time::Duration {
    std::time::Duration::from_secs(std::env::var("CORI_STEP_TIMEOUT_SECS").ok().and_then(|v| v.parse().ok()).filter(|s: &u64| *s > 0).unwrap_or(120))
}

/// The runner of `run` for the workflow at `root`, from its first step on: the workflow's spare when one is ready, else
/// one started now. Idle runners are swept on every call and once a minute, so a quiet worker does not keep the processes
/// of finished runs.
pub async fn runner_for(root: &Path, run: &str) -> Result<Arc<Mutex<Runner>>> {
    static SWEEPER: std::sync::Once = std::sync::Once::new();
    SWEEPER.call_once(|| { tokio::spawn(async { loop {
        tokio::time::sleep(IDLE).await;
        let now = Instant::now();
        if let Some(p) = POOL.lock().unwrap().as_mut() { sweep(p, now); }
        if let Some(s) = SPARES.lock().unwrap().as_mut() { sweep_spares(s, now); }
    } }); });
    let key = format!("{}\u{0}{run}", root.display());
    {
        let mut pool = POOL.lock().unwrap();
        let pool = pool.get_or_insert_with(HashMap::new);
        let now = Instant::now();
        sweep(pool, now);
        if let Some(p) = pool.get_mut(&key) { p.used = now; return Ok(p.runner.clone()); }
    }
    let runner = match take_spare(root) { Some(r) => r, None => spawn(root).await? };
    let r = Arc::new(Mutex::new(runner));
    POOL.lock().unwrap().get_or_insert_with(HashMap::new).insert(key, Pooled { runner: r.clone(), used: Instant::now() });
    // the next run's, ahead of it
    if KEEP_SPARES.load(Ordering::Relaxed) { start_spare(root); }
    Ok(r)
}

/// The workflow's spare when it is up (one still starting is left to come up for the run after).
fn take_spare(root: &Path) -> Option<Runner> {
    let mut spares = SPARES.lock().unwrap();
    let spares = spares.as_mut()?;
    match spares.remove(root)? {
        Spare::Ready { runner, .. } => { tracing::debug!(root = %root.display(), "runner: the workflow's spare taken"); Some(*runner) }
        starting => { spares.insert(root.to_path_buf(), starting); None }
    }
}

/// Starts a spare for the workflow at `root` unless one is ready or on its way. It is kept only if no `stop_all` came
/// meanwhile, so that what a sandbox stopped when it started over stays stopped.
fn start_spare(root: &Path) {
    let epoch = EPOCH.load(Ordering::SeqCst);
    {
        let mut spares = SPARES.lock().unwrap();
        let spares = spares.get_or_insert_with(HashMap::new);
        if spares.contains_key(root) { return; }
        spares.insert(root.to_path_buf(), Spare::Starting);
    }
    let root = root.to_path_buf();
    tokio::spawn(async move {
        let started = tokio::time::timeout(call_timeout(), spawn(&root)).await;
        let mut guard = SPARES.lock().unwrap();
        let spares = guard.get_or_insert_with(HashMap::new);
        let ours = EPOCH.load(Ordering::SeqCst) == epoch && matches!(spares.get(&root), Some(Spare::Starting));
        match started {
            Ok(Ok(runner)) if ours => { spares.insert(root, Spare::Ready { runner: Box::new(runner), at: Instant::now() }); }
            // not up, or no longer wanted: dropping it stops it
            other => {
                if ours { spares.remove(&root); }
                if let Ok(Err(e)) = &other { tracing::debug!(root = %root.display(), error = %e, "runner: no spare"); }
            }
        }
    });
}

/// Caches the runner's own dependencies (zod) once per process, with network access, so the sandboxed runner can run
/// with `--cached-only` and never fetch a module a step names. A no-op when the cache is warm (the container image).
///
/// Without the lockfile, as the runner resolves them (a workflow's config has none): it reads zod's registry entry, and
/// Deno installing from a lockfile asks for that entry without waiting for it, so it may exit before the entry is saved.
async fn precache() -> Result<()> {
    static DONE: tokio::sync::OnceCell<()> = tokio::sync::OnceCell::const_new();
    DONE.get_or_try_init(|| async {
        let rt = cori_compiler::runtime::runtime().map_err(|e| anyhow!(e))?;
        let mut cmd = Command::new("deno");
        crate::events::scrub_env(&mut cmd);
        cmd.arg("cache").arg("--quiet").arg("--no-lock").arg("--config").arg(rt.runner_dir.join("deno.json")).arg(&rt.runner).arg(&rt.sdk_index).kill_on_drop(true);
        let out = tokio::time::timeout(std::time::Duration::from_secs(300), cmd.output()).await
            .map_err(|_| anyhow!("RuntimeUnavailableError: caching the runner's dependencies took over 5 minutes"))?
            .map_err(|e| match cori_compiler::runtime::deno_not_found(&e) { Some(m) => anyhow!("RuntimeUnavailableError: {m}"), None => anyhow!(e).context("deno cache") })?;
        if !out.status.success() { return Err(anyhow!("RuntimeUnavailableError: caching the runner's dependencies failed: {}", String::from_utf8_lossy(&out.stderr).trim())); }
        Ok(())
    }).await.map(|_| ())
}

async fn spawn(root: &Path) -> Result<Runner> {
    precache().await?;
    let script = runner_script().context("runner.ts")?;
    let mut cmd = Command::new("deno");
    crate::events::scrub_env(&mut cmd);
    // read access to the workflow, its app and the runtime only; no net, env, write, run or ffi; a heap cap; and no module
    // fetched at run time at all (`--cached-only`: the precache above holds the runner's own)
    let mut args = cori_compiler::runtime::sandbox_run_args(root).map_err(|e| anyhow!(e))?;
    args.insert(1, "--cached-only".into());
    cmd.args(args)
        .arg(&script)
        .stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::inherit())
        .kill_on_drop(true);
    let mut child = cmd.spawn().context("spawning deno runner")?;
    let mut stdin = child.stdin.take().unwrap();
    let stdout = BufReader::new(child.stdout.take().unwrap());
    // the nonce every response starts with, sent before any step is loaded (see runner.ts)
    let nonce = format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple());
    stdin.write_all(format!("{}\n", json!({ "nonce": nonce })).as_bytes()).await.context("runner stdin")?;
    let mut r = Runner { child, stdin, stdout, next_id: 1, root: root.to_path_buf(), nonce };
    // ping loads the runner itself as a "step" and fails on purpose; we only need the process alive, not one that exited
    // (a module missing from Deno's cache): that one would be kept as a spare
    if let Err(e) = r.exchange(json!({ "op": "ping", "file": script.display().to_string() })).await
        && e.to_string().starts_with("RuntimeUnavailableError") { return Err(e); }
    Ok(r)
}

impl Runner {
    /// One request, answered within `CORI_STEP_TIMEOUT_SECS`. A runner that timed out or died is replaced by a fresh
    /// process (the next call finds it ready); the call itself fails with `Timeout` or `RuntimeUnavailableError`.
    pub async fn call(&mut self, req: Value) -> Result<Value> {
        if matches!(self.child.try_wait(), Ok(Some(_))) { *self = spawn(&self.root).await?; }
        let limit = call_timeout();
        match tokio::time::timeout(limit, self.exchange(req)).await {
            Ok(Ok(v)) => Ok(v),
            Ok(Err(e)) => {
                if e.to_string().starts_with("RuntimeUnavailableError") { let root = self.root.clone(); let _ = self.child.kill().await; *self = spawn(&root).await?; }
                Err(e)
            }
            Err(_) => {
                let root = self.root.clone();
                let _ = self.child.kill().await;
                *self = spawn(&root).await?;
                Err(anyhow!("Timeout: the step's code did not answer within {}s (CORI_STEP_TIMEOUT_SECS); its runner was restarted", limit.as_secs()))
            }
        }
    }

    async fn exchange(&mut self, mut req: Value) -> Result<Value> {
        let id = self.next_id; self.next_id += 1;
        req["id"] = json!(id);
        let line = serde_json::to_string(&req)? + "\n";
        self.stdin.write_all(line.as_bytes()).await.map_err(|e| anyhow!("RuntimeUnavailableError: runner stdin: {e}"))?;
        self.stdin.flush().await.map_err(|e| anyhow!("RuntimeUnavailableError: runner stdin: {e}"))?;
        let prefix = format!("{} ", self.nonce);
        loop {
            let mut buf = String::new();
            let n = self.stdout.read_line(&mut buf).await.map_err(|e| anyhow!("RuntimeUnavailableError: runner stdout: {e}"))?;
            if n == 0 { let _ = self.child.kill().await; return Err(anyhow!("RuntimeUnavailableError: deno runner exited")); }
            // anything without the nonce was printed by step code, not by the runner
            let Some(body) = buf.trim_end().strip_prefix(&prefix) else { continue };
            let v: Value = match serde_json::from_str(body) { Ok(v) => v, Err(_) => continue };
            if v.get("id").and_then(|i| i.as_u64()) != Some(id) { continue; }
            if v.get("ok").and_then(|b| b.as_bool()) == Some(true) { return Ok(v); }
            let ty = v.get("type").and_then(|t| t.as_str()).unwrap_or("StepFailedError");
            let msg = v.get("error").and_then(|t| t.as_str()).unwrap_or("runner error");
            return Err(anyhow!("{ty}: {msg}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A workflow folder whose one step answers `{ ok: true }`, with the config the runner reads (`.cori/deno.json`).
    fn workflow() -> PathBuf {
        let app = std::env::temp_dir().join(format!("cori-runner-{}", uuid::Uuid::new_v4().simple()));
        let wf = app.join("workflows/w");
        std::fs::create_dir_all(wf.join("steps")).unwrap();
        std::fs::create_dir_all(wf.join(".cori")).unwrap();
        std::fs::write(app.join("cori.toml"), "[app]\nid = \"a\"\n").unwrap();
        std::fs::write(wf.join("steps/s.ts"), "import { step } from \"@cori-do/sdk\";\nimport { z } from \"zod\";\nexport default step.code({ name: \"s\", description: \"s\", input: () => ({}), output: z.object({ ok: z.boolean() }), run: () => ({ ok: true }) });\n").unwrap();
        let sdk = cori_compiler::runtime::runtime().unwrap().sdk_index.display().to_string();
        std::fs::write(wf.join(".cori/deno.json"), json!({ "imports": { "@cori-do/sdk": format!("file://{sdk}"), "zod": "npm:zod@4.4.3" }, "unstable": ["sloppy-imports"], "nodeModulesDir": "none" }).to_string()).unwrap();
        wf.canonicalize().unwrap()
    }

    /// The process id of the workflow's spare once it is up.
    async fn spare_of(wf: &Path) -> u32 {
        for _ in 0..600 {
            if let Some(Spare::Ready { runner, .. }) = SPARES.lock().unwrap().as_ref().and_then(|s| s.get(wf)) { return runner.child.id().unwrap(); }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("no spare came up for {}", wf.display());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_run_takes_the_spare_started_for_its_workflow_and_none_outlives_stop_all() {
        if std::process::Command::new("deno").arg("--version").output().is_err() { eprintln!("deno is not on the PATH: skipped"); return; }
        let wf = workflow();
        keep_spares();
        let a = runner_for(&wf, "run-a").await.unwrap();
        let spare = spare_of(&wf).await;
        let b = runner_for(&wf, "run-b").await.unwrap();
        assert_eq!(b.lock().await.child.id(), Some(spare), "the next run took the spare");
        assert_ne!(a.lock().await.child.id(), b.lock().await.child.id(), "each run has a process of its own");
        // a spare serves its run as a runner started for it would: the step's code loads in it now
        let file = wf.join("steps/s.ts").display().to_string();
        let v = b.lock().await.call(json!({ "op": "code", "file": file, "name": "s", "input": {} })).await.unwrap();
        assert_eq!(v["output"], json!({ "ok": true }));
        // the run after's spare is on its way: what `stop_all` stops stays stopped, even a spare that comes up after it
        stop_all();
        for _ in 0..600 {
            if let Some(s) = SPARES.lock().unwrap().as_ref() { assert!(s.get(&wf).is_none(), "a spare started before stop_all was kept"); break; }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let _ = std::fs::remove_dir_all(wf.parent().and_then(Path::parent).unwrap());
    }
}
