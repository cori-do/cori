// SPDX-License-Identifier: Apache-2.0
//! Where builders' code runs: the step modules (the Deno runner), the programs of `cli` steps and stdio MCP servers, and
//! at deploy the type check and the output-schema export.
//!
//! - Without `CORI_SANDBOX_URL` (a laptop, `cori up`, `cori run`, a cluster without sandboxes): in processes of this
//!   machine, as before (a Deno runner per run, with read access to its bundle only; programs with a scrubbed environment).
//! - With it: in sandboxes (`cori sandbox serve`, `sandbox_server`), processes of their own, in a cluster each in a pod
//!   under gVisor that holds no credential and reaches no network. This process (a worker, the server) sends a sandbox the
//!   bundle and each call, and keeps everything else: the orchestration, the connector calls, every token. A sandbox serves
//!   one app of one organization at a time (its lease, `<realm>/<app>`); idle, it kills what its runs started, empties its
//!   work folder, and is free for another lease.
//!
//! A hosted run whose server has sandboxes says so (`BundleRef::sandboxed`): a worker without them refuses its steps rather
//! than running them here.
use crate::activities::StepInput;
use crate::runner::Runner;
use anyhow::{Result, anyhow};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex, OnceLock};
use tokio::sync::Mutex;

/// The sandboxes this process sends builders' code to.
#[derive(Clone, Debug)]
pub struct SandboxConfig {
    /// `CORI_SANDBOX_URL`: `http://<host>:<port>`, every address of the host a sandbox (a headless service); several,
    /// separated by commas.
    pub urls: Vec<String>,
    /// `CORI_SANDBOX_TOKEN`: what the sandboxes check their callers with.
    pub token: String,
    /// How long a call waits for a sandbox to be free when every one serves another app (`CORI_SANDBOX_WAIT_SECS`, 20).
    pub wait: std::time::Duration,
}

impl SandboxConfig {
    pub fn from_env() -> Option<Self> {
        let var = |n: &str| std::env::var(n).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
        let urls: Vec<String> = var("CORI_SANDBOX_URL")?.split(',').map(|u| u.trim().trim_end_matches('/').to_string()).filter(|u| !u.is_empty()).collect();
        let wait = std::time::Duration::from_secs(var("CORI_SANDBOX_WAIT_SECS").and_then(|v| v.parse().ok()).unwrap_or(20));
        (!urls.is_empty()).then(|| Self { urls, token: var("CORI_SANDBOX_TOKEN").unwrap_or_default(), wait })
    }
}

/// The lease of a hosted app's code: its realm and app (`<realm>/<app>`, the realm empty for the core's single one).
pub fn lease_of(realm: Option<&str>, app: &str) -> String { format!("{}/{app}", realm.unwrap_or("")) }

/// A sandbox this process uses for one lease.
struct Conn { base: String, uploaded: StdMutex<HashSet<String>> }

/// Why a call to a sandbox did not get the sandbox's own answer.
enum Miss { Lost(String), NoBundle, Failed(anyhow::Error) }

/// How the error of a call to a route the sandbox does not have starts (a sandbox of an older Cori).
pub const UNKNOWN_ROUTE: &str = "UnknownRouteError";
/// The deploy checks of several workflows in one call (`SandboxChecks::check_all`), which older sandboxes lack.
const CHECK_APP: &str = "/v1/check-app";

/// The sandboxes of this process, by lease.
pub struct Pool { cfg: SandboxConfig, http: reqwest::Client, leases: StdMutex<HashMap<String, Arc<Conn>>> }

impl Pool {
    /// This process's sandboxes, from the environment (`CORI_SANDBOX_URL`), once; None without.
    pub fn global() -> Option<&'static Pool> {
        static POOL: OnceLock<Option<Pool>> = OnceLock::new();
        POOL.get_or_init(|| SandboxConfig::from_env().map(Pool::new)).as_ref()
    }

    pub fn new(cfg: SandboxConfig) -> Self {
        // a call answers within the sandbox's own deadline (a step's, a check's): the connection only needs to be up
        let http = crate::http::builder().connect_timeout(std::time::Duration::from_secs(5)).timeout(std::time::Duration::from_secs(900)).build().unwrap_or_default();
        Self { cfg, http, leases: StdMutex::new(HashMap::new()) }
    }

    /// The sandbox of `lease`: the one this process already uses for it, else the first sandbox that takes it. When every
    /// one serves another lease, the call waits for one to free up (they do once idle), up to `SandboxConfig::wait`.
    async fn conn(&self, lease: &str) -> Result<Arc<Conn>> {
        if let Some(c) = self.leases.lock().unwrap().get(lease) { return Ok(c.clone()); }
        let deadline = tokio::time::Instant::now() + self.cfg.wait;
        let mut pause = std::time::Duration::from_millis(250);
        loop {
            let why = match self.take(lease).await {
                Ok(c) => return Ok(c),
                Err(why) => why,
            };
            if tokio::time::Instant::now() + pause > deadline {
                // the activity is retried later
                return Err(anyhow!("RuntimeUnavailableError: no sandbox is free for `{lease}` ({})", if why.is_empty() { "none found".into() } else { why.join("; ") }));
            }
            tokio::time::sleep(pause).await;
            pause = (pause * 2).min(std::time::Duration::from_secs(2));
        }
    }

    /// One pass over the sandboxes (every address of every URL): the first that takes `lease`, or why none did.
    async fn take(&self, lease: &str) -> std::result::Result<Arc<Conn>, Vec<String>> {
        let mut bases = Vec::new();
        let mut why = Vec::new();
        for u in &self.cfg.urls {
            let Ok(url) = reqwest::Url::parse(u) else { why.push(format!("CORI_SANDBOX_URL `{u}` is not a URL")); continue };
            let host = url.host_str().unwrap_or_default().to_string();
            match tokio::net::lookup_host((host.as_str(), url.port_or_known_default().unwrap_or(80))).await {
                Ok(addrs) => bases.extend(addrs.map(|a| format!("{}://{a}", url.scheme()))),
                Err(e) => why.push(format!("{host}: {e}")),
            }
        }
        // spread the leases: each process starts from another sandbox
        let start = (std::process::id() as usize + lease.len()) % bases.len().max(1);
        bases.rotate_left(start);
        for base in bases {
            match self.send(&base, lease, "/v1/lease", &json!({})).await {
                Ok(_) => {
                    let c = Arc::new(Conn { base, uploaded: StdMutex::new(HashSet::new()) });
                    self.leases.lock().unwrap().insert(lease.to_string(), c.clone());
                    return Ok(c);
                }
                Err(Miss::Lost(w)) => why.push(w),
                Err(Miss::NoBundle) => {}
                Err(Miss::Failed(e)) => why.push(e.to_string()),
            }
        }
        Err(why)
    }

    fn forget(&self, lease: &str, base: &str) {
        let mut l = self.leases.lock().unwrap();
        if l.get(lease).map(|c| c.base == base).unwrap_or(false) { l.remove(lease); }
    }

    /// One request to the sandbox at `base` for `lease`: its answer's `value`, or why there is none.
    async fn send(&self, base: &str, lease: &str, path: &str, body: &Value) -> std::result::Result<Value, Miss> {
        let res = self.http.post(format!("{base}{path}")).bearer_auth(&self.cfg.token).header("x-cori-lease", lease).json(body).send().await
            .map_err(|e| Miss::Lost(format!("{base}: {e}")))?;
        let status = res.status();
        let v: Value = res.json().await.unwrap_or(Value::Null);
        match status.as_u16() {
            200 if v["ok"].as_bool() == Some(true) => Ok(v["value"].clone()),
            // the step's own failure: its message keeps its type (`StepFailedError: …`), as from a local runner
            200 => Err(Miss::Failed(anyhow!("{}", v["error"].as_str().unwrap_or("StepFailedError: the sandbox failed")))),
            409 => Err(Miss::Lost(format!("{base}: {}", v["error"].as_str().unwrap_or("serves another lease")))),
            404 if v["error"].as_str().map(|e| e.contains("unknown bundle")).unwrap_or(false) => Err(Miss::NoBundle),
            // a sandbox of an older Cori, without the deploy checks of a whole app: the caller checks each workflow instead
            404 | 405 if v.is_null() && path == CHECK_APP => Err(Miss::Failed(anyhow!("{UNKNOWN_ROUTE}: sandbox {base} has no {path}"))),
            _ => Err(Miss::Failed(anyhow!("RuntimeUnavailableError: sandbox {base}: {status} {}", v["error"].as_str().unwrap_or("")))),
        }
    }

    /// A call for `lease` about the bundle `hash` (whose files are in `local`, this process's copy): the sandbox gets the
    /// bundle first when it does not have it, and a sandbox lost (gone, restarted, taken by another lease) is replaced.
    pub async fn call(&self, lease: &str, hash: &str, local: &Path, path: &str, body: Value) -> Result<Value> {
        let mut last = anyhow!("RuntimeUnavailableError: no sandbox answered");
        for _ in 0..3 {
            let conn = self.conn(lease).await?;
            if !conn.uploaded.lock().unwrap().contains(hash) {
                let files = bundle_files(local)?;
                match self.send(&conn.base, lease, "/v1/bundle", &json!({ "hash": hash, "files": files })).await {
                    Ok(_) => { conn.uploaded.lock().unwrap().insert(hash.to_string()); }
                    Err(Miss::Lost(w)) => { self.forget(lease, &conn.base); last = anyhow!("RuntimeUnavailableError: {w}"); continue; }
                    Err(Miss::NoBundle) => continue,
                    Err(Miss::Failed(e)) => return Err(e),
                }
            }
            match self.send(&conn.base, lease, path, &body).await {
                Ok(v) => return Ok(v),
                Err(Miss::Lost(w)) => { self.forget(lease, &conn.base); last = anyhow!("RuntimeUnavailableError: {w}"); }
                // the sandbox started over (idle, retired): it gets the bundle again
                Err(Miss::NoBundle) => { conn.uploaded.lock().unwrap().remove(hash); }
                Err(Miss::Failed(e)) => return Err(e),
            }
        }
        Err(last)
    }
}

/// The files of a bundle copy as a sandbox receives them (`path → text`), without what is generated here (`.cori/`).
pub fn bundle_files(root: &Path) -> Result<std::collections::BTreeMap<String, String>> {
    let mut out = std::collections::BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d)?.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            if name == ".cori" || name == "node_modules" || name == ".complete" || (d == root && name == "dist") { continue; }
            if e.file_type()?.is_dir() { stack.push(p); continue; }
            let rel = p.strip_prefix(root)?.to_string_lossy().replace('\\', "/");
            if let Ok(t) = std::fs::read_to_string(&p) { out.insert(rel, t); }
        }
    }
    Ok(out)
}

/// What a hosted step needs to reach its sandbox: the pool, the lease, the bundle and the workflow folder in it.
struct Hosted<'a> { pool: &'static Pool, lease: String, hash: &'a str, bundle_root: PathBuf, workflow: String }

/// The sandbox side of a hosted step, or None when the step runs here: no bundle (a local run), or no sandboxes here for a
/// run whose server has none either. A run whose server has sandboxes refuses to run here without them.
fn hosted<'a>(i: &'a StepInput, root: &Path) -> Result<Option<Hosted<'a>>> {
    let Some(b) = &i.bundle else {
        // a run without a bundle names a folder of this machine: a worker with sandboxes serves hosted runs only
        if Pool::global().is_some() { return Err(anyhow!("InvalidInputError: this worker runs builders' code in sandboxes: a run comes with its bundle")); }
        return Ok(None);
    };
    let Some(pool) = Pool::global() else {
        // an organization's own pool (`cori work --pool`) runs the steps pinned to it on that organization's machines
        let own_pool = std::env::var("CORI_WORKER_POOL").map(|p| !p.is_empty()).unwrap_or(false);
        if b.sandboxed && !own_pool { return Err(anyhow!("RuntimeUnavailableError: this run's code runs in sandboxes, and this worker has none (CORI_SANDBOX_URL)")); }
        return Ok(None);
    };
    let bundle_root = crate::activities::bundle_cache_dir().join(&b.hash);
    let workflow = root.strip_prefix(&bundle_root).map(|p| p.to_string_lossy().replace('\\', "/")).unwrap_or_else(|_| i.root.clone());
    Ok(Some(Hosted { pool, lease: lease_of(b.realm.as_deref(), b.app.as_deref().unwrap_or("")), hash: &b.hash, bundle_root, workflow }))
}

/// The step code of one run: a Deno runner of this machine, or the run's runner in its sandbox.
pub enum StepRunner {
    Local(Arc<Mutex<Runner>>),
    Remote { pool: &'static Pool, lease: String, hash: String, bundle_root: PathBuf, workflow: String, root: PathBuf, run: String },
}

impl StepRunner {
    /// One runner request (`op`, and `file` naming a step file under the workflow folder).
    pub async fn call(&self, req: Value) -> Result<Value> {
        match self {
            StepRunner::Local(r) => r.lock().await.call(req).await,
            StepRunner::Remote { pool, lease, hash, bundle_root, workflow, root, run } => {
                let mut req = req;
                // a step file is named from the workflow folder: the sandbox has its own copy of it
                if let Some(f) = req["file"].as_str() { req["file"] = json!(Path::new(f).strip_prefix(root).map(|p| p.to_string_lossy().replace('\\', "/")).unwrap_or_else(|_| f.to_string())); }
                pool.call(lease, hash, bundle_root, "/v1/runner", json!({ "bundle": hash, "workflow": workflow, "run": run, "req": req })).await
            }
        }
    }
}

/// The runner of the run `i` belongs to, for the workflow folder `root` (`resolve_root`).
pub async fn step_runner(i: &StepInput, root: &Path) -> Result<StepRunner> {
    let run = i.run["id"].as_str().unwrap_or("").to_string();
    Ok(match hosted(i, root)? {
        Some(h) => StepRunner::Remote { pool: h.pool, lease: h.lease, hash: h.hash.to_string(), bundle_root: h.bundle_root, workflow: h.workflow, root: root.to_path_buf(), run },
        None => StepRunner::Local(crate::runner::runner_for(root, &run).await?),
    })
}

/// A `cli` step's program (`argv`, checked against its connector by the caller): stdout, stderr, exit code. In a sandbox
/// it runs in a folder of its run, with no environment of this process.
pub async fn exec(i: &StepInput, root: &Path, argv: &[String]) -> Result<(String, String, i32)> {
    match hosted(i, root)? {
        Some(h) => {
            let v = h.pool.call(&h.lease, h.hash, &h.bundle_root, "/v1/exec", json!({ "bundle": h.hash, "workflow": h.workflow, "run": i.run["id"], "argv": argv })).await?;
            Ok((v["stdout"].as_str().unwrap_or("").to_string(), v["stderr"].as_str().unwrap_or("").to_string(), v["code"].as_i64().unwrap_or(-1) as i32))
        }
        None => {
            let o = crate::events::scrub_env(tokio::process::Command::new(&argv[0]).args(&argv[1..])).output().await.map_err(|e| anyhow!("MissingCapabilityError: cannot run `{}`: {e}", argv[0]))?;
            Ok((String::from_utf8_lossy(&o.stdout).to_string(), String::from_utf8_lossy(&o.stderr).to_string(), o.status.code().unwrap_or(-1)))
        }
    }
}

/// One JSON-RPC request to a stdio MCP server (`command`, its connector's): started for the call, here or in the sandbox.
pub async fn mcp_stdio(i: &StepInput, root: &Path, command: &[String], method: &str, params: Value) -> Result<Value> {
    match hosted(i, root)? {
        Some(h) => h.pool.call(&h.lease, h.hash, &h.bundle_root, "/v1/mcp", json!({ "bundle": h.hash, "workflow": h.workflow, "command": command, "method": method, "params": params })).await,
        None => crate::mcp::rpc_stdio(command, root, method, params).await,
    }
}

/// The compiler's Deno half (`cori_compiler::DenoRunner`) run in a sandbox: the server's deploy-time checks of an app's
/// bundle (`bundle`, its local copy) under the app's lease. The server computes everything else itself.
pub struct SandboxChecks { pub pool: &'static Pool, pub lease: String, pub hash: String, pub bundle: PathBuf, pub connectors: Vec<Value>, pub handle: tokio::runtime::Handle }

impl cori_compiler::DenoRunner for SandboxChecks {
    fn check(&self, workflow_dir: &Path, step_files: &[String]) -> std::result::Result<(cori_compiler::TypeCheck, std::collections::BTreeMap<String, Value>), String> {
        let bundle = self.bundle.canonicalize().unwrap_or_else(|_| self.bundle.clone());
        let workflow = workflow_dir.strip_prefix(&bundle).map(|p| p.to_string_lossy().replace('\\', "/")).map_err(|_| format!("{} is not in the bundle", workflow_dir.display()))?;
        let body = json!({ "bundle": self.hash, "workflow": workflow, "files": step_files, "connectors": self.connectors });
        let v = self.handle.block_on(self.pool.call(&self.lease, &self.hash, &self.bundle, "/v1/check", body)).map_err(|e| format!("in a sandbox: {e}"))?;
        let tc: cori_compiler::TypeCheck = serde_json::from_value(v["type_check"].clone()).map_err(|e| e.to_string())?;
        let schemas = serde_json::from_value(v["schemas"].clone()).map_err(|e| e.to_string())?;
        Ok((tc, schemas))
    }

    /// The workflows' passes in one sandbox call (`/v1/check-app`); None when they do not pass together, or when the
    /// sandbox is of an older Cori: the compiler then checks them one at a time.
    fn check_all(&self, workflows: &[(PathBuf, Vec<String>)]) -> std::result::Result<Option<Vec<std::collections::BTreeMap<String, Value>>>, String> {
        let bundle = self.bundle.canonicalize().unwrap_or_else(|_| self.bundle.clone());
        let mut each = Vec::new();
        for (dir, files) in workflows {
            let workflow = dir.strip_prefix(&bundle).map(|p| p.to_string_lossy().replace('\\', "/")).map_err(|_| format!("{} is not in the bundle", dir.display()))?;
            each.push(json!({ "workflow": workflow, "files": files }));
        }
        let body = json!({ "bundle": self.hash, "workflows": each, "connectors": self.connectors });
        match self.handle.block_on(self.pool.call(&self.lease, &self.hash, &self.bundle, CHECK_APP, body)) {
            Ok(v) if v["passed"].as_bool() == Some(true) => serde_json::from_value(v["schemas"].clone()).map(Some).map_err(|e| e.to_string()),
            Ok(_) => Ok(None),
            Err(e) if e.to_string().starts_with(UNKNOWN_ROUTE) => Ok(None),
            Err(e) => Err(format!("in a sandbox: {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lease_is_an_app_of_a_realm() {
        assert_eq!(lease_of(Some("org-a"), "desk"), "org-a/desk");
        assert_eq!(lease_of(None, "desk"), "/desk");
    }

    #[test]
    fn a_worker_without_sandboxes_refuses_the_code_of_a_run_that_has_them() {
        // this process has none (CORI_SANDBOX_URL)
        if Pool::global().is_some() { return; }
        let input = |sandboxed: bool| StepInput {
            run: json!({ "id": "r" }), user: Value::Null, params: Value::Null, root: "workflows/w".into(),
            bundle: Some(crate::workflow::BundleRef { hash: "h".into(), server: "http://s".into(), realm: Some("org-a".into()), app: Some("desk".into()), sandboxed }),
            store_server: None, record_dir: None, redact: vec![], step: crate::activities::StepSpec { name: "s".into(), kind: "code".into(), file: "s.ts".into(), literals: Default::default() },
            steps: Default::default(), item: None, connector: None, llm: Default::default(), selector: None,
        };
        let refused = hosted(&input(true), Path::new("/w")).err().expect("refused");
        assert!(refused.to_string().starts_with("RuntimeUnavailableError"), "{refused}");
        assert!(hosted(&input(false), Path::new("/w")).unwrap().is_none(), "a run whose server has no sandboxes runs here");
    }

    #[test]
    fn a_bundle_travels_without_what_is_generated() {
        let d = std::env::temp_dir().join(format!("cori-bundle-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(d.join("workflows/w/.cori")).unwrap();
        std::fs::write(d.join("cori.toml"), "[app]\nid = \"a\"\n").unwrap();
        std::fs::write(d.join("workflows/w/workflow.yaml"), "id: w\n").unwrap();
        std::fs::write(d.join("workflows/w/.cori/deno.json"), "{}").unwrap();
        std::fs::write(d.join(".complete"), "ok").unwrap();
        std::fs::create_dir_all(d.join("dist")).unwrap();
        std::fs::write(d.join("dist/index.html"), "<p>front</p>").unwrap();
        let f = bundle_files(&d).unwrap();
        assert_eq!(f.keys().cloned().collect::<Vec<_>>(), ["cori.toml", "workflows/w/workflow.yaml"]);
        std::fs::remove_dir_all(d).ok();
    }
}
