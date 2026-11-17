// SPDX-License-Identifier: Apache-2.0
//! `cori sandbox serve`: a sandbox (`sandbox`), the process builders' code runs in when a cluster has sandboxes. It holds
//! no credential. A worker or the server sends it a bundle (`/v1/bundle`) and each call (`/v1/runner`: the run's Deno
//! runner; `/v1/exec`: a `cli` step's program; `/v1/mcp`: a request to a stdio MCP server; `/v1/check`: the deploy-time
//! type check and output schemas), all for one lease: an app of one organization (`x-cori-lease`). Callers prove
//! themselves with `CORI_SANDBOX_TOKEN`. In a cluster it runs in a pod of its own under gVisor, reached by the workers and
//! the server only, reaching nothing, with a read-only root: its work folder is the only place it writes.
//!
//! Between leases it starts over: once no call has come for `CORI_SANDBOX_IDLE_SECS` (default 30), or its lease is older
//! than `CORI_SANDBOX_LEASE_MAX_SECS` (default 3600) and no call is running, it stops every process its calls started
//! (`contained`, in a container of its own: every process of its user there, detached or not), empties its work folder
//! and forgets its lease. The next call may then come from another app or organization.
use anyhow::{Result, anyhow};
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{Value, json};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

/// How the sandbox runs (`cori sandbox serve`).
#[derive(Clone, Debug)]
pub struct SandboxOptions {
    pub listen: String,
    /// What callers must present (`CORI_SANDBOX_TOKEN`).
    pub token: String,
    /// The only folder it writes: bundles and runs' folders, emptied between leases.
    pub work: PathBuf,
    /// In a container of its own (`--contained`): starting over stops every process of its user, not only those it knows.
    pub contained: bool,
    pub idle: Duration,
    pub max_lease: Duration,
    /// The Deno cache whose packages (`npm/`) the work folder's own Deno cache links to (in a cluster, the image's,
    /// read-only); none: Deno's cache is left where it is.
    pub deno_packages: Option<PathBuf>,
}

impl SandboxOptions {
    pub fn from_env(listen: String, work: Option<PathBuf>, contained: bool) -> Self {
        let secs = |var: &str, d: u64| Duration::from_secs(std::env::var(var).ok().and_then(|v| v.parse().ok()).filter(|s: &u64| *s > 0).unwrap_or(d));
        Self {
            listen, token: std::env::var("CORI_SANDBOX_TOKEN").unwrap_or_default(),
            work: work.unwrap_or_else(|| std::env::temp_dir().join(format!("cori-sandbox-{}", std::process::id()))),
            contained, idle: secs("CORI_SANDBOX_IDLE_SECS", 30), max_lease: secs("CORI_SANDBOX_LEASE_MAX_SECS", 3600), deno_packages: None,
        }
    }
}

struct Lease { id: String, since: Instant }

struct Sandbox {
    opts: SandboxOptions,
    /// Every call holds it for reading; starting over takes it for writing, after the last call and before the next.
    gate: RwLock<()>,
    state: StdMutex<(Option<Lease>, usize, Instant)>,
}

/// Serves until the process stops. The process's own environment points its home, temporary and cache folders into the
/// work folder (`cori sandbox serve` sets them): what a call writes is emptied with it.
pub async fn serve(opts: SandboxOptions) -> Result<()> {
    if opts.token.is_empty() { anyhow::bail!("CORI_SANDBOX_TOKEN is not set: a sandbox answers only callers that present it"); }
    if opts.contained && !in_own_container() { anyhow::bail!("--contained stops every process of this user when the sandbox starts over: only in a container of its own (Linux, under an init such as tini)"); }
    // its memory may still hold what earlier leases sent it, and its environment holds its token: the programs it runs, of
    // the same user, may not read them (/proc/<pid>/mem, environ) nor trace it
    #[cfg(target_os = "linux")]
    unsafe { libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0); }
    std::fs::create_dir_all(&opts.work)?;
    crate::runner::keep_spares();
    let sb = Arc::new(Sandbox { opts: opts.clone(), gate: RwLock::new(()), state: StdMutex::new((None, 0, Instant::now())) });
    start_over(&sb.opts).await;
    { let sb = sb.clone(); tokio::spawn(async move { retire_loop(sb).await }); }
    let app = Router::new()
        .route("/v1/lease", post(|| async { ok(json!({})) }))
        .route("/v1/bundle", post(bundle))
        .route("/v1/runner", post(runner))
        .route("/v1/exec", post(exec))
        .route("/v1/mcp", post(mcp))
        .route("/v1/check", post(check))
        .route("/v1/check-app", post(check_app))
        .route_layer(axum::middleware::from_fn_with_state(sb.clone(), guard))
        .route("/healthz", get(|| async { "ok" }))
        .layer(axum::extract::DefaultBodyLimit::max(256 * 1024 * 1024))
        .with_state(sb);
    let listener = tokio::net::TcpListener::bind(&opts.listen).await?;
    tracing::info!(listen = %opts.listen, work = %opts.work.display(), contained = opts.contained, "cori sandbox listening");
    axum::serve(listener, app).with_graceful_shutdown(crate::runtime::shutdown_signal()).await?;
    Ok(())
}

fn ok(v: Value) -> Response { Json(json!({ "ok": true, "value": v })).into_response() }
fn failed(e: anyhow::Error) -> Response { Json(json!({ "ok": false, "error": e.to_string() })).into_response() }
fn refuse(status: StatusCode, msg: impl Into<String>) -> Response { (status, Json(json!({ "ok": false, "error": msg.into() }))).into_response() }

/// The caller's token, and its lease: the sandbox's own, or the first one while it has none. A call for another lease is
/// refused (409): the caller asks another sandbox.
async fn guard(State(sb): State<Arc<Sandbox>>, req: Request, next: Next) -> Response {
    let (bearer, lease, path) = {
        let header = |n: &str| req.headers().get(n).and_then(|v| v.to_str().ok()).map(str::trim).unwrap_or("").to_string();
        (header("authorization"), header("x-cori-lease"), req.uri().path().to_string())
    };
    if !same(bearer.strip_prefix("Bearer ").unwrap_or("").as_bytes(), sb.opts.token.as_bytes()) { return refuse(StatusCode::UNAUTHORIZED, "sandbox token required"); }
    if lease.is_empty() || lease.len() > 200 { return refuse(StatusCode::BAD_REQUEST, "x-cori-lease names the app the call is for"); }
    let _call = sb.gate.read().await;
    {
        let mut s = sb.state.lock().unwrap();
        match &s.0 {
            Some(l) if l.id != lease => return refuse(StatusCode::CONFLICT, format!("this sandbox serves `{}`", l.id)),
            Some(_) => {}
            None => { tracing::info!(lease = %lease, "sandbox leased"); s.0 = Some(Lease { id: lease.clone(), since: Instant::now() }); }
        }
        s.1 += 1;
    }
    // counted out however the call ends, a caller gone mid-call included: a call never counted out would hold the lease
    let _in_flight = InFlight(&sb);
    tracing::debug!(lease = %lease, path = %path, "sandbox call");
    next.run(req).await
}

/// A call under way, until it is dropped.
struct InFlight<'a>(&'a Sandbox);

impl Drop for InFlight<'_> {
    fn drop(&mut self) { let mut s = self.0.state.lock().unwrap(); s.1 -= 1; s.2 = Instant::now(); }
}

/// Starts over once the lease is idle, or old and quiet.
async fn retire_loop(sb: Arc<Sandbox>) {
    let due = |sb: &Sandbox| { let s = sb.state.lock().unwrap(); s.0.as_ref().map(|l| s.1 == 0 && (s.2.elapsed() >= sb.opts.idle || l.since.elapsed() >= sb.opts.max_lease)).unwrap_or(false) };
    loop {
        tokio::time::sleep(Duration::from_secs(2)).await;
        if !due(&sb) { continue; }
        let _all = sb.gate.write().await;
        if !due(&sb) { continue; }
        let lease = sb.state.lock().unwrap().0.take().map(|l| l.id).unwrap_or_default();
        start_over(&sb.opts).await;
        tracing::info!(lease = %lease, "sandbox started over");
    }
}

/// Stops what calls started, empties the work folder, and writes Cori's runtime files again.
async fn start_over(opts: &SandboxOptions) {
    crate::runner::stop_all();
    // a run's program may have left processes behind, detached from anything this process knows: in a container of its
    // own, every process of this user but this one (and the init) goes, before the folder they could write is emptied
    #[cfg(unix)]
    if opts.contained {
        for _ in 0..50 {
            unsafe { libc::kill(-1, libc::SIGKILL); }
            if others().is_empty() { break; }
            tokio::time::sleep(Duration::from_millis(40)).await;
        }
    }
    empty(&opts.work);
    // what processes leave behind them in the pod's shared memory, which outlives them
    #[cfg(target_os = "linux")]
    if opts.contained { clear_ipc(); }
    for d in ["home", "tmp"] { let _ = std::fs::create_dir_all(opts.work.join(d)); }
    deno_cache(opts);
    if let Err(e) = cori_compiler::runtime::restore() { tracing::error!(error = %e, "sandbox: the runtime files could not be written again"); }
}

/// Deno's cache in the work folder (`DENO_DIR`, set so by `cori sandbox serve`): Deno keeps there the modules it transpiles,
/// and a lease's are emptied with it. A cache Deno cannot write costs each runner start about 170 ms (it retries the
/// write). Its packages (`npm/`) are the ones of the cache Deno had (`deno_packages`), linked: the runner's own are
/// cached beforehand, and no step fetches one.
fn deno_cache(opts: &SandboxOptions) {
    let Some(src) = &opts.deno_packages else { return };
    let dir = opts.work.join("deno");
    let _ = std::fs::create_dir_all(&dir);
    let npm = src.join("npm");
    // a cache with no packages yet (a new laptop): they are fetched into it, not into the work folder
    let _ = std::fs::create_dir_all(&npm);
    #[cfg(unix)]
    if npm.is_dir() && npm != dir.join("npm") && let Err(e) = std::os::unix::fs::symlink(&npm, dir.join("npm")) {
        tracing::error!(error = %e, "sandbox: Deno's packages could not be linked into its cache");
    }
}

/// Removes everything in `dir`.
fn empty(dir: &Path) {
    if let Ok(rd) = std::fs::read_dir(dir) { for e in rd.flatten() { let p = e.path(); let _ = if p.is_dir() { std::fs::remove_dir_all(&p) } else { std::fs::remove_file(&p) }; } }
}

/// The files of /dev/shm and /dev/mqueue, and the System V shared memory segments, message queues and semaphore sets
/// (`/proc/sysvipc`): a container's processes share them, and they outlive the processes.
#[cfg(target_os = "linux")]
fn clear_ipc() {
    empty(Path::new("/dev/shm"));
    empty(Path::new("/dev/mqueue"));
    for kind in ["shm", "msg", "sem"] {
        let Ok(t) = std::fs::read_to_string(format!("/proc/sysvipc/{kind}")) else { tracing::debug!(kind, "no /proc/sysvipc here"); continue };
        // a header line, then one object per line: its key, then its id
        for id in t.lines().skip(1).filter_map(|l| l.split_whitespace().nth(1)?.parse::<libc::c_int>().ok()) {
            unsafe {
                match kind {
                    "shm" => { libc::shmctl(id, libc::IPC_RMID, std::ptr::null_mut()); }
                    "msg" => { libc::msgctl(id, libc::IPC_RMID, std::ptr::null_mut()); }
                    _ => { libc::semctl(id, 0, libc::IPC_RMID); }
                }
            }
        }
    }
}

/// The processes of this container but this one and its init (`/proc`; none known elsewhere).
fn others() -> Vec<u32> {
    let me = std::process::id();
    std::fs::read_dir("/proc").map(|rd| rd.flatten().filter_map(|e| e.file_name().to_str()?.parse::<u32>().ok()).filter(|p| *p != me && *p != 1).collect()).unwrap_or_default()
}

/// Whether this process is alone in a container under its init: Linux, and its parent is the namespace's first process.
fn in_own_container() -> bool {
    #[cfg(target_os = "linux")]
    { std::process::id() == 1 || unsafe { libc::getppid() } == 1 }
    #[cfg(not(target_os = "linux"))]
    { false }
}

/// Constant-time equality of two secrets.
fn same(a: &[u8], b: &[u8]) -> bool { a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0 }

/// `base` joined with `rel`, a relative path that stays below it (no `..`, no root, no prefix).
fn below(base: &Path, rel: &str) -> Result<PathBuf> {
    let p = Path::new(rel);
    if rel.is_empty() || rel.contains('\\') || !p.components().all(|c| matches!(c, Component::Normal(_))) { return Err(anyhow!("InvalidInputError: `{rel}` is not a path below the bundle")); }
    Ok(base.join(p))
}

fn bundle_dir(sb: &Sandbox, hash: &str) -> Result<PathBuf> {
    if hash.is_empty() || !hash.chars().all(|c| c.is_ascii_alphanumeric() || c == ':' || c == '-' || c == '_') { return Err(anyhow!("InvalidInputError: bad bundle hash")); }
    Ok(sb.opts.work.join("bundles").join(hash.replace(':', "_")))
}

/// The workflow folder of a call, in a bundle this sandbox has (404 `unknown bundle` otherwise: the caller sends it), its
/// `.cori/` generated for this binary when `generate`.
fn workflow_of(sb: &Sandbox, body: &Value, generate: bool) -> std::result::Result<PathBuf, Box<Response>> {
    let dir = bundle_dir(sb, body["bundle"].as_str().unwrap_or("")).map_err(|e| refuse(StatusCode::BAD_REQUEST, e.to_string()))?;
    if !dir.join(".complete").exists() { return Err(refuse(StatusCode::NOT_FOUND, "unknown bundle: send it first (/v1/bundle)").into()); }
    let root = below(&dir, body["workflow"].as_str().unwrap_or("")).map_err(|e| refuse(StatusCode::BAD_REQUEST, e.to_string()))?;
    if !root.join("workflow.yaml").is_file() { return Err(refuse(StatusCode::BAD_REQUEST, "InvalidInputError: no workflow there").into()); }
    if generate { crate::activities::ensure_generated(&root).map_err(failed)?; }
    Ok(root)
}

/// A bundle, written beside its place and moved there whole: calls running on it never read a file being written (two
/// callers may send the same bundle at once).
async fn bundle(State(sb): State<Arc<Sandbox>>, Json(b): Json<Value>) -> Response {
    let r: Result<()> = (|| {
        let dir = bundle_dir(&sb, b["hash"].as_str().unwrap_or(""))?;
        if dir.join(".complete").exists() { return Ok(()); }
        let files = b["files"].as_object().ok_or_else(|| anyhow!("InvalidInputError: `files` is the bundle's files (path → text)"))?;
        let tmp = dir.with_file_name(format!(".tmp-{}", uuid::Uuid::new_v4().simple()));
        let write = || -> Result<()> {
            for (rel, text) in files {
                let p = below(&tmp, rel)?;
                if let Some(parent) = p.parent() { std::fs::create_dir_all(parent)?; }
                std::fs::write(&p, text.as_str().unwrap_or(""))?;
            }
            Ok(std::fs::write(tmp.join(".complete"), b"ok")?)
        };
        let moved = write().and_then(|()| Ok(std::fs::rename(&tmp, &dir)?));
        if moved.is_err() { let _ = std::fs::remove_dir_all(&tmp); }
        // another caller's copy got there first: the same files
        if dir.join(".complete").exists() { Ok(()) } else { moved }
    })();
    match r { Ok(()) => ok(json!({})), Err(e) => failed(e) }
}

async fn runner(State(sb): State<Arc<Sandbox>>, Json(b): Json<Value>) -> Response {
    let root = match workflow_of(&sb, &b, true) { Ok(r) => r, Err(r) => return *r };
    let mut req = b["req"].clone();
    if let Some(f) = req["file"].as_str() {
        match below(&root, f) { Ok(p) => req["file"] = json!(p.display().to_string()), Err(e) => return failed(e) }
    }
    let run = b["run"].as_str().unwrap_or("").to_string();
    let r = async { crate::runner::runner_for(&root, &run).await?.lock().await.call(req).await }.await;
    match r { Ok(v) => ok(v), Err(e) => failed(e) }
}

/// A run's own folder, its programs' working folder and home.
fn run_dir(sb: &Sandbox, run: &str) -> Result<PathBuf> {
    let safe: String = run.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' { c } else { '_' }).collect();
    let d = sb.opts.work.join("runs").join(if safe.is_empty() { "run".into() } else { safe });
    std::fs::create_dir_all(&d)?;
    Ok(d)
}

/// The output of a program, cut at this many bytes per stream.
const OUTPUT_MAX: usize = 8 * 1024 * 1024;

async fn exec(State(sb): State<Arc<Sandbox>>, Json(b): Json<Value>) -> Response {
    if let Err(r) = workflow_of(&sb, &b, false) { return *r; }
    let r: Result<Value> = async {
        let argv: Vec<String> = serde_json::from_value(b["argv"].clone())?;
        let (bin, args) = argv.split_first().ok_or_else(|| anyhow!("InvalidInputError: empty command"))?;
        let dir = run_dir(&sb, b["run"].as_str().unwrap_or(""))?;
        let mut cmd = tokio::process::Command::new(bin);
        crate::events::scrub_env(&mut cmd);
        cmd.args(args).current_dir(&dir).env("HOME", &dir).env("TMPDIR", &dir).stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).kill_on_drop(true);
        // a group of its own, so that what it starts stops with it
        #[cfg(unix)]
        cmd.process_group(0);
        let child = cmd.spawn().map_err(|e| anyhow!("MissingCapabilityError: cannot run `{bin}`: {e}"))?;
        let pid = child.id();
        let limit = crate::runner::call_timeout();
        let out = tokio::time::timeout(limit, child.wait_with_output()).await;
        #[cfg(unix)]
        if let Some(pid) = pid { unsafe { libc::kill(-(pid as i32), libc::SIGKILL); } }
        let out = out.map_err(|_| anyhow!("Timeout: `{bin}` ran over {}s (CORI_STEP_TIMEOUT_SECS)", limit.as_secs()))??;
        let cut = |b: &[u8]| String::from_utf8_lossy(&b[..b.len().min(OUTPUT_MAX)]).to_string();
        Ok(json!({ "stdout": cut(&out.stdout), "stderr": cut(&out.stderr), "code": out.status.code().unwrap_or(-1) }))
    }.await;
    match r { Ok(v) => ok(v), Err(e) => failed(e) }
}

async fn mcp(State(sb): State<Arc<Sandbox>>, Json(b): Json<Value>) -> Response {
    let root = match workflow_of(&sb, &b, false) { Ok(r) => r, Err(r) => return *r };
    let r = async {
        let command: Vec<String> = serde_json::from_value(b["command"].clone())?;
        crate::mcp::rpc_stdio(&command, &root, b["method"].as_str().unwrap_or(""), b["params"].clone()).await
    }.await;
    match r { Ok(v) => ok(v), Err(e) => failed(e) }
}

/// The compiler's Deno half for the server (`sandbox::SandboxChecks`): the generated context (typed by the registry's
/// connectors, as the server's), then the type check and the output schemas.
async fn check(State(sb): State<Arc<Sandbox>>, Json(b): Json<Value>) -> Response {
    let root = match workflow_of(&sb, &b, false) { Ok(r) => r, Err(r) => return *r };
    let r = tokio::task::spawn_blocking(move || -> Result<Value> {
        let connectors: Vec<Value> = serde_json::from_value(b["connectors"].clone()).unwrap_or_default();
        let files: Vec<String> = serde_json::from_value(b["files"].clone())?;
        cori_compiler::compile(&root, &cori_compiler::Options { run_deno: false, sdk_path: None, connectors, deno: None }).map_err(|e| anyhow!("StepFailedError: {}", e.join("; ")))?;
        let (type_check, schemas) = cori_compiler::deno_checks(&root, &files).map_err(|e| anyhow!("StepFailedError: {e}"))?;
        Ok(json!({ "type_check": type_check, "schemas": schemas }))
    }).await;
    match r { Ok(Ok(v)) => ok(v), Ok(Err(e)) => failed(e), Err(e) => failed(anyhow!("RuntimeUnavailableError: {e}")) }
}

/// `check` for several workflows of the bundle at once (`cori_compiler::deno_checks_all`): one type check of all their
/// steps, one export of their schemas. `passed: false` when they do not pass together: the server then checks them one
/// at a time, which says which one fails and why.
async fn check_app(State(sb): State<Arc<Sandbox>>, Json(b): Json<Value>) -> Response {
    let mut roots = Vec::new();
    for w in b["workflows"].as_array().cloned().unwrap_or_default() {
        let one = json!({ "bundle": b["bundle"], "workflow": w["workflow"] });
        match workflow_of(&sb, &one, false) { Ok(r) => roots.push((r, w["files"].clone())), Err(r) => return *r }
    }
    let r = tokio::task::spawn_blocking(move || -> Result<Value> {
        let connectors: Vec<Value> = serde_json::from_value(b["connectors"].clone()).unwrap_or_default();
        let mut workflows = Vec::new();
        for (root, files) in roots {
            let files: Vec<String> = serde_json::from_value(files)?;
            cori_compiler::compile(&root, &cori_compiler::Options { run_deno: false, sdk_path: None, connectors: connectors.clone(), deno: None }).map_err(|e| anyhow!("StepFailedError: {}", e.join("; ")))?;
            workflows.push((root.canonicalize().unwrap_or(root), files));
        }
        let schemas = cori_compiler::deno_checks_all(&workflows).map_err(|e| anyhow!("StepFailedError: {e}"))?;
        Ok(json!({ "passed": schemas.is_some(), "schemas": schemas }))
    }).await;
    match r { Ok(Ok(v)) => ok(v), Ok(Err(e)) => failed(e), Err(e) => failed(anyhow!("RuntimeUnavailableError: {e}")) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_stays_below_its_bundle() {
        let base = Path::new("/w/bundles/h");
        assert_eq!(below(base, "workflows/a/steps/x.ts").unwrap(), base.join("workflows/a/steps/x.ts"));
        for bad in ["", "../x", "workflows/../../x", "/etc/passwd", "a\\b", "./x"] { assert!(below(base, bad).is_err(), "{bad}"); }
    }
}
