// SPDX-License-Identifier: Apache-2.0
//! Local execution: compile, resolve the app config, start the workflow on a Temporal dev server, await the result.
use crate::activities::CoriActivities;
use crate::config::AppConfig;
use crate::runtime::{TemporalRuntime, port_open};
use crate::workflow::{CoriWorkflow, WorkflowInput, WorkflowOutput};
use anyhow::{Context, Result, anyhow};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;
use temporalio_client::{WorkflowGetResultOptions, WorkflowStartOptions};
use temporalio_sdk::{Worker, WorkerOptions};

pub struct RunOptions { pub params: Value, pub user: Value, pub decisions: BTreeMap<String, Value>, pub temporal: String, pub namespace: String, pub task_queue: String, /// Connectors whose dev mock is ignored (live transport, e.g. a native activity worker on its queue)
    pub no_mocks: Vec<String>,
    /// In-process store endpoint for local runs against a real Cori Store (see `cori_server::stores::local_endpoint`).
    pub store_server: Option<String>,
    /// Org registry (name → connector config) merged under the app's cori.toml; also typed into the context.
    pub registry: BTreeMap<String, crate::config::ConnectorConfig>,
    /// Registry rows with tool declarations, for the typed context (`ConnectorTools`).
    pub registry_rows: Vec<Value>,
    /// Record every live connector call as a dev fixture under this directory (`cori run --record`).
    pub record_dir: Option<String>,
    /// `Some(file)`: answer `llm` steps from this fixture whatever `[llm]` says (`cori run --mock-llm`); `Some(None)` = the app's mock file.
    pub mock_llm: Option<Option<String>> }

impl Default for RunOptions {
    fn default() -> Self { Self { params: Value::Object(Default::default()), user: serde_json::json!({ "sub": "dev", "roles": ["it_admin"], "email": "dev@local" }), decisions: BTreeMap::new(), temporal: "http://127.0.0.1:7233".into(), namespace: crate::runtime::default_namespace().into(), task_queue: "cori.local".into(), no_mocks: vec![], store_server: None, registry: BTreeMap::new(), registry_rows: vec![], record_dir: None, mock_llm: None } }
}

/// The largest file a local run reads, as the server's default (`CORI_FILES_MAX_BYTES`).
pub const LOCAL_FILE_MAX: u64 = 25 * 1024 * 1024;

/// The file parameters of a local run (`cori run k=<path>`): each file read, checked against its parameter (`accept`,
/// `max_size`), kept as `<app>/.cori/files/<id>` with its reference beside it (`<id>.json`), and replaced by that
/// reference, which is what a step sees. A `files` parameter takes a JSON list or paths separated by commas. A value that
/// is already a kept file's id (or reference) stays as it is.
pub fn stage_local_files(params: &mut Value, parameters: &[cori_compiler::manifest::Parameter], app_dir: &Path) -> Result<()> {
    use cori_compiler::files;
    let dir = app_dir.join(".cori").join("files");
    let kept = |id: &str| -> Option<Value> {
        if !files::valid_id(id) { return None; }
        std::fs::read_to_string(dir.join(format!("{id}.json"))).ok().and_then(|t| serde_json::from_str(&t).ok())
    };
    let one = |p: &cori_compiler::manifest::Parameter, v: &Value| -> Result<Value> {
        if let Some(r) = files::id_of(v).and_then(|id| kept(id)) { return Ok(r); }
        let path = v.as_str().ok_or_else(|| anyhow!("parameter `{}`: give a file's path (`{}=./invoice.pdf`), got {v}", p.name, p.name))?;
        let bytes = std::fs::read(path).map_err(|e| anyhow!("parameter `{}`: cannot read `{path}`: {e}", p.name))?;
        let name = files::clean_name(path);
        let limit = p.max_size_bytes().unwrap_or(LOCAL_FILE_MAX).min(LOCAL_FILE_MAX);
        if bytes.len() as u64 > limit { return Err(anyhow!("parameter `{}`: `{path}` is {}, more than the {} it takes", p.name, files::human_size(bytes.len() as u64), files::human_size(limit))); }
        let content_type = files::sniff(&bytes[..bytes.len().min(8192)], &name);
        if !files::accepts(p.accept.as_deref(), &name, &content_type) { return Err(anyhow!("parameter `{}`: `{path}` is {content_type}, which it does not take ({})", p.name, p.accept.as_deref().unwrap_or_default().join(", "))); }
        let sha256 = files::sha256_hex(&bytes);
        // the same bytes under the same name are the same file: a run again is given the same reference
        let id = format!("file_local{}", &files::sha256_hex(format!("{sha256}/{name}").as_bytes())[..24]);
        let r = files::CoriFile { id: id.clone(), name, size: bytes.len() as u64, content_type, sha256 };
        std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        std::fs::write(dir.join(&id), &bytes)?;
        std::fs::write(dir.join(format!("{id}.json")), serde_json::to_string_pretty(&r)?)?;
        Ok(r.to_json())
    };
    let Some(obj) = params.as_object_mut() else { return Ok(()) };
    for p in parameters.iter().filter(|p| p.is_file()) {
        let Some(v) = obj.get(&p.name).filter(|v| !v.is_null()).cloned() else { continue };
        let staged = if p.ty == "files" {
            let list: Vec<Value> = match v {
                Value::Array(a) => a,
                Value::String(s) => s.split(',').map(|x| x.trim()).filter(|x| !x.is_empty()).map(|x| Value::String(x.to_string())).collect(),
                other => vec![other],
            };
            Value::Array(list.iter().map(|x| one(p, x)).collect::<Result<Vec<_>>>()?)
        } else { one(p, &v)? };
        obj.insert(p.name.clone(), staged);
    }
    Ok(())
}

/// Ensures a Temporal dev server answers on 7233, spawning `temporal server start-dev --headless` if needed.
/// Returns the child when this call started it, so the caller can stop it on shutdown (`cori up` does).
pub async fn ensure_local_temporal() -> Result<Option<std::process::Child>> {
    if port_open("127.0.0.1", 7233) { return Ok(None); }
    let mut cmd = std::process::Command::new("temporal");
    cmd.args(["server", "start-dev", "--headless", "--port", "7233", "--log-level", "error"]).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    let child = cmd.spawn().context("starting `temporal server start-dev` (is the Temporal CLI installed?)")?;
    for _ in 0..100 { if port_open("127.0.0.1", 7233) { return Ok(Some(child)); } tokio::time::sleep(std::time::Duration::from_millis(200)).await; }
    Err(anyhow!("Temporal dev server did not come up on 7233"))
}

pub async fn run_local(workflow_dir: &Path, opts: RunOptions) -> Result<(cori_compiler::Compiled, WorkflowOutput)> {
    let workflow_dir = workflow_dir.canonicalize()?;
    let compiled = cori_compiler::compile(&workflow_dir, &cori_compiler::Options { run_deno: true, sdk_path: None, connectors: opts.registry_rows.clone(), deno: None }).map_err(|errs| anyhow!("check failed:\n  - {}", errs.join("\n  - ")))?;
    // layout v3: the workflow folder is `<app>/workflows/<id>`; the app (cori.toml, fixtures/, stores/) is two levels up
    if !workflow_dir.join("workflow.yaml").exists() { return Err(anyhow!("{} has no workflow.yaml (pass the workflow folder: <app>/workflows/<id>)", workflow_dir.display())); }
    let app_dir = workflow_dir.parent().and_then(|p| p.parent()).ok_or_else(|| anyhow!("workflow folder {} is not under <app>/workflows/", workflow_dir.display()))?;
    if !app_dir.join("cori.toml").exists() { return Err(anyhow!("{} has no cori.toml: a workflow lives in <app>/workflows/<id>/ with cori.toml at the app root", app_dir.display())); }
    let mut app = AppConfig::load(app_dir)?;
    app.merge_registry(&opts.registry);
    if let Some(file) = &opts.mock_llm { app.mock_llm(file.as_deref()); }
    let mut connectors = BTreeMap::new();
    // Every app connector travels with the run (child workflows pick theirs from the same map); the manifest's must resolve.
    for c in &compiled.manifest.connectors { app.resolve_connector(c)?; }
    for c in app.connectors.keys() { let Ok(mut rc) = app.resolve_connector(c) else { continue }; if opts.no_mocks.contains(c) || (rc.kind == "store" && opts.store_server.is_some()) { rc.mock = None; } connectors.insert(c.clone(), rc); }
    let llm = app.resolve_llm()?;
    // defaults, required and types for parameters
    let mut params = opts.params.clone();
    // `cori run k=v` parses each value as JSON: `title=123` meant the text "123" for a string parameter
    for p in &compiled.manifest.parameters { if matches!(p.ty.as_str(), "string" | "path" | "enum") { if let Some(v) = params.get_mut(&p.name).filter(|v| v.is_number() || v.is_boolean()) { *v = Value::String(v.to_string()); } } }
    // `invoice=./invoice.pdf`: the run gets the file's reference, as from an upload; its bytes stay in <app>/.cori/files/
    stage_local_files(&mut params, &compiled.manifest.parameters, app_dir)?;
    cori_compiler::manifest::validate_params(&mut params, &compiled.manifest.parameters).map_err(|e| if e.starts_with("missing parameter") { anyhow!("{e} (pass <name>=<value>)") } else { anyhow!("{e}") })?;
    ensure_local_temporal().await?;
    let rt = TemporalRuntime::connect(&opts.temporal, &opts.namespace, &opts.task_queue).await?;
    let input = WorkflowInput { compiled: compiled.clone(), params, user: opts.user, root: workflow_dir.display().to_string(), bundle: None, store_server: opts.store_server.clone(), record_dir: opts.record_dir.clone(), schedule: None, connectors, llm, started_at: chrono::Utc::now().to_rfc3339(), pool_queue_prefix: None, sandbox_queue: None, decisions: opts.decisions };
    let worker_options = WorkerOptions::new(rt.task_queue.clone()).register_workflow::<CoriWorkflow>()?.register_activities(CoriActivities).build();
    let mut worker = Worker::new(&rt.core, (*rt.client).clone(), worker_options).map_err(|e| anyhow!("constructing Temporal worker: {e}"))?;
    let shutdown = worker.shutdown_handle();
    let workflow_id = format!("{}-{}", compiled.id, uuid::Uuid::new_v4().simple());
    let starter = async {
        let handle = rt.client.start_workflow(CoriWorkflow::run, input, WorkflowStartOptions::new(rt.task_queue.clone(), workflow_id.clone()).build()).await.map_err(|e| anyhow::Error::new(e).context("starting workflow"))?;
        let r = handle.get_result(WorkflowGetResultOptions::default()).await.map_err(|e| anyhow::Error::new(e).context("awaiting workflow"));
        shutdown();
        r
    };
    let worker_fut = async { if let Err(e) = worker.run().await { tracing::warn!(error = %e, "worker exited with error"); } };
    let (result, _) = tokio::join!(starter, worker_fut);
    Ok((compiled, result?))
}

/// `cori work`: poll a task queue until Ctrl-C or SIGTERM. Bundle mode needs no local workflow files.
/// Polls `task_queue.0 … task_queue.(shards-1)` from one process (sharded shared queue, see the server's `shards`).
pub async fn serve_worker_shards(temporal: &str, namespace: &str, task_queue: &str, shards: u32) -> Result<()> {
    if shards <= 1 { return serve_worker(temporal, namespace, task_queue).await; }
    let local = tokio::task::LocalSet::new();
    local.run_until(async move {
        let mut hs = Vec::new();
        for i in 0..shards { let (t, n, q) = (temporal.to_string(), namespace.to_string(), format!("{task_queue}.{i}")); hs.push(tokio::task::spawn_local(async move { serve_worker(&t, &n, &q).await })); }
        for h in hs { h.await??; }
        Ok(())
    }).await
}

/// Several queues from one process (e.g. `cori up`: the shared queue and the sandbox pool, since a dev box has no gVisor pool).
pub async fn serve_worker_queues(temporal: &str, namespace: &str, queues: Vec<String>) -> Result<()> {
    if queues.len() == 1 { return serve_worker(temporal, namespace, &queues[0]).await; }
    let local = tokio::task::LocalSet::new();
    local.run_until(async move {
        let mut hs = Vec::new();
        for q in queues { let (t, n) = (temporal.to_string(), namespace.to_string()); hs.push(tokio::task::spawn_local(async move { serve_worker(&t, &n, &q).await })); }
        for h in hs { h.await??; }
        Ok(())
    }).await
}

pub async fn serve_worker(temporal: &str, namespace: &str, task_queue: &str) -> Result<()> {
    crate::runner::keep_spares();
    let rt = TemporalRuntime::connect(temporal, namespace, task_queue).await?;
    // a worker started before its namespace exists would sit idle without polling or logging: check it (for up to a
    // minute, the time a fresh cluster takes to register it) and exit with an error otherwise, so the pod restarts
    {
        use temporalio_client::grpc::WorkflowService;
        use temporalio_common::protos::temporal::api::workflowservice::v1::DescribeNamespaceRequest;
        let mut conn = rt.client.connection().clone();
        let mut last = String::new();
        let found = 'wait: {
            for _ in 0..30 {
                match conn.describe_namespace(temporalio_client::tonic::Request::new(DescribeNamespaceRequest { namespace: namespace.to_string(), ..Default::default() })).await {
                    Ok(_) => break 'wait true,
                    Err(e) => { last = e.message().to_string(); tokio::time::sleep(std::time::Duration::from_secs(2)).await; }
                }
            }
            false
        };
        if !found { return Err(anyhow!("Temporal namespace `{namespace}` is not available at {temporal}: {last} (create it, or set CORI_TEMPORAL_NAMESPACE)")); }
    }
    let worker_options = WorkerOptions::new(rt.task_queue.clone()).register_workflow::<CoriWorkflow>()?.register_activities(CoriActivities).build();
    let mut worker = Worker::new(&rt.core, (*rt.client).clone(), worker_options).map_err(|e| anyhow!("constructing Temporal worker: {e}"))?;
    let shutdown = worker.shutdown_handle();
    tracing::info!(task_queue = %task_queue, "worker polling");
    // a worker that stops on its own (an error) ends the process, so the pod restarts; a signal drains it first
    let run = worker.run();
    tokio::pin!(run);
    tokio::select! {
        r = &mut run => return r.map_err(|e| anyhow!("worker: {e}")),
        _ = crate::runtime::shutdown_signal() => shutdown(),
    }
    run.await.map_err(|e| anyhow!("worker: {e}"))
}
