// SPDX-License-Identifier: Apache-2.0
//! The two generic activities: `cori_step` executes a step of any kind, `cori_select` evaluates a builtin selector.
use crate::config::{ResolvedConnector, ResolvedLlm};
use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::path::Path;
use temporalio_macros::activities;
use temporalio_sdk::activities::{ActivityContext, ActivityError};
use temporalio_sdk::error::ApplicationFailure;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepSpec { pub name: String, pub kind: String, pub file: String, pub literals: Map<String, Value> }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoadWorkflowInput { pub root: String, #[serde(default)] pub bundle: Option<crate::workflow::BundleRef>, pub id: String }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepInput {
    pub run: Value,
    pub user: Value,
    pub params: Value,
    pub root: String,
    #[serde(default)]
    pub bundle: Option<crate::workflow::BundleRef>,
    #[serde(default)]
    pub store_server: Option<String>,
    #[serde(default)]
    pub record_dir: Option<String>,
    /// Manifest `redact:` paths; their current values are blanked in captured LLM inputs and outputs.
    #[serde(default)]
    pub redact: Vec<String>,
    pub step: StepSpec,
    /// Outputs of the steps this step references (only those).
    pub steps: Map<String, Value>,
    #[serde(default)]
    pub item: Option<Value>,
    #[serde(default)]
    pub connector: Option<ResolvedConnector>,
    pub llm: ResolvedLlm,
    #[serde(default)]
    pub selector: Option<String>,
}

/// A step's result. `attempts`: the attempt that gave it (Temporal's, from 1), so how many the step took; 0 in an output
/// recorded before steps counted them.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StepOutput { pub output: Value, pub cost_eur: f64, pub notes: Vec<String>, #[serde(default)] pub tokens_in: u64, #[serde(default)] pub tokens_out: u64, #[serde(default)] pub attempts: u32 }

/// What a failed attempt of `cori_step` tells the workflow besides its error, in the failure's details: which attempt it
/// was (Temporal's, from 1). The last one's is how many the step took.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailedAttempt { pub attempt: u32 }

pub struct CoriActivities;

#[activities]
impl CoriActivities {
    #[activity]
    pub async fn cori_step(ctx: ActivityContext, input: StepInput) -> Result<StepOutput, ActivityError> {
        let server = input.bundle.as_ref().map(|b| b.server.clone());
        let realm = crate::workflow::BundleRef::realm_of(&input.bundle).map(String::from);
        let (run_id, name, kind) = (input.run["id"].as_str().unwrap_or("").to_string(), input.step.name.clone(), input.step.kind.clone());
        let attempt = ctx.info().attempt.max(1);
        // what the step's event shows of its data, kept as the run keeps it (`StepData`)
        let data = server.as_ref().map(|_| StepData::of(&input));
        let mut bound = None;
        let t0 = std::time::Instant::now();
        let r = run_step(input, &mut bound).await;
        let ms = t0.elapsed().as_millis() as u64;
        let mut ev = match &r {
            Ok(o) => json!({ "name": name, "kind": kind, "status": "ok", "ms": ms, "attempt": attempt, "notes": o.notes, "cost_eur": o.cost_eur }),
            Err(e) => json!({ "name": name, "kind": kind, "status": "failed", "ms": ms, "attempt": attempt, "error": e.to_string() }),
        };
        if let Some(d) = &data { d.add(&mut ev, bound.as_ref(), r.as_ref().ok().map(|o| &o.output)); }
        crate::events::emit(server.as_deref(), realm.as_deref(), &run_id, "step", ev);
        r.map(|o| StepOutput { attempts: attempt, ..o }).map_err(|e| activity_error(e, Some(attempt)))
    }
    #[activity]
    pub async fn cori_select(_ctx: ActivityContext, input: StepInput) -> Result<StepOutput, ActivityError> {
        run_select(input).await.map_err(to_activity_error)
    }
    /// Loads (and compiles) a sibling workflow of the app for `step.workflow`: the child runs as its own Temporal
    /// workflow with the same generic code, so the parent only needs its compiled DAG.
    #[activity]
    pub async fn cori_load_workflow(_ctx: ActivityContext, input: LoadWorkflowInput) -> Result<cori_compiler::Compiled, ActivityError> {
        let parent = resolve_root(&StepInput { run: Value::Null, user: Value::Null, params: Value::Null, root: input.root.clone(), bundle: input.bundle.clone(), store_server: None, record_dir: None, redact: vec![], step: StepSpec { name: String::new(), kind: String::new(), file: String::new(), literals: Default::default() }, steps: Map::new(), item: None, connector: None, llm: ResolvedLlm::default(), selector: None }).await.map_err(to_activity_error)?;
        // siblings live under the same `workflows/` folder: <app>/workflows/<parent> → <app>/workflows/<child>
        if !crate::workflow::valid_workflow_id(&input.id) { return Err(to_activity_error(anyhow!("InvalidInputError: child workflow `{}` is not a workflow folder name (letters, digits, `_`, `-`)", input.id))); }
        let dir = parent.parent().map(|p| p.join(&input.id)).ok_or_else(|| to_activity_error(anyhow!("InvalidInputError: workflow folder has no parent")))?;
        if !dir.join("workflow.yaml").exists() { return Err(to_activity_error(anyhow!("MissingCapabilityError: child workflow `{}` is not in this app ({} has no workflow.yaml)", input.id, dir.display()))); }
        cori_compiler::compile(&dir, &cori_compiler::Options { run_deno: false, sdk_path: None, connectors: vec![], deno: None }).map_err(|e| to_activity_error(anyhow!("StepFailedError: child workflow `{}` does not compile: {}", input.id, e.join("; "))))
    }
    /// Side-effect-only: tells the server something the workflow code cannot (an approval is pending, a run started).
    #[activity]
    pub async fn cori_emit(_ctx: ActivityContext, input: EmitInput) -> Result<Value, ActivityError> {
        if let Some(server) = &input.server {
            let url = format!("{}/v1/ingest/events", server.trim_end_matches('/'));
            let body = json!({ "run_id": input.run_id, "kind": input.kind, "at": chrono::Utc::now().to_rfc3339(), "payload": input.payload });
            crate::events::worker_auth(crate::http::client().post(url), input.realm.as_deref()).json(&body).timeout(std::time::Duration::from_secs(10)).send().await.map_err(|e| to_activity_error(anyhow!("Transient: emit: {e}")))?;
        }
        Ok(Value::Null)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmitInput { pub server: Option<String>, #[serde(default)] pub realm: Option<String>, pub run_id: String, pub kind: String, pub payload: Value }

fn to_activity_error(e: anyhow::Error) -> ActivityError { activity_error(e, None) }

/// A step error as Temporal's application failure: its `Type: ` prefix sets the type and whether a retry may help. A
/// `cori_step` attempt also says which it was (`FailedAttempt`).
fn activity_error(e: anyhow::Error, attempt: Option<u32>) -> ActivityError {
    let msg = e.to_string();
    tracing::warn!(error = %msg, "step failed");
    let (ty, non_retryable) = if let Some(i) = msg.find(": ") {
        let head = &msg[..i];
        match head {
            "SchemaValidationError" | "StepFailedError" | "InvalidInputError" | "MissingCapabilityError" | "AuthenticationError" | "BudgetExceeded" => (head.to_string(), true),
            "RuntimeUnavailableError" | "RateLimited" | "Transient" => (head.to_string(), false),
            _ => ("StepFailedError".to_string(), true),
        }
    } else { ("StepFailedError".to_string(), true) };
    ActivityError::application(ApplicationFailure::builder(e).type_name(ty).non_retryable(non_retryable).maybe_details(attempt.map(|attempt| FailedAttempt { attempt })).build())
}

fn ctx_json(i: &StepInput) -> Value {
    json!({ "params": i.params, "user": i.user, "run": i.run, "steps": i.steps, "item": i.item })
}

/// Where a worker caches fetched bundles: `$CORI_CACHE_DIR/bundles` (the image sets it to `/cache`), else `~/.cori/cache/bundles`.
pub fn bundle_cache_dir() -> std::path::PathBuf {
    match std::env::var_os("CORI_CACHE_DIR") {
        Some(d) if !d.is_empty() => std::path::PathBuf::from(d).join("bundles"),
        _ => dirs::home_dir().unwrap_or_else(|| Path::new("/tmp").to_path_buf()).join(".cori/cache/bundles"),
    }
}

/// Local mode: `root` is absolute. Bundle mode: fetch + cache the bundle, regenerate `.cori/`, return the workflow dir.
pub async fn resolve_root(i: &StepInput) -> Result<std::path::PathBuf> {
    let Some(b) = &i.bundle else { return Ok(Path::new(&i.root).to_path_buf()) };
    let cache = bundle_cache_dir().join(&b.hash);
    let dir = cache.join(&i.root);
    if !cache.join(".complete").exists() {
        let url = format!("{}/v1/bundles/{}/files", b.server.trim_end_matches('/'), b.hash);
        let files: std::collections::BTreeMap<String, String> = crate::events::worker_auth(crate::http::client().get(&url), b.realm.as_deref()).send().await.map_err(|e| anyhow!("RuntimeUnavailableError: bundle fetch {url}: {e}"))?.error_for_status().map_err(|e| anyhow!("RuntimeUnavailableError: bundle fetch: {e}"))?.json().await.map_err(|e| anyhow!("RuntimeUnavailableError: bundle decode: {e}"))?;
        for (rel, content) in &files { let p = cache.join(rel); if let Some(parent) = p.parent() { std::fs::create_dir_all(parent)?; } std::fs::write(&p, content)?; }
        std::fs::write(cache.join(".complete"), b"ok")?;
    }
    ensure_generated(&dir)?;
    Ok(dir)
}

/// Regenerates a bundle workflow's `.cori/` when it is missing or was written by another binary (an older worker, the
/// server's own copy in the bundle): its deno.json must map `@cori-do/sdk` to this binary's runtime folder, the only one
/// the runner may read.
pub fn ensure_generated(dir: &Path) -> Result<()> {
    let sdk = cori_compiler::runtime::runtime().map_err(|e| anyhow!("RuntimeUnavailableError: {e}"))?.sdk_index.display().to_string();
    let current = std::fs::read_to_string(dir.join(".cori/deno.json")).map(|t| t.contains(&sdk)).unwrap_or(false);
    if !current {
        cori_compiler::compile(dir, &cori_compiler::Options { run_deno: false, sdk_path: None, connectors: vec![], deno: None }).map_err(|e| anyhow!("StepFailedError: bundle workflow does not compile: {}", e.join("; ")))?;
    }
    Ok(())
}

async fn run_select(i: StepInput) -> Result<StepOutput> {
    let root = resolve_root(&i).await?;
    let runner = crate::sandbox::step_runner(&i, &root).await?;
    let file = root.join("steps").join(&i.step.file).display().to_string();
    let selector = i.selector.clone().ok_or_else(|| anyhow!("InvalidInputError: select needs a selector"))?;
    let v = match selector.as_str() {
        // `bind` and `validate` let the workflow drive steps whose execution happens elsewhere (native activities).
        "bind" => {
            let v = runner.call(json!({ "op": "bind", "file": file, "name": i.step.name, "ctx": ctx_json(&i) })).await?;
            match i.connector.as_ref().and_then(|c| c.audience.clone()) {
                Some(aud) if i.bundle.is_some() => {
                    // the native worker decides what an envelope without a principal token may do; say why it has none
                    let token = match principal_token_for(&i, &aud).await { Ok(t) => t, Err(e) => { tracing::warn!(step = %i.step.name, audience = %aud, error = %e, "no principal token for the native activity"); None } };
                    json!({ "input": v["input"], "principal_token": token })
                }
                _ => json!({ "input": v["input"], "principal_token": null }),
            }
        }
        "validate" => { let v = runner.call(json!({ "op": "validate", "file": file, "name": i.step.name, "value": i.item })).await?; v["output"].clone() }
        _ => { let v = runner.call(json!({ "op": "select", "file": file, "name": i.step.name, "selector": selector, "ctx": ctx_json(&i) })).await?; v["value"].clone() }
    };
    Ok(StepOutput { output: v, ..Default::default() })
}

// ---------------------------------------------------------------------------
// Native activities: dispatch by name on the developers' own task queue with the SDK's `UntypedActivity`
// (temporalio 1.0). The envelope travels as a plain `json/plain` payload so any official SDK decodes it.
// ---------------------------------------------------------------------------
use temporalio_common::data_converters::RawValue;
use temporalio_common::protos::temporal::api::common::v1::Payload;

pub fn json_raw(v: &Value) -> RawValue {
    let mut metadata = std::collections::HashMap::new();
    metadata.insert("encoding".to_string(), b"json/plain".to_vec());
    RawValue::new(vec![Payload { metadata, data: serde_json::to_vec(v).unwrap_or_default(), ..Default::default() }])
}

pub fn raw_json(r: RawValue) -> Result<Value> {
    let p = r.payloads.into_iter().next().ok_or_else(|| anyhow!("StepFailedError: native activity returned no payload"))?;
    if p.data.is_empty() { return Ok(Value::Null); }
    serde_json::from_slice(&p.data).map_err(|e| anyhow!("StepFailedError: native activity result is not JSON: {e}"))
}

/// Runs a step's attempt; `bound` gets its input once its binding gave it, failed attempt or not.
async fn run_step(i: StepInput, bound: &mut Option<Value>) -> Result<StepOutput> {
    let root = resolve_root(&i).await?;
    let runner = crate::sandbox::step_runner(&i, &root).await?;
    let file = root.join("steps").join(&i.step.file).display().to_string();
    let name = i.step.name.clone();
    let b = runner.call(json!({ "op": "bind", "file": file, "name": name, "ctx": ctx_json(&i) })).await?;
    let input = b["input"].clone();
    *bound = Some(input.clone());
    let mut out = StepOutput::default();
    match i.step.kind.as_str() {
        "code" => {
            let v = runner.call(json!({ "op": "code", "file": file, "name": name, "input": input })).await?;
            out.output = v["output"].clone();
        }
        "llm" => {
            let p = runner.call(json!({ "op": "prompt", "file": file, "name": name, "input": input })).await?;
            let tier = p["tier"].as_str().unwrap_or("medium").to_string();
            let wf = i.run["workflow"].as_str().unwrap_or("").to_string();
            let primary = binding_for(&i.llm, &wf, &name, "primary");
            let t0 = std::time::Instant::now();
            let a = llm_call(&i, &name, &tier, &p, &input, primary.as_ref()).await?;
            let latency = t0.elapsed().as_millis() as u64;
            out.cost_eur = a.cost_eur; out.tokens_in = a.tokens_in; out.tokens_out = a.tokens_out; if let Some(n) = &a.note { out.notes.push(n.clone()); }
            let validated = runner.call(json!({ "op": "validate", "file": file, "name": name, "value": a.raw.clone() })).await;
            let schema_valid = validated.is_ok();
            // call capture, whatever the answer's fate (the call is billed all the same): values under the manifest's
            // `redact:` paths are blanked before anything leaves the worker
            let server = i.bundle.as_ref().map(|b| b.server.clone());
            let secret_values = redact_values(&i);
            let input = redact_by_value(&input, &secret_values);
            let kept = match &validated { Ok(v) => v["output"].clone(), Err(_) => a.raw.clone() };
            // the instructions the model was given, for the run's inspection (left out when longer than a step's data)
            let prompt = p["prompt"].as_str().filter(|t| t.len() <= STEP_DATA_MAX).map(|t| redact_by_value(&json!(t), &secret_values));
            crate::events::emit_now(server.as_deref(), crate::workflow::BundleRef::realm_of(&i.bundle), i.run["id"].as_str().unwrap_or(""), "llm_call", json!({ "workflow": wf, "step": name, "tier": tier, "provider": a.provider, "model": a.model, "mode": "primary", "input": input, "output": kept, "prompt": prompt, "schema_valid": schema_valid, "latency_ms": latency, "tokens_in": a.tokens_in, "tokens_out": a.tokens_out, "cost_eur": a.cost_eur })).await;
            out.output = match validated {
                Ok(v) => v["output"].clone(),
                Err(e) => match p["fallback"].get("default") { Some(d) => { out.notes.push(format!("model output rejected ({e}); fallback default used")); d.clone() } None => return Err(e) },
            };
            // what the binary's modules add to the step (`hooks`): its output is decided by now
            let hooks = crate::hooks::llm_step_hooks();
            if !hooks.is_empty() {
                let step = crate::hooks::LlmStep { workflow: &wf, name: &name, tier: &tier, input: &input, i: &i, runner: &runner, file: &file, prompt: &p, server: server.as_deref() };
                for h in hooks { out.notes.extend(h.after_call(&step).await); }
            }
        }
        "http" => {
            let r = runner.call(json!({ "op": "request", "file": file, "name": name, "input": input })).await?;
            let req = r["request"].clone();
            let connector = i.connector.clone().ok_or_else(|| anyhow!("MissingCapabilityError: http step without connector"))?;
            let res = http_call(&i, &connector, &req).await?;
            out.notes.push(format!("{} {} → {}", req["method"].as_str().unwrap_or("?"), req["path"].as_str().unwrap_or("?"), res["status"]));
            let v = runner.call(json!({ "op": "parse", "kind": "http", "file": file, "name": name, "input": input, "raw": { "res": res } })).await?;
            out.output = v["output"].clone();
        }
        "mcp" | "store" => {
            let connector = i.connector.clone();
            let is_query = i.step.literals.contains_key("query");
            if is_query {
                let q = runner.call(json!({ "op": "query", "file": file, "name": name, "input": input })).await?;
                // fixture: `"query": { "<step>": [rows], "*": [rows] }` (per step) or `"query": [rows]` (every query step)
                let mocked = connector.as_ref().and_then(|c| c.mock.as_ref()).and_then(|m| m.get("query")).map(|q| match q {
                    Value::Object(by_step) => by_step.get(&name).or_else(|| by_step.get("*")).cloned().ok_or_else(|| anyhow!("StepFailedError: no query fixture for step `{name}` in the mock of `{}` (add `\"query\": {{ \"{name}\": [rows] }}`, or a `\"*\"` entry)", connector.as_ref().map(|c| c.name.as_str()).unwrap_or("default"))),
                    rows => Ok(rows.clone()),
                });
                let rows = match mocked {
                    Some(rows) => { out.notes.push("(mock: fixture rows, the SQL is not executed)".into()); rows? }
                    None if i.step.kind == "store" && (i.bundle.is_some() || i.store_server.is_some()) => {
                        let c = connector.clone().unwrap();
                        let v = store_rpc(&i, &c, "query", json!({ "sql": q["query"]["text"], "params": q["query"]["params"] })).await?;
                        let rows = v.get("rows").cloned().unwrap_or(Value::Array(vec![]));
                        record(&i, &c.name, "query", "", &rows);
                        rows
                    }
                    None => return Err(anyhow!("MissingCapabilityError: store `query` needs a Cori Store (hosted) or a dev mock (`[connectors.{}.dev] mock`)", connector.map(|c| c.name).unwrap_or("default".into()))),
                };
                out.notes.push(format!("query: {}", q["query"]["text"].as_str().unwrap_or("").split_whitespace().take(6).collect::<Vec<_>>().join(" ")));
                let v = runner.call(json!({ "op": "parse", "kind": "store_query", "file": file, "name": name, "input": input, "raw": { "rows": rows } })).await?;
                out.output = v["output"].clone();
            } else {
                let tool = i.step.literals.get("tool").and_then(|t| t.as_str()).unwrap_or("").to_string();
                let result = match connector.as_ref().and_then(|c| c.mock.as_ref()).and_then(|m| m.get("tools")).and_then(|t| t.get(&tool)) {
                    Some(v) => { out.notes.push(format!("{tool} (mock)")); v.clone() }
                    None if i.step.kind == "mcp" => {
                        let c = connector.clone().ok_or_else(|| anyhow!("MissingCapabilityError: mcp step without connector"))?;
                        let bearer = bearer_for(&i, &c).await?;
                        // a server over stdio is a program: it starts where the run's code runs (`sandbox`)
                        let v = match &c.command {
                            Some(cmd) if c.url.is_none() => crate::mcp::tool_result(crate::sandbox::mcp_stdio(&i, &root, cmd, "tools/call", json!({ "name": tool, "arguments": input })).await?)?,
                            _ => crate::mcp::call_tool(&c, &root, &tool, &input, bearer.as_deref(), egress_of(&i, c.egress)).await?,
                        };
                        out.notes.push(format!("{tool} via {}", if c.url.is_some() { "mcp http" } else { "mcp stdio" }));
                        record(&i, &c.name, "tools", &tool, &v);
                        v
                    }
                    None if i.step.kind == "store" && (i.bundle.is_some() || i.store_server.is_some()) => {
                        let c = connector.clone().ok_or_else(|| anyhow!("MissingCapabilityError: store step without connector"))?;
                        let v = store_rpc(&i, &c, &tool, input.clone()).await?;
                        out.notes.push(format!("{tool} via cori store"));
                        record(&i, &c.name, "tools", &tool, &v);
                        v
                    }
                    None => return Err(anyhow!("MissingCapabilityError: store tool `{tool}` on `{}` has no Cori Store in local mode and no dev mock", connector.map(|c| c.name).unwrap_or("default".into()))),
                };
                let v = runner.call(json!({ "op": "parse", "kind": i.step.kind, "file": file, "name": name, "input": input, "raw": { "result": result } })).await?;
                out.output = v["output"].clone();
            }
        }
        "activity" => {
            let act = i.step.literals.get("activity").and_then(|t| t.as_str()).unwrap_or("").to_string();
            let connector = i.connector.clone().ok_or_else(|| anyhow!("MissingCapabilityError: activity step without connector"))?;
            let result = match connector.mock.as_ref().and_then(|m| m.get("activities")).and_then(|a| a.get(&act)) {
                Some(v) => v.clone(),
                None => return Err(anyhow!("MissingCapabilityError: native activity `{act}` on `{}`: live dispatch to `{}` is not available in local mode; declare a dev mock", connector.name, connector.task_queue.clone().unwrap_or("<no queue>".into()))),
            };
            out.notes.push(format!("{act}@{} (mock)", i.step.literals.get("version").and_then(|v| v.as_f64()).map(|v| v as i64).unwrap_or(0)));
            let v = runner.call(json!({ "op": "validate", "file": file, "name": name, "value": result })).await?;
            out.output = v["output"].clone();
        }
        "cli" => {
            let c = runner.call(json!({ "op": "command", "file": file, "name": name, "input": input })).await?;
            let argv: Vec<String> = serde_json::from_value(c["argv"].clone())?;
            let argv0 = i.step.literals.get("argv0").and_then(|v| v.as_str()).unwrap_or("");
            if argv.first().map(|s| s.as_str()) != Some(argv0) { return Err(anyhow!("StepFailedError: command binary `{}` differs from the frozen `{argv0}`", argv.first().cloned().unwrap_or_default())); }
            // the binary is the connector's (its `command`, else its name, as `cori scaffold` writes it): a step cannot run
            // another program with the connector's place on the sandbox worker
            let connector = i.connector.clone().ok_or_else(|| anyhow!("MissingCapabilityError: cli step `{name}` names no connector"))?;
            let binary = connector.command.as_ref().and_then(|v| v.first()).cloned().unwrap_or_else(|| connector.name.clone());
            if argv0 != binary { return Err(anyhow!("InvalidInputError: cli step `{name}` runs `{argv0}`, but connector `{}` allows `{binary}` only", connector.name)); }
            let (stdout, stderr, code) = match connector.mock.as_ref() {
                Some(m) => (m.get("stdout").and_then(|s| s.as_str()).unwrap_or("").to_string(), String::new(), 0),
                None => crate::sandbox::exec(&i, &root, &argv).await?,
            };
            if code != 0 { return Err(anyhow!("StepFailedError: `{argv0}` exited {code}: {}", stderr.trim())); }
            out.notes.push(format!("{} (exit {code})", argv.join(" ").chars().take(80).collect::<String>()));
            let v = runner.call(json!({ "op": "parse", "kind": "cli", "file": file, "name": name, "input": input, "raw": { "stdout": stdout, "stderr": stderr, "exitCode": code } })).await?;
            out.output = v["output"].clone();
        }
        other => return Err(anyhow!("InvalidInputError: kind `{other}` is not an activity")),
    }
    Ok(out)
}

/// A token for the run's user, exchanged for `audience` (hosted mode only).
pub async fn principal_token_for(i: &StepInput, audience: &str) -> Result<Option<String>> {
    let Some(b) = &i.bundle else { return Ok(None) };
    let url = format!("{}/v1/leases", b.server.trim_end_matches('/'));
    // the lease names the connector (the server checks the audience is that connector's), not the step
    let connector = i.connector.as_ref().map(|c| c.name.clone()).unwrap_or_else(|| i.step.name.clone());
    let res = crate::events::worker_auth(crate::http::client().post(url), crate::workflow::BundleRef::realm_of(&i.bundle)).json(&json!({ "run_id": i.run["id"], "connector": connector, "audience": audience })).send().await.map_err(|e| anyhow!("Transient: lease: {e}"))?;
    if res.status().as_u16() == 403 { let v: Value = res.json().await.unwrap_or(Value::Null); return Err(anyhow!("AuthenticationError: {}", v["error"].as_str().unwrap_or("principal token refused"))); }
    let v: Value = res.json().await.map_err(|e| anyhow!("Transient: lease: {e}"))?;
    Ok(v["value"].as_str().map(String::from))
}

/// Bearer for a connector: a connection's access token for a deployment's OAuth connector; the run user's exchanged token
/// when the connector names an `audience`; else a broker lease (`secret = "name"`) in hosted mode; else the local env var.
async fn bearer_for(i: &StepInput, c: &ResolvedConnector) -> Result<Option<String>> {
    if let (Some(o), Some(_)) = (&c.oauth, &i.bundle) { return connection_token(i, c, &o.provider).await.map(Some); }
    if let (Some(aud), Some(_)) = (&c.audience, &i.bundle) { if let Some(t) = principal_token_for(i, aud).await? { return Ok(Some(t)); } }
    if let (Some(name), Some(b)) = (&c.secret, &i.bundle) {
        let url = format!("{}/v1/leases", b.server.trim_end_matches('/'));
        let res = crate::events::worker_auth(crate::http::client().post(url), crate::workflow::BundleRef::realm_of(&i.bundle)).json(&json!({ "run_id": i.run["id"], "connector": c.name, "secret": name })).send().await.map_err(|e| anyhow!("Transient: lease: {e}"))?;
        if res.status().as_u16() == 403 { return Err(anyhow!("AuthenticationError: lease refused for secret `{name}`")); }
        let v: Value = res.json().await.map_err(|e| anyhow!("Transient: lease: {e}"))?;
        return Ok(v["value"].as_str().map(String::from));
    }
    // in a hosted run only the org registry can name `auth_env` (merge_registry_hosted)
    Ok(c.auth_env.as_deref().and_then(env_credential))
}

/// The access token of the connection a deployment's OAuth connector calls with (the run's person's, or the app's): the
/// server refreshes it and says, when it refuses, what to connect.
async fn connection_token(i: &StepInput, c: &ResolvedConnector, provider: &str) -> Result<String> {
    let Some(b) = &i.bundle else { return Err(anyhow!("MissingCapabilityError: connector `{}` calls with a connection, which a hosted run leases", c.name)) };
    let url = format!("{}/v1/leases", b.server.trim_end_matches('/'));
    let res = crate::events::worker_auth(crate::http::client().post(url), crate::workflow::BundleRef::realm_of(&i.bundle)).json(&json!({ "run_id": i.run["id"], "connector": c.name, "connection": provider })).send().await.map_err(|e| anyhow!("Transient: lease: {e}"))?;
    let status = res.status().as_u16();
    let v: Value = res.json().await.unwrap_or(Value::Null);
    let why = v["error"].as_str().unwrap_or("").to_string();
    if status == 403 || status == 404 { return Err(anyhow!("AuthenticationError: {}", if why.is_empty() { format!("no connection to `{provider}` for `{}`", c.name) } else { why })); }
    if status >= 400 { return Err(anyhow!("Transient: lease {status}: {why}")); }
    v["value"].as_str().map(String::from).ok_or_else(|| anyhow!("Transient: the lease for `{}` held no token", c.name))
}

/// Hosted store call: the server's per-store MCP endpoint, acting as the run's user (worker token + run id).
async fn store_rpc(i: &StepInput, c: &ResolvedConnector, tool: &str, args: Value) -> Result<Value> {
    let server = i.store_server.clone().or_else(|| i.bundle.as_ref().map(|b| b.server.clone())).ok_or_else(|| anyhow!("MissingCapabilityError: store calls need the hosted server or `cori run --store-database-url`"))?;
    let store = c.base_url.clone().unwrap_or_else(|| c.name.clone());
    let url = format!("{}/v1/stores/{}/mcp", server.trim_end_matches('/'), store);
    let res = crate::events::worker_auth(crate::http::client().post(&url), crate::workflow::BundleRef::realm_of(&i.bundle)).header("x-cori-run", i.run["id"].as_str().unwrap_or(""))
        .header("x-cori-user", if i.store_server.is_some() { i.user.to_string() } else { String::new() })
        .json(&json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": tool, "arguments": args } })).send().await.map_err(|e| anyhow!("Transient: store: {e}"))?;
    let status = res.status().as_u16();
    let v: Value = res.json().await.map_err(|e| anyhow!("Transient: store: {e}"))?;
    // the server or its store database is unavailable (5xx): retried; a refusal (4xx) is not
    let why = v.get("error").and_then(|e| e.as_str()).unwrap_or("");
    if status >= 500 { return Err(anyhow!("Transient: store {status}: {why}")); }
    if status >= 400 { return Err(anyhow!("StepFailedError: store {status}: {why}")); }
    if let Some(e) = v.get("error") { return Err(anyhow!("StepFailedError: store: {}", e.get("message").and_then(|m| m.as_str()).unwrap_or(""))); }
    let r = &v["result"];
    if r.get("isError").and_then(|b| b.as_bool()) == Some(true) { return Err(anyhow!("StepFailedError: store: {}", r["content"][0]["text"].as_str().unwrap_or(""))); }
    Ok(r.get("structuredContent").cloned().unwrap_or(Value::Null))
}

/// The URL of an `http` step: the connector's `base_url` followed by the step's `path`, which must stay on the connector's
/// origin: the connector's credential goes with the request (`"@evil.example/"` or `".evil.example"` would carry it away).
fn connector_url(base: &str, path: &str) -> Result<reqwest::Url> {
    check_path(path)?;
    let origin = reqwest::Url::parse(base).map_err(|e| anyhow!("MissingCapabilityError: connector base_url `{base}`: {e}"))?.origin();
    let url = reqwest::Url::parse(&format!("{}{}", base.trim_end_matches('/'), path)).map_err(|e| anyhow!("InvalidInputError: http path `{path}`: {e}"))?;
    if url.origin() != origin { return Err(anyhow!("InvalidInputError: http path `{path}` leaves the connector's origin {}", origin.ascii_serialization())); }
    Ok(url)
}

fn check_path(path: &str) -> Result<()> {
    if !path.starts_with('/') || path.starts_with("//") { return Err(anyhow!("InvalidInputError: http path `{path}` must start with a single `/`: it is appended to the connector's base_url")); }
    Ok(())
}

/// What a request of this run may reach: the server's decision (`egress`) in a hosted run; anything in a local run, whose
/// network is the developer's own.
fn egress_of(i: &StepInput, hosted: crate::egress::Egress) -> crate::egress::Egress {
    if i.bundle.is_some() { hosted } else { crate::egress::Egress::Any }
}

/// Whether a credential may be read from the worker's environment variable `name`: never one of Cori's own settings
/// (`CORI_WORKER_TOKEN`, `CORI_TEMPORAL_API_KEY`, …), which would hand the worker's powers to a step.
/// A credential read from an environment variable the configuration names: never one of Cori's own (`CORI_*`: the
/// master key, the ingest token, database URLs), which a builder or a connector could otherwise send out or sign with.
pub fn env_credential(name: &str) -> Option<String> {
    if name.starts_with("CORI_") { return None; }
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

async fn http_call(i: &StepInput, c: &ResolvedConnector, req: &Value) -> Result<Value> {
    let method = req["method"].as_str().unwrap_or("GET").to_uppercase();
    let path = req["path"].as_str().unwrap_or("/").to_string();
    check_path(&path)?;   // mocked or live alike
    if let Some(m) = c.mock.as_ref().and_then(|m| m.get("http")).and_then(|h| h.as_object()) {
        let key = format!("{method} {path}");
        let hit = m.get(&key).or_else(|| m.iter().find(|(k, _)| k.starts_with(&format!("{method} ")) && path.starts_with(&k[method.len() + 1..])).map(|(_, v)| v));
        return match hit {
            Some(v) => Ok(json!({ "status": v.get("status").cloned().unwrap_or(json!(200)), "headers": {}, "json": v.get("json").cloned().unwrap_or(Value::Null), "text": v.get("text").cloned().unwrap_or(json!("")) })),
            None => Err(anyhow!("StepFailedError: no mock for `{key}` on connector `{}`", c.name)),
        };
    }
    let base = c.base_url.clone().ok_or_else(|| anyhow!("MissingCapabilityError: connector `{}` has no base_url and no dev mock", c.name))?;
    let url = connector_url(&base, &path)?;
    let client = crate::egress::client_for(&url, egress_of(i, c.egress), None).await?;
    let mut r = client.request(reqwest::Method::from_bytes(method.as_bytes())?, url);
    if let Some(q) = req["query"].as_object() { let pairs: Vec<(String, String)> = q.iter().filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())).or_else(|| Some((k.clone(), v.to_string())))).collect(); r = r.query(&pairs); }
    if let Some(h) = req["headers"].as_object() { for (k, v) in h { if let Some(s) = v.as_str() { r = r.header(k, s); } } }
    if let Some(tok) = bearer_for(i, c).await? { r = r.bearer_auth(tok); }
    if !req["body"].is_null() { r = r.json(&req["body"]); }
    let res = r.send().await.map_err(|e| anyhow!("Transient: {e}"))?;
    let status = res.status().as_u16();
    let headers: Map<String, Value> = res.headers().iter().map(|(k, v)| (k.to_string(), json!(v.to_str().unwrap_or("")))).collect();
    let text = res.text().await.unwrap_or_default();
    let js: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    record(i, &c.name, "http", &format!("{method} {path}"), &json!({ "status": status, "json": js, "text": if js.is_null() { text.clone() } else { String::new() } }));
    Ok(json!({ "status": status, "headers": headers, "json": js, "text": text }))
}

/// `cori run --record`: append a live result as a fixture line (`<record_dir>/<connector>.jsonl`), secrets-looking
/// keys blanked. The CLI merges the lines into `fixtures/<connector>.json` when the run ends.
fn record(i: &StepInput, connector: &str, section: &str, key: &str, value: &Value) {
    let Some(dir) = &i.record_dir else { return };
    fn scrub(v: &Value) -> Value { match v { Value::Object(m) => Value::Object(m.iter().map(|(k, v)| { let kl = k.to_lowercase(); if kl.contains("password") || kl.contains("secret") || kl.contains("token") || kl == "authorization" { (k.clone(), json!("<redacted>")) } else { (k.clone(), scrub(v)) } }).collect()), Value::Array(a) => Value::Array(a.iter().map(scrub).collect()), o => o.clone() } }
    let line = json!({ "connector": connector, "section": section, "key": key, "value": scrub(value) }).to_string() + "\n";
    if std::fs::create_dir_all(dir).is_ok() { if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(std::path::Path::new(dir).join(format!("{connector}.jsonl"))) { use std::io::Write; let _ = f.write_all(line.as_bytes()); } }
}

pub(crate) fn binding_for(llm: &crate::config::ResolvedLlm, wf: &str, step: &str, mode: &str) -> Option<crate::config::Binding> {
    llm.bindings.get(&format!("{wf}:{step}:{mode}")).or_else(|| llm.bindings.get(&format!("*:*:{mode}"))).cloned()
}

/// Which provider and model answer a step: its binding (console), else the app's own `[llm]` provider and tier table when
/// the app set one, else the org's default model for the tier (console: Settings › AI models), else the app's `[llm]` as is.
fn route_llm(llm: &crate::config::ResolvedLlm, tier: &str, binding: Option<&crate::config::Binding>) -> (String, Option<String>) {
    if let Some(b) = binding { return (b.provider.clone(), Some(b.model.clone())); }
    if llm.app_provider && (llm.provider == "mock" || llm.tiers.contains_key(tier)) { return (llm.provider.clone(), llm.tiers.get(tier).cloned()); }
    if let Some(b) = llm.org_tiers.get(tier) { return (b.provider.clone(), Some(b.model.clone())); }
    (llm.provider.clone(), llm.tiers.get(tier).cloned())
}

/// The key for one call. Hosted runs first ask the server (`POST /v1/leases` with `llm`): it refuses once the app's monthly
/// LLM budget is spent (`BudgetExceeded`, not retried) and hands out the key of an org provider from the broker. Otherwise
/// the provider's `api_key_env` on this worker, sent to the provider's own endpoint only: in a hosted run, a provider the app
/// points at a base_url of its choosing gets its key from the org (console), never from the worker's environment.
async fn llm_key(i: &StepInput, provider: &str, model: &str, tokens_in: u64, pc: Option<&crate::config::ProviderCfg>, app_base_url: bool) -> Result<String> {
    if let Some(b) = &i.bundle {
        let url = format!("{}/v1/leases", b.server.trim_end_matches('/'));
        // the model and the prompt's size: the server sets aside what the call may cost against the budgets
        let res = crate::events::worker_auth(crate::http::client().post(url), crate::workflow::BundleRef::realm_of(&i.bundle)).json(&json!({ "run_id": i.run["id"], "llm": provider, "model": model, "tokens_in": tokens_in })).send().await.map_err(|e| anyhow!("Transient: llm lease: {e}"))?;
        let status = res.status().as_u16();
        let v: Value = res.json().await.unwrap_or(Value::Null);
        let msg = v["error"].as_str().unwrap_or("").to_string();
        if status == 402 { return Err(anyhow!("BudgetExceeded: {msg}")); }
        if status == 403 { return Err(anyhow!("AuthenticationError: {msg}")); }
        if status >= 400 { return Err(anyhow!("Transient: llm lease returned {status}: {msg}")); }
        if let Some(k) = v["value"].as_str() { return Ok(k.to_string()); }
    } else if let Some(sec) = pc.and_then(|c| c.secret.as_ref()) {
        return Err(anyhow!("MissingCapabilityError: llm provider `{provider}` keeps its key in the server's broker (`{sec}`); local runs use `cori run --mock-llm` or an `api_key_env`"));
    }
    match pc.and_then(|c| c.api_key_env.clone()).or_else(|| i.llm.api_key_env.clone()) {
        Some(e) if e.starts_with("CORI_") => Err(anyhow!("MissingCapabilityError: llm provider `{provider}` names `{e}` as its key: Cori's own settings are never handed to a step")),
        Some(e) if i.bundle.is_some() && app_base_url => Err(anyhow!("MissingCapabilityError: llm provider `{provider}` calls a base_url set by the app, so a hosted run does not give it the worker's `{e}`: declare the provider for the org (console, LLM providers) with its key")),
        Some(e) => env_credential(&e).ok_or_else(|| anyhow!("MissingCapabilityError: llm provider `{provider}` reads its key from `{e}`, which is not set (export it; for local runs use `cori run --mock-llm` or`[llm.dev] provider = \"mock\"` in cori.toml)")),
        None => Ok(String::new()),
    }
}

/// A model's answer to a step's prompt: the raw output, what it cost at the run's prices (the server prices hosted calls
/// again itself), its tokens, a note for the step, and who answered.
pub(crate) struct LlmAnswer { pub raw: Value, pub cost_eur: f64, pub tokens_in: u64, pub tokens_out: u64, pub note: Option<String>, pub provider: String, pub model: String }

/// One call of a step's prompt. A binding overrides the app's provider/tier table.
pub(crate) async fn llm_call(i: &StepInput, step: &str, tier: &str, p: &Value, input: &Value, binding: Option<&crate::config::Binding>) -> Result<LlmAnswer> {
    let (provider, model) = route_llm(&i.llm, tier, binding);
    if provider.is_empty() { return Err(anyhow!("MissingCapabilityError: no model for tier `{tier}`: the app declares no `[llm]` provider and the org has no default model for this tier (console, LLM)")); }
    // the files the step gives its model (`files`): each of a type a model reads, whoever answers (a fixture too)
    let refs = file_refs(step, p)?;
    if provider == "mock" {
        let m = i.llm.mock.as_ref().ok_or_else(|| anyhow!("MissingCapabilityError: llm provider `mock` needs `[llm] mock = \"fixtures/llm.json\"`"))?;
        // a bound mock "model" answers from `<step>@<model>` when present, else from `<step>`
        let model = model.unwrap_or_else(|| "mock".into());
        let tpl = m.get(&format!("{step}@{model}")).cloned().or_else(|| m.get(step).cloned()).ok_or_else(|| anyhow!("StepFailedError: no llm fixture for step `{step}`"))?;
        return Ok(LlmAnswer { raw: template(tpl, input), cost_eur: 0.0, tokens_in: 0, tokens_out: 0, note: Some(format!("tier {tier} · {model}")), provider, model });
    }
    let pc = i.llm.providers.get(&provider);
    let kind = pc.and_then(|c| c.kind.clone()).unwrap_or_else(|| provider.clone());
    if !matches!(kind.as_str(), "openai" | "vllm" | "anthropic") { return Err(anyhow!("MissingCapabilityError: unknown llm provider `{provider}` (not declared in cori.toml nor by the org)")); }
    let model = model.ok_or_else(|| anyhow!("MissingCapabilityError: no model bound to tier `{tier}` (cori.toml `[llm.tiers]`, a binding, or the org's tier defaults)"))?;
    // whether the app chose where the call goes: an org provider (console) carries its broker `secret` and its own base_url
    let org_provider = pc.map(|c| c.secret.is_some()).unwrap_or(false);
    let app_base_url = !org_provider && (pc.and_then(|c| c.base_url.as_ref()).is_some() || (kind != "anthropic" && i.llm.base_url.is_some()));
    let files = load_files(i, step, &refs).await?;
    // about four characters a token: what the prompt and its schema take, and an estimate for the files
    let tokens_in = ((p["prompt"].as_str().map(str::len).unwrap_or(0) + p["schema"].to_string().len()) / 4) as u64 + files.iter().map(Attachment::tokens).sum::<u64>();
    let key = llm_key(i, &provider, &model, tokens_in, pc, app_base_url).await?;
    let base = llm_base(&kind, pc, i.llm.base_url.as_deref(), org_provider);
    let egress = egress_of(i, if org_provider { i.llm.egress.org } else { i.llm.egress.app });
    let (parsed, tin, tout) = if kind == "anthropic" {
        anthropic_call(&base, &key, &model, p, &files, egress).await?
    } else {
        openai_call(&provider, &base, &key, &model, p, &files, egress).await?
    };
    let price = pc.and_then(|c| c.models.get(&model));
    let cost = price.map(|m| (tin as f64 * m.input_eur_per_mtok + tout as f64 * m.output_eur_per_mtok) / 1_000_000.0).unwrap_or(0.0);
    Ok(LlmAnswer { raw: parsed, cost_eur: cost, tokens_in: tin, tokens_out: tout, note: Some(format!("tier {tier} → {provider}/{model}")), provider, model })
}

/// The most bytes of files one model call sends (`files` on an `llm` step): the providers refuse requests not much larger.
pub const LLM_FILES_MAX: u64 = 20 * 1024 * 1024;

/// What a model reads of a file, by its type: PDFs as documents, four image formats, and text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelInput { Document, Image, Text }

pub fn model_input(content_type: &str) -> Option<ModelInput> {
    match content_type {
        "application/pdf" => Some(ModelInput::Document),
        "image/png" | "image/jpeg" | "image/gif" | "image/webp" => Some(ModelInput::Image),
        t if t.starts_with("text/") || matches!(t, "application/json" | "application/xml" | "application/yaml" | "image/svg+xml" | "message/rfc822") => Some(ModelInput::Text),
        _ => None,
    }
}

/// A file a step gives its model, as `files` named it.
#[derive(Debug, Clone)]
struct FileRef { id: String, name: String, content_type: String }

/// The files of a step's prompt (`p["files"]`, from its `files`), each of a type a model reads.
fn file_refs(step: &str, p: &Value) -> Result<Vec<FileRef>> {
    let Some(list) = p["files"].as_array() else { return Ok(vec![]) };
    let mut out = Vec::new();
    for f in list {
        let id = f["id"].as_str().filter(|id| cori_compiler::files::valid_id(id)).ok_or_else(|| anyhow!("InvalidInputError: step `{step}`: files() gave {f}, which is not a file (a parameter of type file or files)"))?;
        let r = FileRef { id: id.to_string(), name: f["name"].as_str().unwrap_or(id).to_string(), content_type: f["content_type"].as_str().unwrap_or("").to_string() };
        refuse_unreadable(step, &r.name, &r.content_type)?;
        out.push(r);
    }
    Ok(out)
}

fn refuse_unreadable(step: &str, name: &str, content_type: &str) -> Result<()> {
    if model_input(content_type).is_some() { return Ok(()); }
    Err(anyhow!("InvalidInputError: step `{step}`: a model reads PDFs, PNG, JPEG, GIF and WebP images, and text files; `{name}` is {}", if content_type.is_empty() { "of no known type".to_string() } else { content_type.to_string() }))
}

/// A file's bytes for one model call, with the type and the name the server keeps for it.
#[derive(Debug, Clone)]
pub struct Attachment { pub name: String, pub content_type: String, pub bytes: Vec<u8> }

impl Attachment {
    /// About what the file costs the model in tokens: text at four characters a token, an image about 1,600, a PDF at
    /// least 1,500 (its pages are read as text and as images). The budget's reservation, not the bill.
    fn tokens(&self) -> u64 {
        match model_input(&self.content_type) {
            Some(ModelInput::Text) => self.bytes.len() as u64 / 4,
            Some(ModelInput::Image) => 1_600,
            Some(ModelInput::Document) => (self.bytes.len() as u64 / 30).max(1_500),
            None => 0,
        }
    }
    fn base64(&self) -> String { use base64::Engine; base64::engine::general_purpose::STANDARD.encode(&self.bytes) }
    /// A text file as the model reads it: its name and type around its content.
    fn text(&self) -> String {
        let body = String::from_utf8_lossy(self.bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&self.bytes)).into_owned();
        format!("<file name={:?} type={:?}>\n{body}\n</file>", self.name, self.content_type)
    }
}

/// The bytes of the files a step gives its model: a hosted run's from the server (`GET /v1/runs/{run}/files/{id}`, which
/// gives a run the files of its own app and release only), a local run's from the app's `.cori/files/` (`cori run`).
async fn load_files(i: &StepInput, step: &str, refs: &[FileRef]) -> Result<Vec<Attachment>> {
    let mut out = Vec::new();
    let mut total = 0u64;
    for r in refs {
        let a = match &i.bundle {
            Some(b) => {
                let run = i.run["id"].as_str().unwrap_or("");
                let url = format!("{}/v1/runs/{run}/files/{}", b.server.trim_end_matches('/'), r.id);
                let res = crate::events::worker_auth(crate::http::client().get(&url), b.realm.as_deref()).send().await.map_err(|e| anyhow!("Transient: file {}: {e}", r.id))?;
                let status = res.status().as_u16();
                if status == 404 { return Err(anyhow!("StepFailedError: step `{step}`: no file `{}` ({}) for this run: a run reads the files of its own app and release, while they are kept", r.name, r.id)); }
                if status >= 500 || status == 429 { return Err(anyhow!("Transient: file {}: the server answered {status}", r.id)); }
                if status >= 400 { let why = res.text().await.unwrap_or_default(); return Err(anyhow!("StepFailedError: file {}: {status} {}", r.id, why.chars().take(200).collect::<String>())); }
                let header = |h: &str| res.headers().get(h).and_then(|v| v.to_str().ok()).map(String::from);
                let content_type = header("content-type").unwrap_or_else(|| r.content_type.clone());
                let name = header("x-cori-file-name").and_then(|n| urlencoding_decode(&n)).unwrap_or_else(|| r.name.clone());
                let bytes = res.bytes().await.map_err(|e| anyhow!("Transient: file {}: {e}", r.id))?.to_vec();
                Attachment { name, content_type, bytes }
            }
            None => {
                let dir = local_files_dir(Path::new(&i.root)).ok_or_else(|| anyhow!("StepFailedError: {} is not a workflow folder of an app", i.root))?;
                let bytes = std::fs::read(dir.join(&r.id)).map_err(|_| anyhow!("StepFailedError: step `{step}`: no local file `{}` ({}): `cori run` keeps the files it was given in {}", r.name, r.id, dir.display()))?;
                let meta: Value = std::fs::read_to_string(dir.join(format!("{}.json", r.id))).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(Value::Null);
                Attachment { name: meta["name"].as_str().unwrap_or(&r.name).to_string(), content_type: meta["content_type"].as_str().unwrap_or(&r.content_type).to_string(), bytes }
            }
        };
        // the server's record of the type, not the reference a step passed on
        refuse_unreadable(step, &a.name, &a.content_type)?;
        total += a.bytes.len() as u64;
        if total > LLM_FILES_MAX { return Err(anyhow!("InvalidInputError: step `{step}`: its files come to more than {} for one model call; give it fewer (a `map` over the files, one call each)", cori_compiler::files::human_size(LLM_FILES_MAX))); }
        out.push(a);
    }
    Ok(out)
}

/// `%xx` escapes undone (the server sends a file's name so, as headers are ASCII).
fn urlencoding_decode(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut k = 0;
    while k < b.len() {
        if b[k] == b'%' && k + 2 < b.len() {
            let h = std::str::from_utf8(&b[k + 1..k + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok())?;
            out.push(h); k += 3;
        } else { out.push(b[k]); k += 1; }
    }
    String::from_utf8(out).ok()
}

/// Where `cori run` keeps the files a local run was given: `<app>/.cori/files/`, the app two folders above the workflow's.
pub fn local_files_dir(workflow_root: &Path) -> Option<std::path::PathBuf> {
    workflow_root.parent().and_then(|p| p.parent()).map(|app| app.join(".cori").join("files"))
}

/// A chat completion's user content: the prompt alone, or the files (PDFs as `file` parts, images as data URLs, text as
/// text) and then the prompt.
fn openai_content(prompt: &str, files: &[Attachment]) -> Value {
    if files.is_empty() { return json!(prompt); }
    let mut parts: Vec<Value> = files.iter().filter_map(|f| match model_input(&f.content_type)? {
        ModelInput::Document => Some(json!({ "type": "file", "file": { "filename": f.name, "file_data": format!("data:{};base64,{}", f.content_type, f.base64()) } })),
        ModelInput::Image => Some(json!({ "type": "image_url", "image_url": { "url": format!("data:{};base64,{}", f.content_type, f.base64()) } })),
        ModelInput::Text => Some(json!({ "type": "text", "text": f.text() })),
    }).collect();
    parts.push(json!({ "type": "text", "text": prompt }));
    Value::Array(parts)
}

/// A Messages API user content: the prompt alone, or the files (PDFs as `document` blocks, images as `image` blocks,
/// text as text) and then the prompt.
fn anthropic_content(prompt: &str, files: &[Attachment]) -> Value {
    if files.is_empty() { return json!(prompt); }
    let mut blocks: Vec<Value> = files.iter().filter_map(|f| match model_input(&f.content_type)? {
        ModelInput::Document => Some(json!({ "type": "document", "source": { "type": "base64", "media_type": f.content_type, "data": f.base64() }, "title": f.name })),
        ModelInput::Image => Some(json!({ "type": "image", "source": { "type": "base64", "media_type": f.content_type, "data": f.base64() } })),
        ModelInput::Text => Some(json!({ "type": "text", "text": f.text() })),
    }).collect();
    blocks.push(json!({ "type": "text", "text": prompt }));
    Value::Array(blocks)
}

/// Where a provider's calls go. An org provider (console, its key in the broker) goes to its own base_url or its kind's
/// public endpoint, never to an `[llm] base_url` the app set: the app would receive the org's key.
fn llm_base(kind: &str, pc: Option<&crate::config::ProviderCfg>, app_base_url: Option<&str>, org_provider: bool) -> String {
    let own = pc.and_then(|c| c.base_url.clone());
    if kind == "anthropic" { return own.unwrap_or_else(|| "https://api.anthropic.com/v1".into()); }
    let app = if org_provider { None } else { app_base_url.map(String::from) };
    own.or(app).unwrap_or_else(|| "https://api.openai.com/v1".into())
}

/// The instruction a prompt carries when the endpoint did not hold the model to the step's schema.
fn with_schema(p: &Value) -> String {
    format!("{}\n\nAnswer with a single JSON value, and nothing else, matching this JSON schema:\n{}", p["prompt"].as_str().unwrap_or(&p["prompt"].to_string()), p["schema"])
}

/// An answer without the code fence a model may wrap its JSON in.
fn unfenced(text: &str) -> &str { text.trim().trim_start_matches("```json").trim_start_matches("```").trim_end_matches("```").trim() }

/// A provider's body as JSON: a refusal says why in its body, JSON or not, while an answer that is no JSON is a broken
/// exchange, tried again.
fn provider_json(status: u16, text: &str) -> Result<Value> {
    match serde_json::from_str(text) { Ok(v) => Ok(v), Err(e) if status < 400 => Err(anyhow!("Transient: {e}")), Err(_) => Ok(Value::Null) }
}

/// A call the provider refused, for the step's error (the run's page shows it): `provider returned <status>: <why>`. Why is
/// the error's `message` (OpenAI, Anthropic), with OpenRouter its upstream provider's own words (`metadata.raw`) and name,
/// else a `message` or `detail` at the top (vLLM, other servers), else the body as is. One line of at most 300 characters,
/// the call's key blanked.
fn refused(status: u16, v: &Value, text: &str, key: &str) -> String {
    let e = &v["error"];
    let said = |m: &Value| m.as_str().filter(|s| !s.trim().is_empty()).map(String::from);
    // an upstream's raw answer may be JSON itself: its message then
    let raw = match &e["metadata"]["raw"] { Value::String(_) | Value::Null => said(&e["metadata"]["raw"]), o => Some(o.to_string()) }
        .map(|r| serde_json::from_str::<Value>(&r).ok().and_then(|j| said(&j["error"]["message"]).or_else(|| said(&j["message"]))).unwrap_or(r));
    let why = raw.or_else(|| [&e["message"], e, &v["message"], &v["detail"]].into_iter().find_map(said)).unwrap_or_else(|| text.to_string());
    let mut why = why.split_whitespace().collect::<Vec<_>>().join(" ");
    if !key.trim().is_empty() { why = why.replace(key, "<redacted>"); }
    if why.chars().count() > 300 { why = why.chars().take(300).collect::<String>() + "…"; }
    let by = e["metadata"]["provider_name"].as_str().map(|p| format!(" ({p})")).unwrap_or_default();
    if why.is_empty() { format!("provider returned {status}{by}") } else { format!("provider returned {status}: {why}{by}") }
}

/// OpenAI-compatible chat completions with a JSON-schema output (`response_format`). An endpoint may not hold its model to
/// the schema (one that knows JSON mode only, as many free and self-hosted ones): an answer that is no JSON, in prose or
/// in a code fence, is asked for once more, the schema in the prompt and JSON mode on. The step's own validation checks
/// the answer anyway.
async fn openai_call(provider: &str, base: &str, key: &str, model: &str, p: &Value, files: &[Attachment], egress: crate::egress::Egress) -> Result<(Value, u64, u64)> {
    let url = reqwest::Url::parse(&format!("{}/chat/completions", base.trim_end_matches('/'))).map_err(|e| anyhow!("MissingCapabilityError: llm provider `{provider}` base_url `{base}`: {e}"))?;
    let client = crate::egress::client_for(&url, egress, None).await?;
    let content = if files.is_empty() { p["prompt"].clone() } else { openai_content(p["prompt"].as_str().unwrap_or(&p["prompt"].to_string()), files) };
    let mut body = json!({ "model": model, "messages": [{ "role": "user", "content": content }], "response_format": { "type": "json_schema", "json_schema": { "name": "output", "schema": p["schema"], "strict": false } } });
    let (mut tin, mut tout, mut first) = (0, 0, String::new());
    for attempt in 0..2 {
        // a keyless endpoint (a gateway on the private network) gets no Authorization header
        let req = client.post(url.clone()).json(&body);
        let req = if key.is_empty() { req } else { req.bearer_auth(key) };
        let res = req.send().await.map_err(|e| anyhow!("Transient: {e}"))?;
        let status = res.status().as_u16();
        let text = res.text().await.map_err(|e| anyhow!("Transient: {e}"))?;
        let v = provider_json(status, &text)?;
        if status == 429 || status >= 500 { return Err(anyhow!("RateLimited: {}", refused(status, &v, &text, key))); }
        // an endpoint without JSON mode: the first answer is what the step validates
        if status == 400 && attempt == 1 { return Ok((Value::String(first), tin, tout)); }
        if status >= 400 { return Err(anyhow!("StepFailedError: {}", refused(status, &v, &text, key))); }
        tin += v["usage"]["prompt_tokens"].as_u64().unwrap_or(0);
        tout += v["usage"]["completion_tokens"].as_u64().unwrap_or(0);
        let content = v["choices"][0]["message"]["content"].as_str().unwrap_or("").to_string();
        if let Ok(parsed) = serde_json::from_str::<Value>(unfenced(&content)) { return Ok((parsed, tin, tout)); }
        if attempt == 1 { return Ok((Value::String(content), tin, tout)); }
        first = content;
        body = json!({ "model": model, "messages": [{ "role": "user", "content": openai_content(&with_schema(p), files) }], "response_format": { "type": "json_object" } });
    }
    unreachable!()
}

/// One structured call to `model` (`cori preflight`): what a step's call does, with a schema of one field. Answers what came
/// back, parsed.
pub async fn llm_probe(kind: &str, base: &str, key: &str, model: &str, egress: crate::egress::Egress) -> Result<Value> {
    let p = json!({ "prompt": "Answer with a JSON object whose field `ok` is true.", "schema": { "type": "object", "properties": { "ok": { "type": "boolean" } }, "required": ["ok"] } });
    let (v, _, _) = if kind == "anthropic" { anthropic_call(base, key, model, &p, &[], egress).await? } else { openai_call("probe", base, key, model, &p, &[], egress).await? };
    Ok(v)
}

/// Anthropic Messages API with a JSON-schema output (`output_config.format`). A schema the API does not accept for
/// structured output is retried once as an instruction in the prompt; the step's own validation checks the answer anyway.
async fn anthropic_call(base: &str, key: &str, model: &str, p: &Value, files: &[Attachment], egress: crate::egress::Egress) -> Result<(Value, u64, u64)> {
    let url = reqwest::Url::parse(&format!("{}/messages", base.trim_end_matches('/'))).map_err(|e| anyhow!("MissingCapabilityError: llm base_url `{base}`: {e}"))?;
    let client = crate::egress::client_for(&url, egress, None).await?;
    let content = if files.is_empty() { p["prompt"].clone() } else { anthropic_content(p["prompt"].as_str().unwrap_or(&p["prompt"].to_string()), files) };
    let mut body = json!({ "model": model, "max_tokens": 16000, "messages": [{ "role": "user", "content": content }], "output_config": { "format": { "type": "json_schema", "schema": p["schema"] } } });
    for attempt in 0..2 {
        let res = client.post(url.clone()).header("x-api-key", key).header("anthropic-version", "2023-06-01").json(&body).send().await.map_err(|e| anyhow!("Transient: {e}"))?;
        let status = res.status().as_u16();
        let text = res.text().await.map_err(|e| anyhow!("Transient: {e}"))?;
        let v = provider_json(status, &text)?;
        if status == 429 || status == 529 || status >= 500 { return Err(anyhow!("RateLimited: {}", refused(status, &v, &text, key))); }
        if status == 400 && attempt == 0 {
            body = json!({ "model": model, "max_tokens": 16000, "messages": [{ "role": "user", "content": anthropic_content(&with_schema(p), files) }] });
            continue;
        }
        if status >= 400 { return Err(anyhow!("StepFailedError: {}", refused(status, &v, &text, key))); }
        if v["stop_reason"] == "refusal" { return Err(anyhow!("StepFailedError: the model declined: {}", v["stop_details"]["explanation"].as_str().unwrap_or(""))); }
        let text: String = v["content"].as_array().map(|a| a.iter().filter(|b| b["type"] == "text").filter_map(|b| b["text"].as_str()).collect()).unwrap_or_default();
        let parsed = serde_json::from_str(unfenced(&text)).unwrap_or(Value::String(text.clone()));
        return Ok((parsed, v["usage"]["input_tokens"].as_u64().unwrap_or(0), v["usage"]["output_tokens"].as_u64().unwrap_or(0)));
    }
    unreachable!()
}

/// `{{a.b}}` in fixture strings is replaced by the value at that path in the step input.
fn template(v: Value, input: &Value) -> Value {
    match v {
        Value::String(s) if s.contains("{{") => Value::String(crate::template::render(&s, |path| {
            let mut cur = input; for seg in path.split('.') { cur = cur.get(seg).unwrap_or(&Value::Null); }
            match cur { Value::String(x) => x.clone(), Value::Null => String::new(), other => other.to_string() }
        })),
        Value::Array(a) => Value::Array(a.into_iter().map(|x| template(x, input)).collect()),
        Value::Object(o) => Value::Object(o.into_iter().map(|(k, x)| (k, template(x, input))).collect()),
        other => other,
    }
}

/// The current values of the manifest's `redact:` paths, resolved against the step outputs this step references.
fn redact_values(i: &StepInput) -> Vec<String> {
    let mut out = Vec::new();
    for p in &i.redact {
        let parts: Vec<&str> = p.trim_start_matches("steps.").split('.').collect();
        let mut v: Option<&Value> = i.steps.get(parts[0]);
        for part in &parts[1..] { v = v.and_then(|x| x.get(*part)); }
        if let Some(Value::String(s)) = v { if s.len() >= 3 { out.push(s.clone()); } }
    }
    out
}

/// The largest input or output a step's event carries, in bytes of JSON: a bigger one is left out and its size said
/// (`too_large`). The run's own record keeps every output whole.
const STEP_DATA_MAX: usize = 64 * 1024;

/// What a step's event shows of its data, for the run's inspection (`GET /v1/apps/{app}/runs/{run}/steps`): its input as
/// its LLM capture keeps it (the values under the manifest's `redact:` paths blanked), its output as the run stores it
/// (those paths under the step blanked; for a step run per item, those under a map's `items` too, where its output goes).
struct StepData { name: String, secrets: Vec<String>, redact: Vec<String>, per_item: bool }

impl StepData {
    fn of(i: &StepInput) -> Self { Self { name: i.step.name.clone(), secrets: redact_values(i), redact: i.redact.clone(), per_item: i.item.is_some() } }

    /// Sets the event's `input` and `output` (those the attempt has), or their sizes under `too_large`.
    fn add(&self, ev: &mut Value, input: Option<&Value>, output: Option<&Value>) {
        let mut too_large = Map::new();
        for (key, v) in [("input", input.map(|v| redact_by_value(v, &self.secrets))), ("output", output.map(|v| self.stored(v)))] {
            let Some(v) = v else { continue };
            let n = v.to_string().len();
            if n > STEP_DATA_MAX { too_large.insert(key.into(), json!(n)); } else { ev[key] = v; }
        }
        if !too_large.is_empty() { ev["too_large"] = Value::Object(too_large); }
    }

    /// `output` as the run stores it (`workflow::redact_path`).
    fn stored(&self, output: &Value) -> Value {
        let mut m = Map::new();
        m.insert(self.name.clone(), output.clone());
        for p in &self.redact {
            let p = p.trim_start_matches("steps.");
            crate::workflow::redact_path(&mut m, p);
            if !self.per_item || p.starts_with(&format!("{}.", self.name)) { continue; }
            // run per item: whose map the step is in is not known here, so every map's paths count (`<map>.items.<field>`,
            // `<map>.items`, and a whole step's)
            match p.split_once(".items.") {
                Some((_, field)) => crate::workflow::redact_path(&mut m, &format!("{}.{field}", self.name)),
                None if p.ends_with(".items") || !p.contains('.') => crate::workflow::redact_path(&mut m, &self.name),
                None => {}
            }
        }
        m.remove(&self.name).unwrap_or(Value::Null)
    }
}

fn redact_by_value(v: &Value, secrets: &[String]) -> Value {
    if secrets.is_empty() { return v.clone(); }
    match v { Value::String(s) => { let mut t = s.clone(); for sec in secrets { if t.contains(sec.as_str()) { t = t.replace(sec.as_str(), "<redacted>"); } } Value::String(t) } Value::Array(a) => Value::Array(a.iter().map(|x| redact_by_value(x, secrets)).collect()), Value::Object(m) => Value::Object(m.iter().map(|(k, x)| (k.clone(), redact_by_value(x, secrets))).collect()), o => o.clone() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn att(name: &str, ct: &str, bytes: &[u8]) -> Attachment { Attachment { name: name.into(), content_type: ct.into(), bytes: bytes.to_vec() } }

    #[test]
    fn a_steps_files_reach_the_model_as_documents_images_and_text() {
        let files = vec![att("invoice.pdf", "application/pdf", b"%PDF-1"), att("scan.png", "image/png", b"\x89PNG"), att("rows.csv", "text/csv", b"\xEF\xBB\xBFa,b\n1,2")];
        let o = openai_content("Read them", &files);
        assert_eq!(o[0], json!({ "type": "file", "file": { "filename": "invoice.pdf", "file_data": "data:application/pdf;base64,JVBERi0x" } }));
        assert_eq!(o[1]["image_url"]["url"], "data:image/png;base64,iVBORw==");
        assert_eq!(o[2], json!({ "type": "text", "text": "<file name=\"rows.csv\" type=\"text/csv\">\na,b\n1,2\n</file>" }));
        assert_eq!(o[3], json!({ "type": "text", "text": "Read them" }));
        let a = anthropic_content("Read them", &files);
        assert_eq!(a[0], json!({ "type": "document", "source": { "type": "base64", "media_type": "application/pdf", "data": "JVBERi0x" }, "title": "invoice.pdf" }));
        assert_eq!(a[1], json!({ "type": "image", "source": { "type": "base64", "media_type": "image/png", "data": "iVBORw==" } }));
        assert_eq!(a[3]["text"], "Read them");
        // no files: the content is the prompt, as before
        assert_eq!(openai_content("p", &[]), json!("p"));
        assert_eq!(anthropic_content("p", &[]), json!("p"));
        assert_eq!((files[0].tokens(), files[1].tokens(), files[2].tokens()), (1_500, 1_600, 2));
    }

    #[test]
    fn a_file_no_model_reads_fails_the_step_before_any_call() {
        let p = json!({ "files": [{ "id": "file_1", "name": "a.pdf", "content_type": "application/pdf" }, { "id": "file_2", "name": "all.zip", "content_type": "application/zip" }] });
        let e = file_refs("extract", &p).unwrap_err().to_string();
        assert!(e.starts_with("InvalidInputError: step `extract`") && e.contains("`all.zip` is application/zip"), "{e}");
        let e = file_refs("extract", &json!({ "files": [{ "id": "../../etc/passwd" }] })).unwrap_err().to_string();
        assert!(e.contains("is not a file"), "{e}");
        assert_eq!(file_refs("extract", &json!({ "prompt": "x" })).unwrap().len(), 0);
        assert_eq!(model_input("text/markdown"), Some(ModelInput::Text));
        assert_eq!(model_input("application/vnd.openxmlformats-officedocument.wordprocessingml.document"), None);
    }

    #[tokio::test]
    async fn a_local_run_reads_its_kept_files_and_caps_what_one_call_sends() {
        let app = tempfile_dir();
        let wf = app.join("workflows").join("w");
        std::fs::create_dir_all(&wf).unwrap();
        let dir = local_files_dir(&wf).unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("file_a"), b"%PDF-1.7").unwrap();
        std::fs::write(dir.join("file_a.json"), r#"{"id":"file_a","name":"a.pdf","size":8,"content_type":"application/pdf","sha256":""}"#).unwrap();
        let mut i: StepInput = serde_json::from_value(json!({ "run": { "id": "r" }, "user": {}, "params": {}, "root": wf.display().to_string(), "step": { "name": "s", "kind": "llm", "file": "s.ts", "literals": {} }, "steps": {}, "llm": serde_json::to_value(ResolvedLlm::default()).unwrap() })).unwrap();
        let refs = vec![FileRef { id: "file_a".into(), name: "a.pdf".into(), content_type: "application/pdf".into() }];
        let got = load_files(&i, "s", &refs).await.unwrap();
        assert_eq!((got[0].name.as_str(), got[0].content_type.as_str(), got[0].bytes.as_slice()), ("a.pdf", "application/pdf", b"%PDF-1.7".as_slice()));
        let missing = vec![FileRef { id: "file_b".into(), name: "b.pdf".into(), content_type: "application/pdf".into() }];
        assert!(load_files(&i, "s", &missing).await.unwrap_err().to_string().contains("no local file `b.pdf`"));
        // past the cap: refused, with the way out
        std::fs::write(dir.join("file_big"), vec![b'a'; (LLM_FILES_MAX + 1) as usize]).unwrap();
        std::fs::write(dir.join("file_big.json"), r#"{"id":"file_big","name":"big.txt","size":1,"content_type":"text/plain","sha256":""}"#).unwrap();
        let big = vec![FileRef { id: "file_big".into(), name: "big.txt".into(), content_type: "text/plain".into() }];
        assert!(load_files(&i, "s", &big).await.unwrap_err().to_string().contains("a `map` over the files"));
        i.root = "/".into();
        assert!(load_files(&i, "s", &refs).await.is_err());
        let _ = std::fs::remove_dir_all(&app);
    }

    fn tempfile_dir() -> std::path::PathBuf { let d = std::env::temp_dir().join(format!("cori-files-{}", uuid::Uuid::new_v4().simple())); std::fs::create_dir_all(&d).unwrap(); d }

    #[test]
    fn names_the_server_sends_decode() {
        assert_eq!(urlencoding_decode("facture%20%C3%A9t%C3%A9.pdf").as_deref(), Some("facture été.pdf"));
        assert_eq!(urlencoding_decode("a%2").as_deref(), Some("a%2"));
        assert_eq!(urlencoding_decode("%FF"), None);
    }

    #[test]
    fn http_paths_stay_on_the_connectors_origin() {
        assert_eq!(connector_url("https://slack.com", "/api/chat.postMessage").unwrap().as_str(), "https://slack.com/api/chat.postMessage");
        assert_eq!(connector_url("https://api.example/v2/", "/users?x=1").unwrap().as_str(), "https://api.example/v2/users?x=1", "the base path is kept");
        for evil in ["@evil.example/x", ".evil.example/x", "//evil.example/x", "x", ""] { assert!(connector_url("https://slack.com", evil).is_err(), "{evil}"); }
        assert_eq!(connector_url("https://slack.com", "/@evil.example").unwrap().host_str(), Some("slack.com"));
        assert_eq!(connector_url("https://slack.com", "/\\evil.example").unwrap().host_str(), Some("slack.com"));
    }

    #[test]
    fn an_org_providers_key_never_goes_where_the_app_says() {
        let org = crate::config::ProviderCfg { secret: Some("llm.openai".into()), ..Default::default() };
        assert_eq!(llm_base("openai", Some(&org), Some("https://attacker.example/v1"), true), "https://api.openai.com/v1");
        let own = crate::config::ProviderCfg { base_url: Some("https://vllm.internal/v1".into()), secret: Some("llm.vllm".into()), ..Default::default() };
        assert_eq!(llm_base("openai", Some(&own), Some("https://attacker.example/v1"), true), "https://vllm.internal/v1");
        assert_eq!(llm_base("openai", None, Some("https://my-gateway.example/v1"), false), "https://my-gateway.example/v1", "the app's own provider goes where it says");
        assert_eq!(llm_base("anthropic", Some(&org), Some("https://attacker.example/v1"), true), "https://api.anthropic.com/v1");
    }

    /// A refused call's error says what the provider said: OpenRouter's upstream in its own words and by name, OpenAI's and
    /// Anthropic's `error.message`, an upstream's JSON answer by its message, any other body as one bounded line; never the key.
    #[test]
    fn a_refused_call_says_why() {
        let said = |status: u16, text: &str| refused(status, &provider_json(status, text).unwrap(), text, "sk-or-v1-secret");
        assert_eq!(said(429, r#"{"error":{"message":"Provider returned error","code":429,"metadata":{"raw":"google/gemma-4-26b-a4b-it:free is temporarily rate-limited upstream.","provider_name":"Google AI Studio"}},"user_id":"org_x"}"#),
            "provider returned 429: google/gemma-4-26b-a4b-it:free is temporarily rate-limited upstream. (Google AI Studio)");
        assert_eq!(said(429, r#"{"error":{"message":"Provider returned error","metadata":{"raw":"{\n  \"error\": {\n    \"code\": 429,\n    \"message\": \"Resource has been exhausted.\"\n  }\n}","provider_name":"Google"}}}"#),
            "provider returned 429: Resource has been exhausted. (Google)");
        assert_eq!(said(429, r#"{"error":{"message":"Provider returned error","metadata":{"raw":" ","provider_name":"Google"}}}"#), "provider returned 429: Provider returned error (Google)");
        assert_eq!(said(429, r#"{"error":{"message":"","metadata":{"provider_name":"Google"}}}"#), "provider returned 429: {\"error\":{\"message\":\"\",\"metadata\":{\"provider_name\":\"Google\"}}} (Google)");
        assert_eq!(said(429, r#"{"error":{"message":"Rate limit reached for gpt-x on requests per min.","type":"requests","code":"rate_limit_exceeded"}}"#), "provider returned 429: Rate limit reached for gpt-x on requests per min.");
        assert_eq!(said(529, r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#), "provider returned 529: Overloaded");
        assert_eq!(said(400, r#"{"object":"error","message":"The model `x` does not exist.","code":400}"#), "provider returned 400: The model `x` does not exist.");
        assert_eq!(said(401, r#"{"error":{"message":"Incorrect API key provided: sk-or-v1-secret."}}"#), "provider returned 401: Incorrect API key provided: <redacted>.");
        assert_eq!(said(502, "<html>\n  <body>Bad   gateway</body>\n</html>"), "provider returned 502: <html> <body>Bad gateway</body> </html>");
        assert_eq!(said(503, ""), "provider returned 503");
        let long = said(500, &"x".repeat(1000));
        assert!(long.ends_with('…') && long.chars().count() == "provider returned 500: ".len() + 301, "{long}");
        // an answer that is no JSON is a broken exchange, retried
        assert!(provider_json(200, "<html>").unwrap_err().to_string().starts_with("Transient: "));
    }

    #[test]
    fn coris_own_settings_are_never_a_credential() {
        unsafe { std::env::set_var("CORI_TEST_SECRET_X", "s"); std::env::set_var("TEST_CONNECTOR_TOKEN_X", "t"); }
        assert_eq!(env_credential("CORI_TEST_SECRET_X"), None);
        assert_eq!(env_credential("TEST_CONNECTOR_TOKEN_X").as_deref(), Some("t"));
    }

    fn data(name: &str, redact: &[&str], steps: Value, item: Option<Value>) -> StepData {
        let i: StepInput = serde_json::from_value(json!({ "run": {}, "user": {}, "params": {}, "root": ".", "redact": redact, "step": { "name": name, "kind": "code", "file": "x.ts", "literals": {} },
            "steps": steps, "item": item, "llm": { "provider": "", "base_url": null, "api_key_env": null, "tiers": {}, "mock": null } })).unwrap();
        StepData::of(&i)
    }

    /// A step's event shows its data as the run keeps it: the `redact:` values blanked in its input as in its LLM capture,
    /// its own `redact:` paths in its output as in the stored outputs, what is too large left out.
    #[test]
    fn a_step_event_shows_its_data_as_the_run_keeps_it() {
        let d = data("notify", &["steps.lookup.ssn", "steps.notify.token"], json!({ "lookup": { "ssn": "123-45-6789", "name": "Ada" } }), None);
        let mut ev = json!({ "name": "notify", "status": "ok" });
        d.add(&mut ev, Some(&json!({ "text": "Ada, 123-45-6789", "n": 1 })), Some(&json!({ "token": "tk_live", "sent": true })));
        assert_eq!(ev["input"], json!({ "text": "Ada, <redacted>", "n": 1 }));
        assert_eq!(ev["output"], json!({ "token": "<redacted>", "sent": true }));
        assert!(ev.get("too_large").is_none());

        // a failed attempt: its input only
        let mut ev = json!({ "status": "failed" });
        d.add(&mut ev, Some(&json!({ "n": 2 })), None);
        assert_eq!((ev["input"].clone(), ev.get("output").is_none()), (json!({ "n": 2 }), true));

        // too large: its size instead
        let big = json!({ "rows": "x".repeat(STEP_DATA_MAX) });
        let mut ev = json!({});
        d.add(&mut ev, Some(&json!({ "n": 3 })), Some(&big));
        assert!(ev.get("output").is_none() && ev["too_large"]["output"].as_u64().unwrap() > STEP_DATA_MAX as u64, "{ev}");
        assert_eq!(ev["input"], json!({ "n": 3 }));
    }

    /// Run per item of a map, a step's output is one of the map's `items`: the map's `redact:` paths blank it too.
    #[test]
    fn an_item_of_a_map_keeps_its_maps_redactions() {
        let d = data("check_one", &["steps.fan.items.email", "steps.other.secret"], json!({}), Some(json!({ "id": 1 })));
        let mut ev = json!({});
        d.add(&mut ev, None, Some(&json!({ "email": "a@b.example", "secret": "kept: another step's field", "ok": true })));
        assert_eq!(ev["output"], json!({ "email": "<redacted>", "secret": "kept: another step's field", "ok": true }));
        let whole = data("check_one", &["steps.fan.items"], json!({}), Some(json!({ "id": 1 })));
        let mut ev = json!({});
        whole.add(&mut ev, None, Some(&json!({ "email": "a@b.example" })));
        assert_eq!(ev["output"], json!("<redacted>"));
        // not run per item: another step's paths do not touch it
        let top = data("check_one", &["steps.fan.items.email"], json!({}), None);
        let mut ev = json!({});
        top.add(&mut ev, None, Some(&json!({ "email": "a@b.example" })));
        assert_eq!(ev["output"], json!({ "email": "a@b.example" }));
    }
}
