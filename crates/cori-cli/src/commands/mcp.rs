//! `cori mcp` — serve Cori over the Model Context Protocol (stdio).
//!
//! Tools are a strict subset of the CLI verbs (`check`, `run`, `show`,
//! `runs_list`, `runs_show`, `status`) — same arguments, same underlying
//! code paths in `cori-run`. There are deliberately **no** `login`,
//! `work`, `config`, or `save_workflow` tools: machine-trust operations
//! stay human-initiated and credentials never transit an MCP client.
//!
//! Consent model (two layers, decided in the Wave 2 sign-off):
//! 1. `CORI_ASSUME_YES` is removed from this process's environment at
//!    startup — an MCP server inherits its launch environment, and
//!    honoring the variable would silently auto-approve agent runs.
//! 2. Every `run` requires a per-run human confirmation — even for
//!    already-trusted refs and local paths — so an agent mid-conversation
//!    never triggers side effects without a human in the loop. The confirm
//!    is an MCP elicitation when the client declares that capability, and
//!    otherwise a **native OS dialog on this machine** (possible because
//!    MCP servers run on the host). Only when neither channel exists
//!    (headless, or `CORI_MCP_DISABLE_NATIVE_CONFIRM=1`) is `run` refused,
//!    directing the human to the CLI or the Cori desktop app. First-run
//!    trust consent for remote refs goes through the same channels with a
//!    richer message.
//!
//! Transport: newline-delimited JSON-RPC over stdio — the same wire
//! conventions as the broker's MCP client (`cori-broker/src/mcp.rs`).
//! Tool dispatch is designed so a second tool source (the CBC-derived
//! capability facade, `capability_contract.md` §5) can be added without
//! restructuring: `tool_definitions()` and `dispatch_tool()` are the
//! only two places a tool is named.

use std::collections::HashMap;
use std::io::{BufRead, Write as _};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value as JsonValue, json};

use cori_broker::identity::{IdentitySource, OsUser};
use cori_protocol::{StepKind, WorkerIdentity, task_queue_for};

const PROTOCOL_VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];
const LATEST_PROTOCOL: &str = "2025-06-18";
/// How long an elicitation waits for the human before being treated as
/// declined. Generous: the user may be reading a diff.
const ELICIT_TIMEOUT: Duration = Duration::from_secs(300);

// Embedded copy of the `cori-save-workflow` skill, served as MCP
// resources/prompts so any client receives the capture procedure without
// a separate skill install. Version-skew governance: the copy is stamped
// with this binary's version in every resource description; the npx-installed
// skill may be newer — the stamp makes the provenance auditable.
const SKILL_MD: &str = include_str!("../../../../skills/cori-save-workflow/SKILL.md");
const REF_ACTIVITY_KINDS: &str =
    include_str!("../../../../skills/cori-save-workflow/references/activity_kinds.md");
const REF_EXAMPLE_WORKFLOW: &str =
    include_str!("../../../../skills/cori-save-workflow/references/example_workflow.md");
const REF_MANIFEST_SCHEMA: &str =
    include_str!("../../../../skills/cori-save-workflow/references/manifest_schema.md");
const REF_TRACE_INTERPRETATION: &str =
    include_str!("../../../../skills/cori-save-workflow/references/trace_interpretation.md");

// ---------------------------------------------------------------------------
// Shared server state
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct Shared {
    out: Arc<Mutex<std::io::Stdout>>,
    /// Server→client requests awaiting a response (elicitation).
    pending: Arc<Mutex<HashMap<u64, mpsc::Sender<JsonValue>>>>,
    next_req_id: Arc<AtomicU64>,
    /// Request ids the client cancelled — suppress their responses.
    cancelled: Arc<Mutex<Vec<JsonValue>>>,
    /// Did the client declare the `elicitation` capability at initialize?
    client_can_elicit: Arc<Mutex<bool>>,
    /// In-flight tool-call threads, joined at stdin EOF so a client that
    /// closes stdin right after its last request still gets every response.
    workers: Arc<Mutex<Vec<std::thread::JoinHandle<()>>>>,
}

impl Shared {
    fn send(&self, msg: &JsonValue) {
        let mut out = self.out.lock().expect("stdout lock");
        // A write error means the client is gone; nothing useful to do.
        let _ = writeln!(out, "{msg}");
        let _ = out.flush();
    }

    fn reply(&self, id: &JsonValue, result: JsonValue) {
        self.send(&json!({ "jsonrpc": "2.0", "id": id, "result": result }));
    }

    fn reply_error(&self, id: &JsonValue, code: i64, message: &str) {
        self.send(&json!({
            "jsonrpc": "2.0", "id": id,
            "error": { "code": code, "message": message }
        }));
    }

    fn is_cancelled(&self, id: &JsonValue) -> bool {
        self.cancelled.lock().expect("cancelled lock").contains(id)
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

pub fn mcp() -> Result<()> {
    // Consent rule 1: never honor an inherited auto-approve. Removing the
    // variable (rather than branching on it at each call site) enforces the
    // stance across every cori-run code path this process touches.
    // SAFETY: called before any thread is spawned.
    unsafe { std::env::remove_var("CORI_ASSUME_YES") };

    let shared = Shared {
        out: Arc::new(Mutex::new(std::io::stdout())),
        pending: Arc::new(Mutex::new(HashMap::new())),
        next_req_id: Arc::new(AtomicU64::new(1)),
        cancelled: Arc::new(Mutex::new(Vec::new())),
        client_can_elicit: Arc::new(Mutex::new(false)),
        workers: Arc::new(Mutex::new(Vec::new())),
    };

    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        let line = line.context("reading stdin")?;
        if line.trim().is_empty() {
            continue;
        }
        let Ok(msg) = serde_json::from_str::<JsonValue>(&line) else {
            tracing::warn!("ignoring non-JSON line on stdin");
            continue;
        };

        match (msg.get("method").and_then(|m| m.as_str()), msg.get("id")) {
            // Request from the client.
            (Some(method), Some(id)) => {
                let id = id.clone();
                let method = method.to_string();
                let params = msg.get("params").cloned().unwrap_or(JsonValue::Null);
                handle_request(&shared, id, &method, params);
            }
            // Notification from the client. `notifications/initialized`
            // and anything unrecognized are deliberate no-ops.
            (Some(method), None) => {
                if method == "notifications/cancelled"
                    && let Some(rid) = msg.pointer("/params/requestId")
                {
                    shared
                        .cancelled
                        .lock()
                        .expect("cancelled lock")
                        .push(rid.clone());
                }
            }
            // Response to a server→client request (elicitation).
            (None, Some(id)) => {
                if let Some(rid) = id.as_u64() {
                    let sender = shared.pending.lock().expect("pending lock").remove(&rid);
                    if let Some(tx) = sender {
                        let _ = tx.send(msg);
                    }
                }
            }
            _ => {}
        }
    }

    // stdin closed: drain in-flight tool calls before exiting.
    let handles = std::mem::take(&mut *shared.workers.lock().expect("workers lock"));
    for h in handles {
        let _ = h.join();
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Request dispatch
// ---------------------------------------------------------------------------

fn handle_request(shared: &Shared, id: JsonValue, method: &str, params: JsonValue) {
    match method {
        "initialize" => {
            let client_version = params
                .get("protocolVersion")
                .and_then(|v| v.as_str())
                .unwrap_or(LATEST_PROTOCOL);
            let negotiated = if PROTOCOL_VERSIONS.contains(&client_version) {
                client_version
            } else {
                LATEST_PROTOCOL
            };
            let can_elicit = params.pointer("/capabilities/elicitation").is_some();
            *shared.client_can_elicit.lock().expect("elicit lock") = can_elicit;
            shared.reply(
                &id,
                json!({
                    "protocolVersion": negotiated,
                    "capabilities": {
                        "tools": { "listChanged": false },
                        "resources": { "subscribe": false, "listChanged": false },
                        "prompts": { "listChanged": false }
                    },
                    "serverInfo": {
                        "name": "cori",
                        "title": "Cori",
                        "version": env!("CARGO_PKG_VERSION")
                    },
                    "instructions": SERVER_INSTRUCTIONS
                }),
            );
        }
        "ping" => shared.reply(&id, json!({})),
        "tools/list" => shared.reply(&id, json!({ "tools": tool_definitions() })),
        "tools/call" => {
            // Tool calls can block for minutes (`run`) and may need to
            // exchange elicitation messages mid-call, so every call runs on
            // its own thread; the read loop stays free to route responses.
            let worker_shared = shared.clone();
            let handle = std::thread::spawn(move || {
                let shared = worker_shared;
                let name = params
                    .get("name")
                    .and_then(|n| n.as_str())
                    .unwrap_or_default()
                    .to_string();
                let args = params.get("arguments").cloned().unwrap_or(json!({}));
                let progress_token = params.pointer("/_meta/progressToken").cloned();
                let outcome = dispatch_tool(&shared, &name, &args, progress_token.as_ref());
                if shared.is_cancelled(&id) {
                    return; // client cancelled: send no response
                }
                match outcome {
                    Ok((payload, is_error)) => shared.reply(
                        &id,
                        json!({
                            "content": [{
                                "type": "text",
                                "text": serde_json::to_string_pretty(&payload)
                                    .unwrap_or_else(|_| payload.to_string()),
                            }],
                            "structuredContent": payload,
                            "isError": is_error
                        }),
                    ),
                    Err(e) => shared.reply(
                        &id,
                        json!({
                            "content": [{ "type": "text", "text": format!("{e:#}") }],
                            "isError": true
                        }),
                    ),
                }
            });
            shared.workers.lock().expect("workers lock").push(handle);
        }
        "resources/list" => shared.reply(&id, json!({ "resources": resource_list() })),
        "resources/read" => {
            let uri = params.get("uri").and_then(|u| u.as_str()).unwrap_or("");
            match resource_body(uri) {
                Some(text) => shared.reply(
                    &id,
                    json!({
                        "contents": [{
                            "uri": uri,
                            "mimeType": "text/markdown",
                            "text": text
                        }]
                    }),
                ),
                None => shared.reply_error(&id, -32002, &format!("unknown resource: {uri}")),
            }
        }
        "prompts/list" => shared.reply(
            &id,
            json!({
                "prompts": [{
                    "name": "cori-save-workflow",
                    "title": "Save this conversation as a Cori workflow",
                    "description": skill_stamp("The full capture procedure"),
                }]
            }),
        ),
        "prompts/get" => {
            let name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
            if name == "cori-save-workflow" {
                shared.reply(
                    &id,
                    json!({
                        "description": skill_stamp("The full capture procedure"),
                        "messages": [{
                            "role": "user",
                            "content": { "type": "text", "text": SKILL_MD }
                        }]
                    }),
                );
            } else {
                shared.reply_error(&id, -32602, &format!("unknown prompt: {name}"));
            }
        }
        other => shared.reply_error(&id, -32601, &format!("method not found: {other}")),
    }
}

const SERVER_INSTRUCTIONS: &str = "Cori turns agent conversations into deterministic, \
re-runnable workflows (a workflow is a reviewed folder on disk; `cori run` executes it \
on Temporal with no LLM in the loop unless a step declares one). Tools mirror the CLI \
verbs: check (preflight readiness), run (execute — always asks the human to confirm, \
via elicitation or a native dialog on the machine; there is no auto-approve), show \
(inspect a workflow), runs_list / runs_show (run history), status (machine overview). There are intentionally no login/work/config \
tools over MCP. The `cori-save-workflow` prompt and the cori://skill/* resources contain \
the full procedure for capturing the current conversation as a workflow folder.";

// ---------------------------------------------------------------------------
// Tools
// ---------------------------------------------------------------------------

fn tool_definitions() -> JsonValue {
    json!([
        {
            "name": "check",
            "title": "Preflight a workflow",
            "description": "Per-step readiness and capability auth status for a workflow \
                folder or remote git ref, without running it. Mirrors `cori check`.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "source": { "type": "string", "description": "Workflow folder path or remote ref (host/owner/repo[/subpath][@ref])" },
                    "update": { "type": "boolean", "description": "For remote refs: re-resolve the ref first", "default": false }
                },
                "required": ["source"]
            }
        },
        {
            "name": "run",
            "title": "Run a workflow",
            "description": "Execute a workflow end-to-end and return the run trace. \
                Always requires a per-run human confirmation: an MCP elicitation when \
                the client supports it, otherwise a native dialog on this machine. \
                Refused only when no confirmation channel exists. First-run consent \
                for untrusted remote refs uses the same channels. Mirrors `cori run`.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "source": { "type": "string", "description": "Workflow folder path or remote ref" },
                    "params": { "type": "object", "description": "Workflow parameters as key → value", "additionalProperties": true },
                    "dry_run": { "type": "boolean", "description": "Validate the plan without external calls", "default": false },
                    "update": { "type": "boolean", "description": "For remote refs: re-resolve the ref first", "default": false }
                },
                "required": ["source"]
            }
        },
        {
            "name": "show",
            "title": "Inspect a workflow",
            "description": "Manifest, steps, and required capabilities of a workflow \
                folder or remote ref. Mirrors `cori show`.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "source": { "type": "string", "description": "Workflow folder path or remote ref" }
                },
                "required": ["source"]
            }
        },
        {
            "name": "runs_list",
            "title": "List recent runs",
            "description": "Recent run history, most recent first. Mirrors `cori runs list`.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "workflow_id": { "type": "string", "description": "Restrict to one workflow id" },
                    "limit": { "type": "integer", "description": "Maximum rows (default 20)", "default": 20, "minimum": 1 }
                }
            }
        },
        {
            "name": "runs_show",
            "title": "Show one run's trace",
            "description": "Persisted trace of one run (per-step status, attempts, \
                duration, cost). Bulky per-activity outputs are elided by default \
                (summaries stay) — fetch one step's full output with `activity`, or \
                everything with `full`. Mirrors `cori runs show`.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "run_id": { "type": "string" },
                    "activity": { "type": "string", "description": "Return only this activity, with its full output (step name or activity id)" },
                    "full": { "type": "boolean", "description": "Include every activity's full output inline", "default": false }
                },
                "required": ["run_id"]
            }
        },
        {
            "name": "status",
            "title": "Machine status",
            "description": "Temporal endpoint reachability, identity, discovered \
                capabilities with auth state, and workers seen on the cluster. \
                Mirrors `cori status`.",
            "inputSchema": { "type": "object", "properties": {} }
        }
    ])
}

/// Single dispatch point — a future CBC-derived tool source plugs in here
/// and in `tool_definitions()`, nowhere else.
fn dispatch_tool(
    shared: &Shared,
    name: &str,
    args: &JsonValue,
    progress_token: Option<&JsonValue>,
) -> Result<(JsonValue, bool)> {
    match name {
        "check" => tool_check(args).map(|v| (v.clone(), v["ready"] != json!(true))),
        "run" => tool_run(shared, args, progress_token),
        "show" => tool_show(args).map(|v| (v, false)),
        "runs_list" => tool_runs_list(args).map(|v| (v, false)),
        "runs_show" => tool_runs_show(args).map(|v| (v, false)),
        "status" => tool_status().map(|v| (v, false)),
        other => bail!("unknown tool: {other}"),
    }
}

fn arg_str(args: &JsonValue, key: &str) -> Result<String> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| anyhow!("missing required argument `{key}`"))
}

fn arg_bool(args: &JsonValue, key: &str) -> bool {
    args.get(key).and_then(|v| v.as_bool()).unwrap_or(false)
}

fn tool_check(args: &JsonValue) -> Result<JsonValue> {
    let source = arg_str(args, "source")?;
    let update = arg_bool(args, "update");

    // Consent probe first: never TTY-prompt, never auto-consent.
    let pf = cori_run::preflight(&source, update, false)?;
    if let Some(consent) = &pf.consent_required {
        return Ok(json!({
            "ready": false,
            "consent_required": {
                "remote_ref": consent.spec.display(),
                "sha": consent.sha,
            },
            "note": "This remote workflow has not been trusted on this machine yet. \
                Run it via the `run` tool (which asks for consent), or review and \
                consent in the Cori desktop app / `cori run` in a terminal."
        }));
    }

    // Trusted or local: the full per-step preflight (same code as `cori check`).
    let report = super::check::preflight(&source, update, false)?;
    Ok(json!({
        "ready": report.ready,
        "endpoint": report.endpoint,
        "temporal_reachable": report.temporal_reachable,
        "user_task_queue": report.user_task_queue,
        "steps": report.steps.iter().map(|s| json!({
            "step_name": s.step_name,
            "kind": kind_label(s.kind),
            "task_queue": s.task_queue,
            "missing": s.missing,
        })).collect::<Vec<_>>(),
        "capabilities": report.capabilities.iter().map(|c| json!({
            "id": c.id,
            "kind": format!("{:?}", c.kind),
            "authed": c.authed,
            "detail": c.detail,
            "remedy": c.remedy,
        })).collect::<Vec<_>>(),
    }))
}

fn tool_show(args: &JsonValue) -> Result<JsonValue> {
    let source = arg_str(args, "source")?;
    let pf = cori_run::preflight(&source, false, false)?;
    let compiled = &pf.loaded.compiled;
    Ok(json!({
        "source": source,
        "path": pf.loaded.absolute_path.display().to_string(),
        "manifest": compiled.manifest,
        "steps": compiled.steps.iter().map(|s| json!({
            "activity_id": s.activity_id,
            "name": s.name,
            "kind": kind_label(s.kind),
        })).collect::<Vec<_>>(),
        "required": {
            "cli": compiled.required_cli_binaries,
            "mcp_servers": compiled.required_mcp_servers,
            "llm_providers": compiled.required_llm_providers,
        },
        "missing_capabilities": pf.missing_caps,
        "consent_required": pf.consent_required.as_ref().map(|c| json!({
            "remote_ref": c.spec.display(),
            "sha": c.sha,
        })),
    }))
}

fn tool_runs_list(args: &JsonValue) -> Result<JsonValue> {
    let filter = args.get("workflow_id").and_then(|v| v.as_str());
    let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(20) as usize;
    let mut entries = super::runs::collect_runs(filter)?;
    entries.truncate(limit);
    Ok(json!(
        entries
            .iter()
            .map(|e| {
                let t = &e.trace;
                json!({
                    "run_id": t.run_id,
                    "workflow_id": t.workflow_id,
                    "status": t.status,
                    "trigger": t.trigger,
                    "started_at": t.started_at,
                    "duration_ms": t.duration_ms,
                    "cost_eur": t.cost.total_eur,
                    "error": t.error,
                })
            })
            .collect::<Vec<_>>()
    ))
}

fn tool_runs_show(args: &JsonValue) -> Result<JsonValue> {
    let run_id = arg_str(args, "run_id")?;
    let activity = args.get("activity").and_then(|v| v.as_str());
    let full = arg_bool(args, "full");
    let entries = super::runs::collect_runs(None)?;
    let entry = entries
        .into_iter()
        .find(|e| e.trace.run_id == run_id)
        .ok_or_else(|| anyhow!("no run found with id `{run_id}`"))?;
    match activity {
        Some(name) => {
            let act = entry
                .trace
                .activities
                .iter()
                .find(|a| a.step_name == name || a.activity_id == name)
                .ok_or_else(|| anyhow!("no activity matching `{name}` in run `{run_id}`"))?;
            Ok(serde_json::to_value(act)?)
        }
        None => {
            let mut trace = serde_json::to_value(&entry.trace)?;
            if !full {
                trim_trace_outputs(&mut trace);
            }
            Ok(trace)
        }
    }
}

/// Max bytes of a single activity's `output` inlined into an MCP tool
/// result. Real workflows shuttle whole row sets between steps; a
/// two-step sheet workflow already produced a ~195 KB trace in the
/// field, blowing tool-result limits. The `*_summary` fields carry the
/// signal; bulk output stays on disk and is fetchable per activity.
const MAX_INLINE_OUTPUT: usize = 2048;

fn trim_trace_outputs(trace: &mut JsonValue) {
    let run_id = trace
        .get("run_id")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let Some(acts) = trace.get_mut("activities").and_then(|a| a.as_array_mut()) else {
        return;
    };
    for a in acts {
        let bytes = a.get("output").map(|o| o.to_string().len()).unwrap_or(0);
        if bytes > MAX_INLINE_OUTPUT {
            let name = a
                .get("step_name")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            a["output"] = json!({
                "_elided": true,
                "bytes": bytes,
                "note": format!(
                    "full output elided from the MCP response (output_summary is intact); \
                     fetch it with runs_show {{\"run_id\": \"{run_id}\", \"activity\": \"{name}\"}}"
                ),
            });
        }
    }
}

fn tool_status() -> Result<JsonValue> {
    use cori_broker::capabilities::{self, CapabilityReport};
    use cori_run::{paths, planner, temporal_endpoint};

    let endpoint = temporal_endpoint::resolve()?;
    let reachable =
        cori_worker::runtime::preflight_check(&endpoint.target, Duration::from_millis(500)).is_ok();
    let identity = OsUser.resolve().context("resolving OS user identity")?;
    let queue = task_queue_for(&identity);
    let credentials = cori_run::resolve_llm_credentials();
    let home = paths::home()?;
    let caps = capabilities::discover(&home, &[], &credentials);
    let report = CapabilityReport::from_capabilities_with(
        identity.clone(),
        &caps,
        Some(&paths::credentials_dir()?),
    );
    let cluster = planner::ClusterView::load().unwrap_or_default();

    Ok(json!({
        "endpoint": endpoint.target,
        "temporal_reachable": reachable,
        "identity": match &identity {
            WorkerIdentity::Person { user_id } => json!({ "kind": "person", "user_id": user_id }),
            WorkerIdentity::Service { pool } => json!({ "kind": "service", "pool": pool }),
        },
        "user_task_queue": queue,
        "capabilities": report.capabilities.iter().map(|c| json!({
            "id": c.id,
            "kind": format!("{:?}", c.kind),
            "authed": c.authed,
            "detail": c.detail,
        })).collect::<Vec<_>>(),
        "workers_seen": cluster.reports.iter().map(|r| json!({
            "task_queue": r.task_queue,
            "kind": match &r.identity {
                WorkerIdentity::Person { .. } => "user",
                WorkerIdentity::Service { .. } => "shared",
            },
        })).collect::<Vec<_>>(),
    }))
}

// ---------------------------------------------------------------------------
// The run tool — per-run confirm + trust consent via elicitation
// ---------------------------------------------------------------------------

fn tool_run(
    shared: &Shared,
    args: &JsonValue,
    progress_token: Option<&JsonValue>,
) -> Result<(JsonValue, bool)> {
    let source = arg_str(args, "source")?;
    let dry_run = arg_bool(args, "dry_run");
    let update = arg_bool(args, "update");
    let params_obj = args.get("params").and_then(|v| v.as_object());

    // `key=value` strings, exactly what the CLI passes to build_initial_input.
    let param_strings: Vec<String> = params_obj
        .map(|m| {
            m.iter()
                .map(|(k, v)| match v {
                    JsonValue::String(s) => format!("{k}={s}"),
                    other => format!("{k}={other}"),
                })
                .collect()
        })
        .unwrap_or_default();

    // Load + compile first so the confirmation names what will actually run.
    let pf = cori_run::preflight(&source, update, false)?;
    let workflow_name = pf.loaded.compiled.manifest.name.clone();
    let initial_params = cori_run::build_initial_input(&pf.loaded.compiled, &param_strings)?;

    // Consent rule 2: per-run confirm, unconditionally (local & trusted included).
    let mode = if dry_run { " (dry run)" } else { "" };
    let confirm_msg = format!(
        "Run Cori workflow \"{workflow_name}\"{mode} from `{source}` with params {params}?",
        params = serde_json::to_string(&initial_params).unwrap_or_else(|_| "{}".into()),
    );
    let confirm_payload = json!({
        "source": source,
        "workflow_id": pf.loaded.compiled.manifest.id,
        "workflow_name": workflow_name,
        "params": initial_params,
        "dry_run": dry_run,
        "steps": pf.loaded.compiled.steps.len(),
    });
    match confirm_with_human(
        shared,
        &confirm_msg,
        cori_run::approvals::ApprovalKind::RunConfirm,
        confirm_payload,
    )? {
        ElicitOutcome::Accepted => {}
        ElicitOutcome::Declined => {
            return Ok((
                json!({ "status": "not_run", "reason": "run declined by the user" }),
                true,
            ));
        }
        ElicitOutcome::NoAnswer => {
            return Ok((
                json!({ "status": "not_run",
                        "reason": "no human confirmation was obtained: the MCP client \
                            does not support elicitation and no native confirmation \
                            dialog could be shown (timed out, headless, or disabled). \
                            Run the workflow from the Cori desktop app or `cori run` \
                            in a terminal instead." }),
                true,
            ));
        }
    }

    // First-run trust consent for untrusted remote refs: a second, richer
    // elicitation wired through the same ConsentCallback seam the Console uses.
    let consent_shared = shared.clone();
    let consent = cori_run::ConsentCallback::Prompt(Box::new(move |prompt| {
        let caps = [
            prompt.compiled.required_cli_binaries.join(", "),
            prompt.compiled.required_mcp_servers.join(", "),
            prompt.compiled.required_llm_providers.join(", "),
        ]
        .iter()
        .filter(|s| !s.is_empty())
        .cloned()
        .collect::<Vec<_>>()
        .join("; ");
        let msg = format!(
            "First-run consent: remote workflow {spec} at commit {sha} has not been \
             trusted on this machine. It declares these capabilities: [{caps}]. \
             Trust this exact version and run it?",
            spec = prompt.spec.display(),
            sha = &prompt.sha[..12.min(prompt.sha.len())],
        );
        let payload = json!({
            "remote_ref": prompt.spec.display(),
            "sha": prompt.sha,
            "capabilities": {
                "cli": prompt.compiled.required_cli_binaries,
                "mcp_servers": prompt.compiled.required_mcp_servers,
                "llm_providers": prompt.compiled.required_llm_providers,
            },
        });
        match confirm_with_human(
            &consent_shared,
            &msg,
            cori_run::approvals::ApprovalKind::TrustConsent,
            payload,
        ) {
            Ok(ElicitOutcome::Accepted) => cori_run::ConsentDecision::Granted,
            Ok(ElicitOutcome::Declined) => cori_run::ConsentDecision::Denied,
            _ => cori_run::ConsentDecision::Defer,
        }
    }));

    let total_steps = pf.loaded.compiled.steps.len();
    let progress: Arc<dyn cori_run::ProgressSink> = match progress_token {
        Some(token) => Arc::new(McpProgressSink {
            shared: shared.clone(),
            token: token.clone(),
            total: total_steps,
            done: AtomicU64::new(0),
        }),
        None => Arc::new(cori_run::NoopSink),
    };

    let tokio_rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| anyhow!("starting tokio runtime: {e}"))?;
    let trace = tokio_rt.block_on(cori_run::run_workflow(
        cori_run::RunRequest {
            source,
            params: initial_params,
            dry_run,
            update,
            trigger: cori_run::Trigger::Mcp,
            run_id: None,
        },
        consent,
        progress,
    ))?;

    let failed = trace.status == "failed";
    let mut trace_json = serde_json::to_value(&trace)?;
    trim_trace_outputs(&mut trace_json);
    Ok((trace_json, failed))
}

// ---------------------------------------------------------------------------
// Elicitation plumbing
// ---------------------------------------------------------------------------

enum ElicitOutcome {
    Accepted,
    Declined,
    /// Timeout, cancel, or no confirmation channel — never treated as a yes.
    NoAnswer,
}

/// The per-run human gate, in channel-preference order:
/// 1. MCP elicitation, when the client declared the capability;
/// 2. the local approval inbox, when the Cori desktop app is running —
///    it renders a rich decision UI (`cori-run::approvals`);
/// 3. a native OS dialog on this machine (MCP servers run on the host,
///    so the dialog reaches the same human who owns the machine).
///
/// Fails closed at every rung: an unanswered channel is a decline, and a
/// live Console that doesn't answer never falls through to the dialog
/// (no double-prompting).
fn confirm_with_human(
    shared: &Shared,
    message: &str,
    kind: cori_run::approvals::ApprovalKind,
    payload: JsonValue,
) -> Result<ElicitOutcome> {
    if *shared.client_can_elicit.lock().expect("elicit lock") {
        return elicit_confirm(shared, message);
    }
    if cori_run::approvals::console_alive() {
        let req = cori_run::approvals::submit(kind, "mcp", message, payload, ELICIT_TIMEOUT)?;
        return Ok(
            match cori_run::approvals::wait_decision(&req.nonce, ELICIT_TIMEOUT)? {
                Some(dec) if dec.decision == cori_run::approvals::Decision::Approved => {
                    ElicitOutcome::Accepted
                }
                Some(_) => ElicitOutcome::Declined,
                None => ElicitOutcome::NoAnswer,
            },
        );
    }
    native_confirm(message)
}

/// Native confirmation dialog fallback. Disabled with
/// `CORI_MCP_DISABLE_NATIVE_CONFIRM=1` (headless servers, tests) — which
/// makes `run` refuse, never auto-approve.
fn native_confirm(message: &str) -> Result<ElicitOutcome> {
    if std::env::var("CORI_MCP_DISABLE_NATIVE_CONFIRM").is_ok_and(|v| v == "1") {
        return Ok(ElicitOutcome::NoAnswer);
    }
    native_confirm_impl(message)
}

#[cfg(target_os = "macos")]
fn native_confirm_impl(message: &str) -> Result<ElicitOutcome> {
    let escaped = message.replace('\\', "\\\\").replace('"', "\\\"");
    let script = format!(
        "display dialog \"{escaped}\" with title \"Cori\" \
         buttons {{\"Don't run\", \"Run\"}} default button \"Don't run\" \
         cancel button \"Don't run\" with icon caution \
         giving up after {timeout}",
        timeout = ELICIT_TIMEOUT.as_secs(),
    );
    let out = std::process::Command::new("osascript")
        .arg("-e")
        .arg(script)
        .output();
    Ok(match out {
        Ok(o) if o.status.success() => {
            // "giving up after" returns success with `gave up:true`.
            if String::from_utf8_lossy(&o.stdout).contains("gave up:true") {
                ElicitOutcome::NoAnswer
            } else {
                ElicitOutcome::Accepted
            }
        }
        // Cancel button (or dialog dismissed) exits non-zero.
        Ok(_) => ElicitOutcome::Declined,
        // osascript unavailable / no window server.
        Err(_) => ElicitOutcome::NoAnswer,
    })
}

#[cfg(target_os = "windows")]
fn native_confirm_impl(message: &str) -> Result<ElicitOutcome> {
    let escaped = message.replace('\'', "''");
    let script = format!(
        "Add-Type -AssemblyName System.Windows.Forms; \
         [System.Windows.Forms.MessageBox]::Show('{escaped}', 'Cori', 'YesNo', 'Warning', 'Button2')"
    );
    let out = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output();
    Ok(match out {
        Ok(o) if o.status.success() => {
            if String::from_utf8_lossy(&o.stdout).trim().ends_with("Yes") {
                ElicitOutcome::Accepted
            } else {
                ElicitOutcome::Declined
            }
        }
        _ => ElicitOutcome::NoAnswer,
    })
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn native_confirm_impl(message: &str) -> Result<ElicitOutcome> {
    let out = std::process::Command::new("zenity")
        .arg("--question")
        .arg("--title=Cori")
        .arg(format!("--text={message}"))
        .arg(format!("--timeout={}", ELICIT_TIMEOUT.as_secs()))
        .output();
    Ok(match out {
        Ok(o) if o.status.success() => ElicitOutcome::Accepted,
        // zenity exits 5 on timeout, 1 on "No"/close.
        Ok(o) if o.status.code() == Some(5) => ElicitOutcome::NoAnswer,
        Ok(_) => ElicitOutcome::Declined,
        Err(_) => ElicitOutcome::NoAnswer,
    })
}

fn elicit_confirm(shared: &Shared, message: &str) -> Result<ElicitOutcome> {
    let id = shared.next_req_id.fetch_add(1, Ordering::SeqCst);
    let (tx, rx) = mpsc::channel();
    shared.pending.lock().expect("pending lock").insert(id, tx);

    shared.send(&json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "elicitation/create",
        "params": {
            "message": message,
            "requestedSchema": {
                "type": "object",
                "properties": {
                    "confirm": {
                        "type": "boolean",
                        "title": "Confirm",
                        "description": "true to proceed"
                    }
                },
                "required": ["confirm"]
            }
        }
    }));

    let response = match rx.recv_timeout(ELICIT_TIMEOUT) {
        Ok(r) => r,
        Err(_) => {
            shared.pending.lock().expect("pending lock").remove(&id);
            return Ok(ElicitOutcome::NoAnswer);
        }
    };
    let action = response
        .pointer("/result/action")
        .and_then(|a| a.as_str())
        .unwrap_or("cancel");
    let confirmed = response
        .pointer("/result/content/confirm")
        .and_then(|c| c.as_bool())
        // Some clients return no content for a plain accept; accept means yes.
        .unwrap_or(true);
    Ok(match action {
        "accept" if confirmed => ElicitOutcome::Accepted,
        "accept" | "decline" => ElicitOutcome::Declined,
        _ => ElicitOutcome::NoAnswer,
    })
}

// ---------------------------------------------------------------------------
// Progress
// ---------------------------------------------------------------------------

struct McpProgressSink {
    shared: Shared,
    token: JsonValue,
    total: usize,
    done: AtomicU64,
}

impl cori_run::ProgressSink for McpProgressSink {
    fn on_plan(&self, plan: &[cori_run::planner::StepAssignment]) {
        self.shared.send(&json!({
            "jsonrpc": "2.0",
            "method": "notifications/progress",
            "params": {
                "progressToken": self.token,
                "progress": 0,
                "total": self.total,
                "message": format!("planned {} steps", plan.len()),
            }
        }));
    }

    fn on_step_start(&self, _summary: &cori_worker::workflow::ActivitySummary) {}

    fn on_step_finish(&self, summary: &cori_worker::workflow::ActivitySummary) {
        let done = self.done.fetch_add(1, Ordering::SeqCst) + 1;
        self.shared.send(&json!({
            "jsonrpc": "2.0",
            "method": "notifications/progress",
            "params": {
                "progressToken": self.token,
                "progress": done,
                "total": self.total,
                "message": format!(
                    "{status} {name} ({ms}ms)",
                    status = summary.status,
                    name = summary.step_name,
                    ms = summary.duration_ms,
                ),
            }
        }));
    }
}

// ---------------------------------------------------------------------------
// Skill resources
// ---------------------------------------------------------------------------

fn skill_stamp(what: &str) -> String {
    format!(
        "{what} from the `cori-save-workflow` skill (embedded copy shipped with \
         cori {v}; the npx-installed skill may be newer)",
        v = env!("CARGO_PKG_VERSION")
    )
}

fn resource_list() -> JsonValue {
    let entry = |uri: &str, name: &str, what: &str| {
        json!({
            "uri": uri,
            "name": name,
            "mimeType": "text/markdown",
            "description": skill_stamp(what),
        })
    };
    json!([
        entry(
            "cori://skill/SKILL.md",
            "cori-save-workflow",
            "Capture procedure"
        ),
        entry(
            "cori://skill/references/activity_kinds.md",
            "activity_kinds",
            "The step kinds and their TypeScript templates"
        ),
        entry(
            "cori://skill/references/example_workflow.md",
            "example_workflow",
            "A complete worked example"
        ),
        entry(
            "cori://skill/references/manifest_schema.md",
            "manifest_schema",
            "Manifest frontmatter spec"
        ),
        entry(
            "cori://skill/references/trace_interpretation.md",
            "trace_interpretation",
            "How to read a persisted RunTrace"
        ),
    ])
}

fn resource_body(uri: &str) -> Option<&'static str> {
    match uri {
        "cori://skill/SKILL.md" => Some(SKILL_MD),
        "cori://skill/references/activity_kinds.md" => Some(REF_ACTIVITY_KINDS),
        "cori://skill/references/example_workflow.md" => Some(REF_EXAMPLE_WORKFLOW),
        "cori://skill/references/manifest_schema.md" => Some(REF_MANIFEST_SCHEMA),
        "cori://skill/references/trace_interpretation.md" => Some(REF_TRACE_INTERPRETATION),
        _ => None,
    }
}

fn kind_label(kind: StepKind) -> &'static str {
    match kind {
        StepKind::Cli => "cli",
        StepKind::McpTool => "mcp_tool",
        StepKind::Code => "code",
        StepKind::Llm => "llm",
        StepKind::Builtin => "builtin",
    }
}
