// SPDX-License-Identifier: Apache-2.0
//! The single generic workflow: executes a compiled v2 DAG level by level, steps within a level concurrently.
//! Deterministic by construction: no wall clock, no I/O; every evaluation is an activity or a timer.
use crate::activities::{CoriActivities, StepInput, StepOutput, StepSpec};
use crate::config::{ResolvedConnector, ResolvedLlm};
use cori_compiler::dag::Node;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;
use temporalio_macros::{workflow, workflow_methods};
use temporalio_sdk::{ActivityOptions, SyncWorkflowContext, WorkflowContext, WorkflowResult};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowInput {
    pub compiled: cori_compiler::Compiled,
    pub params: Value,
    pub user: Value,
    /// Local mode: absolute workflow folder. Bundle mode: the workflow folder relative to the bundle root.
    pub root: String,
    /// Bundle mode: workers fetch `{server}/v1/bundles/{hash}/files` once and cache it.
    #[serde(default)]
    pub bundle: Option<BundleRef>,
    /// Local mode with a real store: the URL of an in-process store endpoint (`cori run --store-database-url`).
    #[serde(default)]
    pub store_server: Option<String>,
    /// Local runs only: directory where live connector calls are recorded as dev fixtures (`cori run --record`).
    #[serde(default)]
    pub record_dir: Option<String>,
    /// Set when a Temporal Schedule started this run: the workflow then indexes itself on the server
    /// (`run_started` / `run_finished` events), since no API call created a run row.
    #[serde(default)]
    pub schedule: Option<String>,
    pub connectors: BTreeMap<String, ResolvedConnector>,
    pub llm: ResolvedLlm,
    pub started_at: String,
    /// Queue name prefix for named pools: `<prefix>.<pool>` (e.g. `cori.pool`). Local runs leave it empty and run everything on their own queue.
    #[serde(default)]
    pub pool_queue_prefix: Option<String>,
    /// The queue of the platform's sandbox pool, which every organization of a server shares (their own pools are under
    /// `pool_queue_prefix`); none, `<pool_queue_prefix>.sandbox`.
    #[serde(default)]
    pub sandbox_queue: Option<String>,
    /// Pre-recorded human decisions (local runs / tests): step name → value.
    #[serde(default)]
    pub decisions: BTreeMap<String, Value>,
}

/// Where a hosted run's bundle and server are. On a server keeping organizations apart, `realm` is the run's: the worker
/// names it on every call to the server (`events::worker_auth`), and it is the run's fairness key on Temporal (`fair`).
/// `app` and `realm` are the lease of the run's code in a sandbox (`sandbox::lease_of`); `sandboxed`, that the server has
/// sandboxes: the run's code runs in one, never on a worker of the platform (`sandbox`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundleRef {
    pub hash: String,
    pub server: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub realm: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub sandboxed: bool,
}

impl BundleRef {
    /// The realm of a run's bundle, when it has one.
    pub fn realm_of(bundle: &Option<BundleRef>) -> Option<&str> { bundle.as_ref().and_then(|b| b.realm.as_deref()) }
}

/// A step of a run's trace. `attempts`: how many its activity took, Temporal's retries included (one for steps without an
/// activity, and for a native activity that succeeded: its SDK does not say). `ms`: from its start to its end, every
/// attempt and the waits between them included.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepTrace { pub name: String, pub kind: String, pub status: String, pub attempts: u32, pub ms: u64, pub cost_eur: f64, pub error: Option<String>, pub notes: Vec<String>,
    /// The way a branch, a switch or a try went: `then` or `else`, `cases.<label>` or `default`, `on_error`. None for
    /// other steps, and in runs recorded before it was kept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String> }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowOutput { pub run_id: String, pub status: String, /// the failure's own message (no Temporal wrapping); `error_detail` has its step and type
    pub error: Option<String>, pub steps: Map<String, Value>, pub trace: Vec<StepTrace>, pub result: Option<Value>,
    /// Absent on runs stored before it existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_detail: Option<ErrorDetail> }

/// Why a run failed: the step that failed (absent for run-level failures such as a budget), the error type
/// (`StepFailedError`, `SchemaValidationError`, `BudgetExceeded`, `Timeout`, …) and the message a front can show.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ErrorDetail { #[serde(default)] pub step: Option<String>, #[serde(rename = "type", default)] pub ty: Option<String>, pub message: String }

impl ErrorDetail {
    /// From a `Type: message` string (what steps fail with); a string without a recognised type prefix is all message.
    pub fn parse(step: Option<String>, e: &str) -> Self {
        match e.split_once(": ") {
            Some((ty, msg)) if !ty.is_empty() && ty.chars().next().is_some_and(|c| c.is_ascii_uppercase()) && ty.chars().all(|c| c.is_ascii_alphanumeric()) => Self { step, ty: Some(ty.to_string()), message: msg.to_string() },
            _ => Self { step, ty: None, message: e.to_string() },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionArgs { pub step: String, pub value: Value }

#[workflow]
#[derive(Default)]
pub struct CoriWorkflow { decisions: BTreeMap<String, Value> }

struct State { steps: Map<String, Value>, trace: Vec<StepTrace>, skipped: BTreeSet<String>, last_error: Option<Value>, seq: u64 }

#[workflow_methods]
impl CoriWorkflow {
    #[run]
    pub async fn run(ctx: &mut WorkflowContext<Self>, input: WorkflowInput) -> WorkflowResult<WorkflowOutput> {
        let run_id = ctx.workflow_id().to_string();
        ctx.state_mut(|s| { for (k, v) in &input.decisions { s.decisions.insert(k.clone(), v.clone()); } });
        if let (Some(sched), Some(b)) = (&input.schedule, &input.bundle) {
            let emit = crate::activities::EmitInput { server: Some(b.server.clone()), realm: b.realm.clone(), run_id: run_id.clone(), kind: "run_started".into(), payload: json!({ "workflow": input.compiled.id, "version": b.hash, "params": input.params, "user": input.user, "schedule": sched }) };
            let _ = ctx.execute_activity(CoriActivities::cori_emit, emit, fair(&input, emit_opts("run_started"))).await;
        }
        let state = RefCell::new(State { steps: Map::new(), trace: Vec::new(), skipped: BTreeSet::new(), last_error: None, seq: 0 });
        let by_name: BTreeMap<&str, &Node> = input.compiled.dag.nodes.iter().map(|n| (n.name.as_str(), n)).collect();
        let ctx_ref: &WorkflowContext<Self> = ctx;
        let mut status = "succeeded".to_string();
        let mut error: Option<ErrorDetail> = None;
        let mut stop = false;
        // budget: `max_duration` and `llm_eur_per_run` from the manifest, checked between levels (deterministic: workflow time)
        let budget = input.compiled.manifest.budget.clone().unwrap_or(Value::Null);
        // resolved by the compiler (`max_duration_ms`); runs of versions compiled before keep reading it the old way
        let max_ms = budget.get("max_duration_ms").and_then(|v| v.as_u64()).or_else(|| budget.get("max_duration").and_then(|d| d.as_str()).map(|d| legacy_duration(d).as_millis() as u64));
        let max_eur = budget.get("llm_eur_per_run").and_then(|e| e.as_f64());
        let start_ms = now_ms(ctx_ref);
        for level in &input.compiled.dag.schedule {
            if stop { break; }
            if let Some(m) = max_ms { if now_ms(ctx_ref) - start_ms > m { status = "failed".into(); error = Some(ErrorDetail { step: None, ty: Some("BudgetExceeded".into()), message: format!("run exceeded max_duration {}", budget["max_duration"]) }); break; } }
            if let Some(e) = max_eur { let spent: f64 = state.borrow().trace.iter().map(|t| t.cost_eur).sum(); if spent > e { status = "failed".into(); error = Some(ErrorDetail { step: None, ty: Some("BudgetExceeded".into()), message: format!("llm spend {spent:.4} € exceeds llm_eur_per_run {e}") }); break; } }
            let names: Vec<&String> = level.iter().filter(|n| !state.borrow().skipped.contains(*n)).collect();
            for n in level { if state.borrow().skipped.contains(n) { record_not_taken(&state, by_name[n.as_str()], &by_name); } }
            let futs: Vec<_> = names.iter().map(|n| exec_top(ctx_ref, &input, &state, &by_name, by_name[n.as_str()], &run_id)).collect();
            let outcomes = futures::future::join_all(futs).await;
            for (n, o) in names.iter().zip(outcomes) {
                match o {
                    Ok(Some(target)) => {
                        let mut st = state.borrow_mut();
                        for s in goto_skipped(&input.compiled.dag, n, &target) { st.skipped.insert(s); }
                        if target == "end" { stop = true; }
                    }
                    Ok(None) => {}
                    Err(e) => {
                        // the step that actually failed may be nested under this top-level one: `last_error` recorded it with the same message
                        let step = state.borrow().last_error.as_ref().filter(|l| l["message"].as_str() == Some(e.as_str())).and_then(|l| l["step"].as_str().map(String::from)).unwrap_or_else(|| n.to_string());
                        status = "failed".into(); if error.is_none() { error = Some(ErrorDetail::parse(Some(step), &e)); } stop = true;
                    }
                }
            }
        }
        let mut st = state.into_inner();
        let result = if status == "succeeded" { input.compiled.manifest.result.as_ref().map(|r| resolve_result(r, &input.params, &st.steps)) } else { None };
        // manifest `redact:` paths (steps.<step>.<field>…) never leave the run: blanked in the stored outputs (the result is rendered first)
        for path in &input.compiled.manifest.redact { redact_path(&mut st.steps, path.trim_start_matches("steps.")); }
        let out = WorkflowOutput { run_id: run_id.clone(), status, error: error.as_ref().map(|d| d.message.clone()), steps: st.steps, trace: st.trace, result, error_detail: error };
        if let (Some(_), Some(b)) = (&input.schedule, &input.bundle) {
            let emit = crate::activities::EmitInput { server: Some(b.server.clone()), realm: b.realm.clone(), run_id: run_id.clone(), kind: "run_finished".into(), payload: json!({ "status": out.status, "error": out.error, "output": serde_json::to_value(&out).unwrap_or(Value::Null), "result": out.result }) };
            let _ = ctx.execute_activity(CoriActivities::cori_emit, emit, fair(&input, emit_opts("run_finished"))).await;
        }
        Ok(out)
    }

    #[signal]
    pub fn human_decision(&mut self, _ctx: &mut SyncWorkflowContext<Self>, args: DecisionArgs) { self.decisions.insert(args.step, args.value); }
}

fn record_not_taken(state: &RefCell<State>, n: &Node, by_name: &BTreeMap<&str, &Node>) {
    state.borrow_mut().trace.push(StepTrace { name: n.name.clone(), kind: n.kind.clone(), status: "not_taken".into(), attempts: 0, ms: 0, cost_eur: 0.0, error: None, notes: vec![], path: None });
    for k in &n.nested { record_not_taken(state, by_name[k.as_str()], by_name); }
}

/// Executes a top-level step. Ok(Some(target)) = a goto was taken.
async fn exec_top<'a>(ctx: &'a WorkflowContext<CoriWorkflow>, input: &'a WorkflowInput, state: &'a RefCell<State>, by_name: &'a BTreeMap<&'a str, &'a Node>, node: &'a Node, run_id: &'a str) -> Result<Option<String>, String> {
    match exec_step(ctx, input, state, by_name, node, run_id, None).await {
        Ok(Exec::Value(v)) => { if !v.is_null() { state.borrow_mut().steps.insert(node.name.clone(), v); } Ok(None) }
        Ok(Exec::Goto(t)) => Ok(Some(t)),
        Err(e) => Err(e),
    }
}

enum Exec { Value(Value), Goto(String) }

fn now_ms(ctx: &WorkflowContext<CoriWorkflow>) -> u64 { ctx.workflow_time().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_millis() as u64).unwrap_or(0) }

fn step_input(input: &WorkflowInput, state: &RefCell<State>, node: &Node, run_id: &str, item: Option<Value>, selector: Option<&str>) -> StepInput {
    let st = state.borrow();
    let mut steps = Map::new();
    for d in &node.depends_on { if let Some(v) = st.steps.get(d) { steps.insert(d.clone(), v.clone()); } }
    let connector = node.literals.get("connector").and_then(|c| c.as_str()).and_then(|c| input.connectors.get(c).cloned())
        .or_else(|| if node.kind == "store" { input.connectors.get("default").cloned() } else { None });
    StepInput {
        run: json!({ "id": run_id, "workflow": input.compiled.id, "version": input.compiled.content_hash, "attempt": 1, "started_at": input.started_at, "last_error": st.last_error }),
        user: input.user.clone(), params: input.params.clone(), root: input.root.clone(), bundle: input.bundle.clone(), store_server: input.store_server.clone(), record_dir: input.record_dir.clone(), redact: input.compiled.manifest.redact.clone(),
        step: StepSpec { name: node.name.clone(), kind: node.kind.clone(), file: node.file.trim_start_matches("steps/").to_string(), literals: node.literals.clone() },
        steps, item, connector, llm: input.llm.clone(), selector: selector.map(String::from),
    }
}

/// Task queue for a step: its pool's queue when the run is hosted (`pool_for`), else the caller's own queue.
fn queue_for(input: &WorkflowInput, node: &Node) -> Option<String> {
    let prefix = input.pool_queue_prefix.as_ref()?;
    let connector = node.literals.get("connector").and_then(|c| c.as_str()).and_then(|c| input.connectors.get(c));
    let sandboxed = input.bundle.as_ref().map(|b| b.sandboxed).unwrap_or(false);
    pool_for(&node.kind, connector, sandboxed).map(|pool| match &input.sandbox_queue {
        Some(q) if pool == SANDBOX => q.clone(),
        _ => format!("{prefix}.{pool}"),
    })
}

/// The pool of the steps that start a program (`pool_for`): the platform's, never an organization's own.
pub const SANDBOX: &str = "sandbox";

/// The worker pool a hosted step runs on, none for the run's own (shared) queue: the connector's pinned pool, else
/// `sandbox` for a step that starts a program on its worker (`cli`, an MCP server over stdio). Such a step never runs on
/// the shared workers, whatever its pin says: they hold the cluster's worker credentials. A run whose server has
/// sandboxes (`sandboxed`) needs no such pool: every worker of the platform sends programs to a sandbox (`sandbox`).
pub(crate) fn pool_for(kind: &str, connector: Option<&ResolvedConnector>, sandboxed: bool) -> Option<String> {
    let starts_program = kind == "cli" || (kind == "mcp" && connector.map(|c| c.url.is_none() && c.command.is_some()).unwrap_or(false));
    match connector.and_then(|c| c.worker.clone()).filter(|p| !p.is_empty() && p != "shared" && !(sandboxed && p == SANDBOX)) {
        Some(pool) => Some(pool),
        None if starts_program && !sandboxed => Some(SANDBOX.into()),
        None => None,
    }
}

/// A child workflow's id is a folder of the app (`workflows/<id>`), joined to paths: letters, digits, `_` and `-` only.
pub fn valid_workflow_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Cori's own step errors that no retry fixes (the activities also mark them non-retryable where they raise them).
const NON_RETRYABLE: [&str; 6] = ["SchemaValidationError", "StepFailedError", "InvalidInputError", "MissingCapabilityError", "AuthenticationError", "BudgetExceeded"];
/// The errors a native activity's SDK raises that no retry fixes.
const NATIVE_NON_RETRYABLE: [&str; 2] = ["Forbidden", "InvalidInputError"];
/// The longest `timeout_ms` of a step Cori runs itself; a native activity runs on its team's worker, unbounded.
const STEP_TIMEOUT_MAX: Duration = Duration::from_secs(3600);

/// A step's activity timeout and attempts: its `timeout_ms` (at most `max`) and `retries.max` (1 to 10) options, else
/// its kind's defaults. A step is retried by default when a second attempt cannot do harm: a pure step (code, llm), a
/// read (store, mcp tools marked readOnlyHint), an HTTP GET/HEAD or `idempotent` request, a native activity.
fn step_limits(node: &Node, max: Duration) -> (Duration, i32) {
    let default_timeout = Duration::from_secs(match node.kind.as_str() { "cli" => 60, "llm" | "activity" => 120, _ => 30 });
    let read = matches!(node.effect.access, cori_compiler::effects::Access::Read);
    let default_attempts: i32 = match node.kind.as_str() {
        "code" | "llm" | "activity" => 3,
        "http" => if node.literals.get("method").and_then(|m| m.as_str()).map(|m| m == "GET" || m == "HEAD").unwrap_or(false) || node.literals.get("idempotent").and_then(|b| b.as_bool()).unwrap_or(false) { 3 } else { 1 },
        "store" | "mcp" => if read { 3 } else { 1 },
        _ => 1,
    };
    let timeout = node.literals.get("timeout_ms").and_then(|v| v.as_u64()).map(|ms| Duration::from_millis(ms.max(1)).min(max)).unwrap_or(default_timeout);
    let attempts = node.literals.get("retries").and_then(|r| r.get("max")).and_then(|m| m.as_u64()).map(|m| m.clamp(1, 10) as i32).unwrap_or(default_attempts);
    (timeout, attempts)
}

/// Retries 1 s apart at first, doubling up to 60 s, `attempts` in all.
fn retry_policy(attempts: i32, non_retryable: &[&str]) -> temporalio_common::protos::temporal::api::common::v1::RetryPolicy {
    temporalio_common::protos::temporal::api::common::v1::RetryPolicy {
        initial_interval: Some(prost_wkt_types::Duration { seconds: 1, nanos: 0 }), backoff_coefficient: 2.0, maximum_interval: Some(prost_wkt_types::Duration { seconds: 60, nanos: 0 }), maximum_attempts: attempts,
        non_retryable_error_types: non_retryable.iter().map(|t| t.to_string()).collect(),
    }
}

/// The options of a step's activity on the workflow's own queue.
fn opts(node: &Node, id: &str) -> ActivityOptions {
    let (timeout, attempts) = step_limits(node, STEP_TIMEOUT_MAX);
    ActivityOptions::with_start_to_close_timeout(timeout).activity_id(id.to_string()).summary(node.name.clone()).retry_policy(retry_policy(attempts, &NON_RETRYABLE)).build()
}

/// `opts` on the step's pool queue when it has one (a connector's worker pool, the sandbox for cli steps).
fn opts_on(input: &WorkflowInput, node: &Node, id: &str) -> ActivityOptions {
    match queue_for(input, node) {
        Some(q) => {
            let (timeout, attempts) = step_limits(node, STEP_TIMEOUT_MAX);
            ActivityOptions::with_start_to_close_timeout(timeout).activity_id(id.to_string()).summary(format!("{} on {q}", node.name)).retry_policy(retry_policy(attempts, &NON_RETRYABLE)).task_queue(q).build()
        }
        None => opts(node, id),
    }
}

/// A native activity's options on its team's queue: the step's limits (its timeout unbounded), and from its declaration
/// (activities.yaml, resolved by the compiler) the errors not to retry besides the SDKs' own, and the heartbeat timeout.
fn native_opts(node: &Node, id: &str, summary: String, queue: &str) -> ActivityOptions {
    let (timeout, attempts) = step_limits(node, Duration::MAX);
    let mut non_retryable: Vec<&str> = NATIVE_NON_RETRYABLE.to_vec();
    non_retryable.extend(node.literals.get("non_retryable").and_then(|v| v.as_array()).into_iter().flatten().filter_map(|t| t.as_str()));
    let heartbeat = node.literals.get("heartbeat_ms").and_then(|v| v.as_u64()).filter(|ms| *ms > 0).map(Duration::from_millis);
    ActivityOptions::with_start_to_close_timeout(timeout).activity_id(id.to_string()).summary(summary).retry_policy(retry_policy(attempts, &non_retryable)).task_queue(queue.to_string()).maybe_heartbeat_timeout(heartbeat).build()
}

/// `o` with the run's realm as Temporal's fairness key (`BundleRef::realm`), so that one organization's runs do not starve
/// another's on a shared queue. Set on every activity and child: a run a Temporal Schedule started has no key to inherit.
fn fair(input: &WorkflowInput, mut o: ActivityOptions) -> ActivityOptions { o.priority = fairness(input); o }

fn fairness(input: &WorkflowInput) -> Option<temporalio_common::Priority> {
    BundleRef::realm_of(&input.bundle).map(|r| temporalio_common::Priority::builder().fairness_key(r).build())
}

/// The options of an event sent to the server (`cori_emit`): retried without limit, so a server that is down delays the
/// run rather than losing the event (an approval nobody sees would never be decided).
fn emit_opts(id: impl Into<String>) -> ActivityOptions {
    ActivityOptions::with_start_to_close_timeout(Duration::from_secs(30)).activity_id(id.into()).build()
}

/// `ctx.run.last_error` for `try … on_error`: `{ message, step, type }`, `message` kept whole (it matches the step's trace error).
fn set_last_error(state: &RefCell<State>, node: &Node, e: &str) {
    let d = ErrorDetail::parse(Some(node.name.clone()), e);
    state.borrow_mut().last_error = Some(json!({ "message": e, "step": node.name, "type": d.ty }));
}

/// A failed activity as `Type: message`, from the application failure the activity raised (activities.rs `to_activity_error`),
/// without Temporal's `Activity failed: Activity task failed: Failure(…)` wrapping. Timeouts and cancellations get their own type.
fn act_err(e: &temporalio_sdk::ActivityExecutionError) -> String {
    use temporalio_common::error::IncomingError as I;
    let mut c = e.cause();
    while let Some(i) = c {
        match i {
            I::Application(a) => { let ty = a.type_name().unwrap_or("StepFailedError"); let m = a.to_string(); return if m.starts_with(&format!("{ty}: ")) { m } else { format!("{ty}: {m}") }; }
            I::Timeout(t) => return format!("Timeout: step timed out ({:?})", t.timeout_type()),
            I::Cancelled(_) => return "Cancelled: step was cancelled".into(),
            I::Terminated(_) => return "Terminated: run was terminated".into(),
            _ => c = i.cause(),
        }
    }
    err_chain(e)
}

fn err_chain(e: &(dyn std::error::Error + 'static)) -> String {
    let mut msg = e.to_string();
    let mut src = e.source();
    while let Some(s) = src { let t = s.to_string(); if !msg.contains(&t) { msg.push_str(": "); msg.push_str(&t); } src = s.source(); }
    msg
}

/// How many attempts a failed activity made: the last one's number, which `cori_step` puts in its failure's details
/// (`FailedAttempt`); for an activity that does not say (a native one, a timed-out attempt), all `max` when Temporal says
/// they ran out, else one.
fn attempts_of(e: &temporalio_sdk::ActivityExecutionError, max: i32) -> u32 {
    use temporalio_common::error::{IncomingError as I, RetryState};
    let mut c = e.cause();
    while let Some(i) = c {
        if let I::Application(a) = i && let Ok(Some(f)) = a.details::<crate::activities::FailedAttempt>() { return f.attempt.max(1); }
        c = i.cause();
    }
    match e {
        temporalio_sdk::ActivityExecutionError::Failed(f) if matches!(f.retry_state(), RetryState::MaximumAttemptsReached) => max.max(1) as u32,
        _ => 1,
    }
}

/// A step's activity: its output, or its error and how many attempts it made.
async fn call_step(ctx: &WorkflowContext<CoriWorkflow>, input: &WorkflowInput, state: &RefCell<State>, node: &Node, run_id: &str, item: Option<Value>, id: &str) -> Result<StepOutput, (String, u32)> {
    let si = step_input(input, state, node, run_id, item, None);
    ctx.execute_activity(CoriActivities::cori_step, si, fair(input, opts_on(input, node, id))).await.map_err(|e| (act_err(&e), attempts_of(&e, step_limits(node, STEP_TIMEOUT_MAX).1)))
}

async fn select(ctx: &WorkflowContext<CoriWorkflow>, input: &WorkflowInput, state: &RefCell<State>, node: &Node, run_id: &str, selector: &str, item: Option<Value>, id: &str) -> Result<Value, String> {
    let si = step_input(input, state, node, run_id, item, Some(selector));
    let o = ctx.execute_activity(CoriActivities::cori_select, si, fair(input, opts(node, id))).await.map_err(|e| act_err(&e))?;
    Ok(o.output)
}

fn push_trace(state: &RefCell<State>, node: &Node, status: &str, ms: u64, out: Option<&StepOutput>, error: Option<String>) {
    push_attempts(state, node, status, ms, out, error, out.map(|o| o.attempts).unwrap_or(1));
}

/// `push_trace` of a step whose activity took `attempts` (at least one: an output from before steps counted them says 0).
fn push_attempts(state: &RefCell<State>, node: &Node, status: &str, ms: u64, out: Option<&StepOutput>, error: Option<String>, attempts: u32) {
    state.borrow_mut().trace.push(StepTrace { name: node.name.clone(), kind: node.kind.clone(), status: status.into(), attempts: attempts.max(1), ms, cost_eur: out.map(|o| o.cost_eur).unwrap_or(0.0), error, notes: out.map(|o| o.notes.clone()).unwrap_or_default(), path: None });
}

/// Records the way a builtin went (`StepTrace::path`) on its trace entry (its latest: the steps it skipped come after it).
fn went(state: &RefCell<State>, node: &Node, path: &str) { if let Some(t) = state.borrow_mut().trace.iter_mut().rev().find(|t| t.name == node.name) { t.path = Some(path.into()); } }

fn path_of<'a>(node: &'a Node, by_name: &BTreeMap<&'a str, &'a Node>, slot: &str) -> Option<Result<&'a Node, String>> {
    if let Some((_, t)) = node.goto_targets.iter().find(|(s, _)| s == slot) { return Some(Err(t.clone())); }
    node.nested.iter().map(|n| by_name[n.as_str()]).find(|n| n.slot.as_deref() == Some(slot)).map(Ok)
}

/// What a native activity receives: the step's input and the run's context, the SDKs' `Envelope` (sdks/go, sdks/dotnet).
/// sdks/testdata/native_envelope.json is its golden form: this crate's tests check it is produced, the SDKs' decode it.
#[derive(Debug, Serialize)]
pub struct NativeEnvelope<'a> {
    pub input: &'a Value,
    pub cori: NativeContext<'a>,
}

/// The `cori` half of a native activity's envelope.
#[derive(Debug, Serialize)]
pub struct NativeContext<'a> {
    pub run_id: &'a str,
    pub workflow: &'a str,
    /// The run's workflow version (its content hash), not the activity's `version`.
    pub version: &'a str,
    pub step: &'a str,
    /// Always 1: Temporal retries the activity itself, and a worker reads the attempt from its activity info.
    pub attempt: u32,
    /// The run's user as Cori resolved it (`sub`, `roles`, `email`, and the identity's other claims).
    pub user: &'a Value,
    /// The token exchanged for the connector's audience, sealed for the connector's own worker (`cori-sealed.v1.…`, which
    /// its key opens: `cori connectors key <name>`) so that Temporal's history holds no token; null without one (local
    /// runs, no audience).
    pub principal_token: &'a Value,
    /// Stable across the retries of the step in the run: `<run>:<step>`, `<run>:<step>#<n>` per map item.
    pub idempotency_key: String,
}

/// (task queue, activity name, version) when the step's connector is a live native-activity connector (no dev mock).
fn native_target(input: &WorkflowInput, node: &Node) -> Option<(String, String, i64)> {
    let c = node.literals.get("connector")?.as_str()?;
    let conn = input.connectors.get(c)?;
    if conn.kind != "activity" || conn.mock.is_some() { return None; }
    let queue = conn.task_queue.clone()?;
    let act = node.literals.get("activity")?.as_str()?.to_string();
    let version = node.literals.get("version").and_then(|v| v.as_f64()).map(|v| v as i64).unwrap_or(1);
    Some((queue, act, version))
}

/// A duration as versions compiled before `cori check` validated them read it: a bare number or an unknown unit meant
/// minutes, and `ms` too. Only for them: `cori check` now accepts only what `cori_compiler::timing::duration` reads.
fn legacy_duration(s: &str) -> Duration {
    let t = s.trim();
    let (num, unit) = t.split_at(t.find(|c: char| !c.is_ascii_digit()).unwrap_or(t.len()));
    let n: u64 = num.parse().unwrap_or(0);
    Duration::from_secs(match unit { "s" => n, "m" => n.saturating_mul(60), "h" => n.saturating_mul(3600), "d" => n.saturating_mul(86400), _ => n.saturating_mul(60) })
}

/// Executes any step (top-level or nested). Nested outputs are recorded under their own names.
fn exec_step<'a>(ctx: &'a WorkflowContext<CoriWorkflow>, input: &'a WorkflowInput, state: &'a RefCell<State>, by_name: &'a BTreeMap<&'a str, &'a Node>, node: &'a Node, run_id: &'a str, item: Option<Value>) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Exec, String>> + 'a>> {
    Box::pin(exec_step_inner(ctx, input, state, by_name, node, run_id, item))
}

async fn exec_step_inner<'a>(ctx: &'a WorkflowContext<CoriWorkflow>, input: &'a WorkflowInput, state: &'a RefCell<State>, by_name: &'a BTreeMap<&'a str, &'a Node>, node: &'a Node, run_id: &'a str, item: Option<Value>) -> Result<Exec, String> {
    let t0 = now_ms(ctx);
    // Activity ids must be unique per run: nested and per-item dispatches get a deterministic sequence number.
    let id = { let mut st = state.borrow_mut(); st.seq += 1; if item.is_some() || node.nested_in.is_some() { format!("{}#{}", node.name, st.seq) } else { node.name.clone() } };
    let v: Value = match node.kind.as_str() {
        "activity" if native_target(input, node).is_some() => {
            // Native Temporal activity on the developers' queue: bind → dispatch by name → validate.
            let (queue, act, version) = native_target(input, node).unwrap();
            let bound = select(ctx, input, state, node, run_id, "bind", item.clone(), &format!("{id}#bind")).await?;
            let envelope = NativeEnvelope { input: &bound["input"], cori: NativeContext {
                run_id, workflow: &input.compiled.id, version: &input.compiled.content_hash, step: &node.name, attempt: 1, user: &input.user,
                principal_token: &bound["principal_token"], idempotency_key: if item.is_some() { format!("{run_id}:{id}") } else { format!("{run_id}:{}", node.name) },
            } };
            let envelope = serde_json::to_value(&envelope).map_err(|e| e.to_string())?;
            let opts = native_opts(node, &id, format!("{act}@{version} on {queue}"), &queue);
            let raw = ctx.execute_activity(temporalio_common::UntypedActivity::new(act.clone()), crate::activities::json_raw(&envelope), fair(input, opts)).await
                .map_err(|e| (act_err(&e), attempts_of(&e, step_limits(node, Duration::MAX).1))).and_then(|r| crate::activities::raw_json(r).map_err(|e| (e.to_string(), 1)));
            match raw {
                Ok(v) => {
                    let out = select(ctx, input, state, node, run_id, "validate", Some(v), &format!("{id}#validate")).await?;
                    state.borrow_mut().trace.push(StepTrace { name: node.name.clone(), kind: node.kind.clone(), status: "ok".into(), attempts: 1, ms: now_ms(ctx) - t0, cost_eur: 0.0, error: None, notes: vec![format!("{act}@{version} on {queue}")], path: None });
                    out
                }
                Err((e, attempts)) => { push_attempts(state, node, "failed", now_ms(ctx) - t0, None, Some(e.clone()), attempts); set_last_error(state, node, &e); return Err(e); }
            }
        }
        "workflow" => {
            // Child workflow: bind the input, load the sibling's compiled DAG, run it as its own Temporal workflow
            // (same generic code, same user, the app's connectors), validate what it returns against `output`.
            let child_id = node.literals.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
            if !valid_workflow_id(&child_id) {
                let e = format!("InvalidInputError: child workflow `{child_id}` is not a workflow folder name (letters, digits, `_`, `-`)");
                push_trace(state, node, "failed", now_ms(ctx) - t0, None, Some(e.clone())); set_last_error(state, node, &e); return Err(e);
            }
            let bound = select(ctx, input, state, node, run_id, "bind", item.clone(), &format!("{id}#bind")).await?;
            let load = crate::activities::LoadWorkflowInput { root: input.root.clone(), bundle: input.bundle.clone(), id: child_id.clone() };
            let compiled = ctx.execute_activity(CoriActivities::cori_load_workflow, load, fair(input, ActivityOptions::with_start_to_close_timeout(Duration::from_secs(120)).activity_id(format!("{id}#load")).summary(format!("load workflow {child_id}")).build())).await.map_err(|e| act_err(&e));
            let compiled = match compiled { Ok(c) => c, Err(e) => { push_trace(state, node, "failed", now_ms(ctx) - t0, None, Some(e.clone())); set_last_error(state, node, &e); return Err(e); } };
            let child_root = std::path::Path::new(&input.root).parent().map(|p| p.join(&child_id).display().to_string()).unwrap_or_else(|| child_id.clone());
            let child_input = WorkflowInput { compiled, params: bound["input"].clone(), user: input.user.clone(), root: child_root, bundle: input.bundle.clone(), store_server: input.store_server.clone(), record_dir: input.record_dir.clone(), schedule: None, connectors: input.connectors.clone(), llm: input.llm.clone(), started_at: input.started_at.clone(), pool_queue_prefix: input.pool_queue_prefix.clone(), sandbox_queue: input.sandbox_queue.clone(), decisions: BTreeMap::new() };
            let child_wf_id = format!("{run_id}:{id}");
            let opts = temporalio_sdk::ChildWorkflowOptions::builder().workflow_id(child_wf_id.clone()).static_summary(format!("{} → {child_id}", node.name)).maybe_priority(fairness(input)).build();
            let started = ctx.start_child_workflow(CoriWorkflow::run, child_input, opts).await.map_err(|e| format!("child workflow `{child_id}` could not start: {e}"));
            if input.bundle.is_some() && started.is_ok() {
                let emit = crate::activities::EmitInput { server: input.bundle.as_ref().map(|b| b.server.clone()), realm: BundleRef::realm_of(&input.bundle).map(String::from), run_id: run_id.to_string(), kind: "child_started".into(), payload: json!({ "child_run_id": child_wf_id, "workflow": child_id, "step": node.name, "params": bound["input"] }) };
                let _ = ctx.execute_activity(CoriActivities::cori_emit, emit, fair(input, emit_opts(format!("{id}#child_started")))).await;
            }
            let res: Result<WorkflowOutput, String> = match started { Ok(h) => h.result().await.map_err(|e| format!("child workflow `{child_id}` failed: {e}")), Err(e) => Err(e) };
            if input.bundle.is_some() {
                let (status, error, output) = match &res { Ok(o) => (o.status.clone(), o.error.clone(), Some(serde_json::to_value(o).unwrap_or(Value::Null))), Err(e) => ("failed".to_string(), Some(e.clone()), None) };
                let emit = crate::activities::EmitInput { server: input.bundle.as_ref().map(|b| b.server.clone()), realm: BundleRef::realm_of(&input.bundle).map(String::from), run_id: run_id.to_string(), kind: "child_finished".into(), payload: json!({ "child_run_id": child_wf_id, "status": status, "error": error, "output": output }) };
                let _ = ctx.execute_activity(CoriActivities::cori_emit, emit, fair(input, emit_opts(format!("{id}#child_finished")))).await;
            }
            match res {
                Ok(o) if o.status == "succeeded" => {
                    // what the child exposes: its manifest result values when it has any, else every step output
                    let values = o.result.as_ref().and_then(|r| r.get("values")).filter(|v| v.as_object().map(|m| !m.is_empty()).unwrap_or(false)).cloned().unwrap_or_else(|| Value::Object(o.steps.clone()));
                    match select(ctx, input, state, node, run_id, "validate", Some(values), &format!("{id}#validate")).await {
                        Ok(v) => { state.borrow_mut().trace.push(StepTrace { name: node.name.clone(), kind: node.kind.clone(), status: "ok".into(), attempts: 1, ms: now_ms(ctx) - t0, cost_eur: o.trace.iter().map(|t| t.cost_eur).sum(), error: None, notes: vec![format!("child {child_id} · {} · {} steps", child_wf_id, o.trace.len())], path: None }); v }
                        Err(e) => { push_trace(state, node, "failed", now_ms(ctx) - t0, None, Some(e.clone())); set_last_error(state, node, &e); return Err(e); }
                    }
                }
                Ok(o) => { let e = format!("child workflow `{child_id}` {}: {}", o.status, o.error.unwrap_or_default()); push_trace(state, node, "failed", now_ms(ctx) - t0, None, Some(e.clone())); set_last_error(state, node, &e); return Err(e); }
                Err(e) => { push_trace(state, node, "failed", now_ms(ctx) - t0, None, Some(e.clone())); set_last_error(state, node, &e); return Err(e); }
            }
        }
        "code" | "llm" | "http" | "mcp" | "store" | "activity" | "cli" => {
            match call_step(ctx, input, state, node, run_id, item, &id).await {
                Ok(o) => { push_trace(state, node, "ok", now_ms(ctx) - t0, Some(&o), None); o.output }
                Err((e, attempts)) => { push_attempts(state, node, "failed", now_ms(ctx) - t0, None, Some(e.clone()), attempts); set_last_error(state, node, &e); return Err(e); }
            }
        }
        "human" => {
            let name = node.name.clone();
            let timeout = node.literals.get("timeout").and_then(|t| t.as_str()).map(|t| cori_compiler::timing::duration(t).unwrap_or_else(|_| legacy_duration(t))).unwrap_or(Duration::from_secs(30 * 86400));
            // Tell the server an approval is pending (with what the approver should see); local runs skip.
            if input.bundle.is_some() && !ctx.state(|s| s.decisions.contains_key(&name)) {
                let shown = select(ctx, input, state, node, run_id, "bind", item.clone(), &format!("{id}#bind")).await.map(|v| v.get("input").cloned().unwrap_or(v)).unwrap_or(Value::Null);
                let emit = crate::activities::EmitInput { server: input.bundle.as_ref().map(|b| b.server.clone()), realm: BundleRef::realm_of(&input.bundle).map(String::from), run_id: run_id.to_string(), kind: "approval_pending".into(), payload: json!({ "step": name, "kind": node.literals.get("kind"), "to": node.literals.get("to"), "timeout": node.literals.get("timeout"), "payload": shown, "workflow": input.compiled.id }) };
                let _ = ctx.execute_activity(CoriActivities::cori_emit, emit, fair(input, emit_opts(format!("{id}#notify")))).await;
            }
            let mut decided = false;
            let name_w = name.clone();
            temporalio_sdk::workflows::select! {
                _ = ctx.timer(timeout) => {}
                _ = ctx.wait_condition(move |s: &CoriWorkflow| s.decisions.contains_key(&name_w)) => { decided = true; }
            }
            if decided {
                let v = ctx.state_mut(|s| s.decisions.remove(&name)).unwrap_or(Value::Null);
                push_trace(state, node, "ok", now_ms(ctx) - t0, None, None);
                v
            } else {
                match node.literals.get("on_timeout").and_then(|o| o.get("default")) {
                    Some(d) => { let mut st = state.borrow_mut(); st.trace.push(StepTrace { name: name.clone(), kind: "human".into(), status: "ok".into(), attempts: 1, ms: now_ms(ctx) - t0, cost_eur: 0.0, error: None, notes: vec!["timed out; on_timeout default used".into()], path: None }); d.clone() }
                    None => { let e = format!("human step `{name}` timed out"); push_trace(state, node, "failed", now_ms(ctx) - t0, None, Some(e.clone())); return Err(e); }
                }
            }
        }
        "wait" => {
            let spec = node.literals.get("for").cloned().unwrap_or(Value::Null);
            let ms = spec.get("timeout_ms").and_then(|v| v.as_u64()).unwrap_or(0);
            if ms > 0 { let _ = ctx.timer(Duration::from_millis(ms.min(30 * 86400 * 1000))).await; }
            push_trace(state, node, "ok", now_ms(ctx) - t0, None, None);
            Value::Null
        }
        "branch" => {
            let cond = select(ctx, input, state, node, run_id, "if", item.clone(), &format!("{id}#if")).await?.as_bool().unwrap_or(false);
            push_trace(state, node, "ok", now_ms(ctx) - t0, None, None);
            let slot = if cond { "then" } else { "else" };
            went(state, node, slot);
            let other = if cond { "else" } else { "then" };
            if let Some(Ok(n)) = path_of(node, by_name, other) { record_not_taken(state, n, by_name); }
            return follow(ctx, input, state, by_name, node, run_id, item, slot).await;
        }
        "switch" => {
            let label = select(ctx, input, state, node, run_id, "on", item.clone(), &format!("{id}#on")).await?.as_str().unwrap_or("").to_string();
            push_trace(state, node, "ok", now_ms(ctx) - t0, None, None);
            let slot = format!("cases.{label}");
            let has = path_of(node, by_name, &slot).is_some();
            let chosen = if has { slot } else { "default".to_string() };
            for n in &node.nested { let nn = by_name[n.as_str()]; if nn.slot.as_deref() != Some(&chosen) { record_not_taken(state, nn, by_name); } }
            if !has && path_of(node, by_name, "default").is_none() { return Err(format!("switch `{}`: no case for `{label}` and no default", node.name)); }
            went(state, node, &chosen);
            return follow(ctx, input, state, by_name, node, run_id, item, &chosen).await;
        }
        "map" => {
            let items = select(ctx, input, state, node, run_id, "over", item.clone(), &format!("{id}#over")).await?;
            let items: Vec<Value> = items.as_array().cloned().unwrap_or_default();
            let each = match path_of(node, by_name, "each") { Some(Ok(n)) => n, _ => return Err(format!("map `{}` has no `each`", node.name)) };
            let conc = node.literals.get("max_concurrency").and_then(|v| v.as_u64()).unwrap_or(8).clamp(1, 64) as usize;
            let collect = node.literals.get("on_item_error").and_then(|v| v.as_str()) == Some("collect");
            let mut outputs: Vec<Value> = Vec::with_capacity(items.len());
            for chunk in items.chunks(conc) {
                let futs: Vec<_> = chunk.iter().map(|it| exec_step(ctx, input, state, by_name, each, run_id, Some(it.clone()))).collect();
                for r in futures::future::join_all(futs).await {
                    match r { Ok(Exec::Value(v)) => outputs.push(v), Ok(Exec::Goto(_)) => return Err("goto inside map is not allowed".into()), Err(e) => if collect { outputs.push(Value::Null) } else { return Err(e) } }
                }
            }
            push_trace(state, node, "ok", now_ms(ctx) - t0, None, None);
            json!({ "items": outputs })
        }
        "loop" => {
            let body = match path_of(node, by_name, "body") { Some(Ok(n)) => n, _ => return Err(format!("loop `{}` has no `body`", node.name)) };
            let max = node.literals.get("max_iterations").and_then(|v| v.as_u64()).unwrap_or(10).clamp(1, 100);
            let mut last; let mut i = 0;
            loop {
                i += 1;
                match exec_step(ctx, input, state, by_name, body, run_id, Some(json!(i))).await? { Exec::Value(v) => { state.borrow_mut().steps.insert(body.name.clone(), v.clone()); last = v; } Exec::Goto(_) => return Err("goto inside loop is not allowed".into()) }
                let done = select(ctx, input, state, node, run_id, "until", Some(json!(i)), &format!("{id}#until[{i}]")).await?.as_bool().unwrap_or(true);
                if done { break; }
                if i >= max { return Err(format!("loop `{}` exhausted max_iterations ({max})", node.name)); }
            }
            push_trace(state, node, "ok", now_ms(ctx) - t0, None, None);
            last
        }
        "try" => {
            let body = match path_of(node, by_name, "body") { Some(Ok(n)) => n, _ => return Err(format!("try `{}` has no `body`", node.name)) };
            match exec_step(ctx, input, state, by_name, body, run_id, item.clone()).await {
                Ok(Exec::Value(v)) => { state.borrow_mut().steps.insert(body.name.clone(), v.clone()); if let Some(Ok(h)) = path_of(node, by_name, "on_error") { record_not_taken(state, h, by_name); } push_trace(state, node, "ok", now_ms(ctx) - t0, None, None); v }
                Ok(Exec::Goto(t)) => return Ok(Exec::Goto(t)),
                Err(e) => {
                    push_trace(state, node, "ok", now_ms(ctx) - t0, None, Some(format!("body failed: {e}; on_error taken")));
                    went(state, node, "on_error");
                    return follow(ctx, input, state, by_name, node, run_id, item, "on_error").await.map(|x| match x { Exec::Value(_) => Exec::Value(Value::Null), g => g });
                }
            }
        }
        other => return Err(format!("unknown step kind `{other}`")),
    };
    Ok(Exec::Value(v))
}

/// Follows a builtin's slot: runs the nested step (recording its output under its own name) or returns the goto.
async fn follow<'a>(ctx: &'a WorkflowContext<CoriWorkflow>, input: &'a WorkflowInput, state: &'a RefCell<State>, by_name: &'a BTreeMap<&'a str, &'a Node>, node: &'a Node, run_id: &'a str, item: Option<Value>, slot: &str) -> Result<Exec, String> {
    match path_of(node, by_name, slot) {
        Some(Err(target)) => Ok(Exec::Goto(target)),
        Some(Ok(n)) => match exec_step(ctx, input, state, by_name, n, run_id, item).await? {
            Exec::Value(v) => { if !v.is_null() { state.borrow_mut().steps.insert(n.name.clone(), v.clone()); } Ok(Exec::Value(Value::Null)) }
            g => Ok(g),
        },
        None => Ok(Exec::Value(Value::Null)),
    }
}

fn lookup<'v>(path: &str, params: &'v Value, steps: &'v Map<String, Value>) -> Value {
    let mut parts = path.split('.');
    let mut cur: Value = match parts.next() { Some("params") => params.clone(), Some("steps") => match parts.next() { Some(n) => steps.get(n).cloned().unwrap_or(Value::Null), None => Value::Null }, _ => Value::Null };
    for p in parts { cur = cur.get(p).cloned().unwrap_or(Value::Null); }
    cur
}

/// The top-level steps a goto from `from` to `target` skips: the compiler's list (declared order), or, for a DAG compiled
/// before it gave one, every step between the two in schedule order (the rule runs in flight started with).
fn goto_skipped(dag: &cori_compiler::dag::Dag, from: &str, target: &str) -> Vec<String> {
    if let Some(list) = dag.goto_skips.get(from).and_then(|t| t.get(target)) { return list.clone(); }
    let sched: Vec<&String> = dag.schedule.iter().flatten().collect();
    let start = sched.iter().position(|s| *s == from).unwrap_or(0);
    let end = if target == "end" { sched.len() } else { sched.iter().position(|s| *s == target).unwrap_or(sched.len()) };
    sched.get(start + 1..end).map(|r| r.iter().map(|s| s.to_string()).collect()).unwrap_or_default()
}

fn render_tpl(t: &str, params: &Value, steps: &Map<String, Value>) -> String {
    crate::template::render(t, |path| match lookup(path, params, steps) { Value::String(x) => x, Value::Null => String::new(), o => o.to_string() })
}

fn resolve_result(r: &Value, params: &Value, steps: &Map<String, Value>) -> Value {
    let headline = r.get("headline").and_then(|h| h.as_str()).map(|h| render_tpl(h, params, steps));
    let fields: Vec<Value> = r.get("fields").and_then(|f| f.as_array()).map(|fs| fs.iter().map(|f| json!({ "label": f.get("label"), "value": f.get("path").and_then(|p| p.as_str()).map(|p| lookup(p, params, steps)) })).collect()).unwrap_or_default();
    let sections: Vec<Value> = r.get("sections").and_then(|f| f.as_array()).map(|fs| fs.iter().map(|f| json!({ "label": f.get("label"), "value": f.get("path").and_then(|p| p.as_str()).map(|p| lookup(p, params, steps)) })).collect()).unwrap_or_default();
    let mut values = Map::new();
    for f in fields.iter().chain(sections.iter()) { if let (Some(l), Some(v)) = (f.get("label").and_then(|l| l.as_str()), f.get("value")) { values.insert(l.trim().to_lowercase().chars().map(|c| if c.is_alphanumeric() { c } else { '_' }).collect::<String>().split('_').filter(|p| !p.is_empty()).collect::<Vec<_>>().join("_"), v.clone()); } }
    json!({ "headline": headline, "fields": fields, "sections": sections, "values": values })
}

/// Sets `a.b.c` inside a JSON map to "<redacted>" when present (arrays: every element).
pub(crate) fn redact_path(root: &mut Map<String, Value>, path: &str) {
    let parts: Vec<&str> = path.split('.').filter(|p| !p.is_empty()).collect();
    fn go(v: &mut Value, parts: &[&str]) {
        match v {
            Value::Array(a) => { for x in a { go(x, parts); } }
            Value::Object(m) => {
                let Some((head, rest)) = parts.split_first() else { return };
                if let Some(child) = m.get_mut(*head) { if rest.is_empty() { *child = Value::String("<redacted>".into()); } else { go(child, rest); } }
            }
            _ => {}
        }
    }
    let Some((head, rest)) = parts.split_first() else { return };
    if let Some(v) = root.get_mut(*head) { if rest.is_empty() { *v = Value::String("<redacted>".into()); } else { go(v, rest); } }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(kind: &str, access: &str, literals: Value) -> Node {
        serde_json::from_value(json!({ "name": "s", "kind": kind, "file": "steps/s.ts", "line": 1, "index": 0, "nested_in": null, "slot": null, "conditional": false,
            "depends_on": [], "after": [], "refs": [], "effect": { "access": access, "target": "t", "external": true }, "literals": literals, "goto_targets": [], "nested": [] })).unwrap()
    }

    fn connector(v: Value) -> ResolvedConnector {
        let mut base = json!({ "name": "c", "kind": "mcp", "base_url": null, "auth_env": null, "task_queue": null, "mock": null });
        for (k, x) in v.as_object().unwrap() { base[k] = x.clone(); }
        serde_json::from_value(base).unwrap()
    }

    #[test]
    fn a_step_that_starts_a_program_never_runs_on_the_shared_workers() {
        assert_eq!(pool_for("cli", None, false), Some("sandbox".into()));
        assert_eq!(pool_for("cli", Some(&connector(json!({ "kind": "cli", "worker": "shared" }))), false), Some("sandbox".into()), "a shared pin is ignored");
        assert_eq!(pool_for("cli", Some(&connector(json!({ "kind": "cli", "worker": "onprem" }))), false), Some("onprem".into()), "the registry's pool stands");
        assert_eq!(pool_for("mcp", Some(&connector(json!({ "command": ["deno", "run", "s.ts"] }))), false), Some("sandbox".into()), "an MCP server over stdio");
        assert_eq!(pool_for("mcp", Some(&connector(json!({ "url": "https://h.example/mcp", "command": ["sh"] }))), false), None, "an MCP server over HTTP");
        assert_eq!(pool_for("http", Some(&connector(json!({ "kind": "http" }))), false), None);
        assert_eq!(pool_for("http", Some(&connector(json!({ "kind": "http", "worker": "onprem" }))), false), Some("onprem".into()));
    }

    #[test]
    fn with_sandboxes_a_program_runs_from_any_worker_of_the_platform() {
        assert_eq!(pool_for("cli", None, true), None, "the shared workers send it to a sandbox");
        assert_eq!(pool_for("cli", Some(&connector(json!({ "kind": "cli", "worker": "sandbox" }))), true), None, "a pin to the old sandbox pool is moot");
        assert_eq!(pool_for("mcp", Some(&connector(json!({ "command": ["deno", "run", "s.ts"] }))), true), None);
        assert_eq!(pool_for("cli", Some(&connector(json!({ "kind": "cli", "worker": "onprem" }))), true), Some("onprem".into()), "an organization's own pool stands");
    }

    #[test]
    fn a_child_workflow_id_is_a_folder_name() {
        for ok in ["support_ticket_triage", "bulk-triage", "a1"] { assert!(valid_workflow_id(ok), "{ok}"); }
        for bad in ["", "..", "../other_app/workflows/x", "a/b", "a\\b", "/etc", "x y", &"a".repeat(65)] { assert!(!valid_workflow_id(bad), "{bad}"); }
    }

    #[test]
    fn a_step_is_retried_by_default_when_a_second_attempt_cannot_harm() {
        let lim = |kind: &str, access: &str, lit: Value| step_limits(&node(kind, access, lit), STEP_TIMEOUT_MAX);
        assert_eq!(lim("code", "none", json!({})), (Duration::from_secs(30), 3));
        assert_eq!(lim("llm", "prompt", json!({})), (Duration::from_secs(120), 3));
        assert_eq!((lim("store", "read", json!({})).1, lim("store", "may_write", json!({})).1), (3, 1));
        assert_eq!((lim("mcp", "read", json!({})).1, lim("mcp", "may_write", json!({})).1), (3, 1), "a read-only tool is retried");
        assert_eq!(lim("http", "read", json!({ "method": "GET" })).1, 3);
        assert_eq!(lim("http", "may_write", json!({ "method": "POST" })).1, 1);
        assert_eq!(lim("http", "may_write", json!({ "method": "POST", "idempotent": true })).1, 3);
        assert_eq!(lim("cli", "may_write", json!({})), (Duration::from_secs(60), 1));
        // the step's own options win, within bounds
        assert_eq!(lim("code", "none", json!({ "timeout_ms": 5000, "retries": { "max": 7 } })), (Duration::from_millis(5000), 7));
        assert_eq!(lim("code", "none", json!({ "timeout_ms": 0, "retries": { "max": 50 } })), (Duration::from_millis(1), 10));
        assert_eq!(lim("code", "none", json!({ "timeout_ms": 86_400_000 })).0, STEP_TIMEOUT_MAX);
        // a native activity runs on its team's worker: its own timeout, however long
        assert_eq!(step_limits(&node("activity", "may_write", json!({ "timeout_ms": 86_400_000 })), Duration::MAX), (Duration::from_secs(86_400), 3));
    }

    #[test]
    fn a_native_activity_follows_its_declaration() {
        let n = node("activity", "may_write", json!({ "timeout_ms": 90_000, "heartbeat_ms": 20_000, "retries": { "max": 5 }, "non_retryable": ["InvoiceNotFound"] }));
        let o = native_opts(&n, "refund", "IssueRefund@2 on q".into(), "q");
        let r = o.retry_policy.clone().unwrap();
        assert_eq!((r.maximum_attempts(), r.non_retryable_error_types()), (5, &["Forbidden".to_string(), "InvalidInputError".to_string(), "InvoiceNotFound".to_string()][..]));
        assert_eq!(o.heartbeat_timeout, Some(Duration::from_secs(20)));
        assert_eq!(o.task_queue.as_deref(), Some("q"));
        assert!(native_opts(&node("activity", "may_write", json!({})), "x", String::new(), "q").heartbeat_timeout.is_none(), "no declared heartbeat, none asked");
    }

    /// The envelope's golden form is what the SDKs' checks decode (sdks/go `go test`, sdks/dotnet Cori.Activities.Check).
    #[test]
    fn the_native_envelope_is_the_one_the_sdks_decode() {
        let golden: Value = serde_json::from_str(&std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../sdks/testdata/native_envelope.json")).unwrap()).unwrap();
        let input = json!({ "invoice_id": "INV-42", "amount_eur": 15.5, "reason": "charged twice" });
        let user = json!({ "sub": "alice", "roles": ["support_agent"], "email": "alice@acme.example", "platform_roles": [] });
        // the run's token sealed for the connector's worker (sdks/testdata/connector_key.hex opens it)
        let token = json!("cori-sealed.v1.oKGio6Slpqeoqaqrg2E2RSeMYdYtDM2AUgCJ7z7FEym80nFcsm0U6hE_QT5YC51eIOYdnM5S-JWf");
        let envelope = NativeEnvelope { input: &input, cori: NativeContext {
            run_id: "refund_request-0001", workflow: "refund_request", version: "sha256:3f9a0b1c2d3e4f5061728394a5b6c7d8e9f00112233445566778899aabbccdd",
            step: "refund", attempt: 1, user: &user, principal_token: &token, idempotency_key: "refund_request-0001:refund".into(),
        } };
        assert_eq!(serde_json::to_value(&envelope).unwrap(), golden);
    }

    #[test]
    fn old_durations_read_as_before_without_panicking() {
        assert_eq!(super::legacy_duration("30"), Duration::from_secs(30 * 60), "a bare number meant minutes");
        assert_eq!(super::legacy_duration("2h"), Duration::from_secs(7200));
        assert_eq!(super::legacy_duration(" 30 "), Duration::from_secs(30 * 60), "padding once made the split panic");
        assert_eq!(super::legacy_duration("99999999999999999999d"), Duration::from_secs(0));
    }

    #[test]
    fn a_goto_skips_the_compilers_list_else_the_schedule_range() {
        let mut dag: cori_compiler::dag::Dag = serde_json::from_value(json!({ "nodes": [], "edges": [], "predecessors": {},
            "schedule": [["check"], ["gate"], ["done", "send_offer"], ["after"]] })).unwrap();
        // a DAG compiled before the skip lists: the steps between the goto and its target in schedule order
        assert_eq!(goto_skipped(&dag, "gate", "send_offer"), vec!["done".to_string()]);
        assert_eq!(goto_skipped(&dag, "gate", "end"), vec!["done".to_string(), "send_offer".to_string(), "after".to_string()]);
        assert!(goto_skipped(&dag, "send_offer", "check").is_empty(), "a backward range skips nothing (it used to panic)");
        dag.goto_skips.insert("gate".into(), [("done".to_string(), vec!["send_offer".to_string()])].into_iter().collect());
        assert_eq!(goto_skipped(&dag, "gate", "done"), vec!["send_offer".to_string()]);
    }
    /// Runs stored before `error_detail` existed still read back.
    #[test]
    fn stored_output_without_error_detail_deserializes() {
        let o: WorkflowOutput = serde_json::from_str(r#"{"run_id":"r","status":"failed","error":"save: Activity failed: x","steps":{},"trace":[],"result":null}"#).unwrap();
        assert!(o.error_detail.is_none());
    }
    #[test]
    fn error_detail_splits_type_prefix() {
        assert_eq!(ErrorDetail::parse(Some("save".into()), "StepFailedError: Todo not found: 42"), ErrorDetail { step: Some("save".into()), ty: Some("StepFailedError".into()), message: "Todo not found: 42".into() });
        assert_eq!(ErrorDetail::parse(None, "human step `ok` timed out").ty, None);
        assert_eq!(ErrorDetail::parse(None, "child workflow `x` failed: boom").message, "child workflow `x` failed: boom");
    }
}
