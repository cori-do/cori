//! The single generic Cori workflow.
//!
//! ⚠️ DETERMINISM RULES — replays will desync if you violate any of these:
//!
//! - Do NOT call `std::time::Instant::now`, `chrono::Utc::now`, or any
//!   wall-clock function inside this file. Use `ctx.workflow_time()`.
//! - Do NOT call `rand` / `tokio::time::sleep` / `tokio::spawn`. Use
//!   `ctx.timer(...)` and the SDK's child-future spawning.
//! - Do NOT touch the filesystem, network, or SQLite. All I/O happens
//!   inside activities.
//! - The workflow body may be replayed from event history. Any
//!   non-deterministic operation here will fail the replay.
//!
//! Architecture: a single `CoriWorkflow` handles every compiled
//! DAG. The DAG is passed in as part of [`WorkflowInput`] (locked
//! decision — see `temporal-implementation-startegy.md` §C.1), so the
//! workflow never reads from SQLite.
//!
//! Builtin control flow (`branch` / `switch` / `for_each` / `loop` /
//! `wait`) runs as workflow code, not as its own activity kind. The
//! parts that must evaluate user TypeScript — a branch's `if`, a
//! switch's `on`, a for_each's `over`, a loop's `until` — are dispatched
//! through the pure `cori_code` activity in `builtin_eval` mode, so
//! every evaluation is recorded in Temporal history and deterministic
//! on replay. A builtin's nested steps (`then` / `cases.<label>` /
//! `apply` / `body`) dispatch as ordinary activities of their own kind,
//! addressed by a `nested_slot` selector inside the builtin's file and
//! routed on the per-slot task queue the planner resolved. `wait` uses
//! only `ctx.timer` and signals — it never dispatches. `map` and
//! `parallel` remain deferred and are skipped with a notice.

use std::collections::BTreeSet;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map as JsonMap, Value as JsonValue, json};
use temporalio_common::error::{ActivityExecutionError, IncomingError};
use temporalio_macros::{workflow, workflow_methods};
use temporalio_sdk::{ActivityOptions, SyncWorkflowContext, WorkflowContext, WorkflowResult};

use cori_protocol::{
    CompiledStep, CompiledWorkflow, DEFAULT_FOR_EACH_ITEMS, DEFAULT_LOOP_ITERATIONS,
    MAX_WAIT_TIMEOUT_MS, SourceBundle, StepKind, bounded_activity_attempts,
    bounded_activity_timeout_ms, parse_wait_until,
};

use crate::activities::{
    ActivityInput, ActivityOutput, CoriActivities, FrozenStep, NeedsReauthDetails,
};

/// Default mid-run re-auth timeout when [`WorkflowInput::reauth_timeout_secs`]
/// is not set. Matches the redesign-migration-plan §Phase 6 default.
const DEFAULT_REAUTH_TIMEOUT_SECS: u64 = 15 * 60;

/// Input to [`CoriWorkflow`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowInput {
    /// Cori workflow id (the user-facing slug — not a Temporal type name).
    pub workflow_id: String,
    /// Content hash of the complete workflow tree, including imported helper
    /// modules. The workflow body deterministically copies it into activity
    /// inputs so workers can reject source drift before dispatch. Optional
    /// because the smoke test constructs an in-memory DAG without a source
    /// folder.
    #[serde(default)]
    pub workflow_content_hash: Option<String>,
    /// Identity of the user requesting this run. The broker uses this
    /// to scope credential lookups; Phase 4 will also use it to derive
    /// per-step task queues. Defaults to the empty string for tests
    /// that don't go through the CLI.
    #[serde(default)]
    pub user_id: String,
    /// The full compiled DAG. Whole-bytes determinism: re-passed into
    /// every replay via Temporal event history.
    pub compiled_dag: CompiledWorkflow,
    /// Initial input object: manifest parameter defaults overlaid with
    /// user-supplied `key=value` CLI args.
    pub user_params: JsonValue,
    /// When true, real-side-effect steps return mocked outputs.
    pub dry_run: bool,
    /// Override for the mid-run re-auth wait timeout. Defaults to
    /// `DEFAULT_REAUTH_TIMEOUT_SECS` (15 min) when unset. Tests use a
    /// short value to exercise the timeout path quickly.
    #[serde(default)]
    pub reauth_timeout_secs: Option<u64>,
    /// Absolute filesystem path of the workflow's folder on the
    /// triggering machine. Carried into every `ActivityInput` so the
    /// activity resolves step files against the **workflow's** root,
    /// not the worker process's startup `cwd`. Empty string for tests
    /// that build in-memory DAGs.
    #[serde(default)]
    pub source_root: String,
    /// Capped immutable source transport for every activity-bearing run.
    /// Repeated into each external activity because Temporal task queues have
    /// no host affinity, including when multiple workers poll the same queue.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_bundle: Option<SourceBundle>,
    /// Machine-owned AI provider selection and level mappings frozen when
    /// the run starts. Activities use this snapshot even if Console settings
    /// change while the workflow is waiting or retrying.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub llm_config: Option<cori_broker::llm::LlmConfig>,
}

/// Output of [`CoriWorkflow`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowOutput {
    /// Cori run id (== Temporal workflow execution id).
    pub run_id: String,
    /// `"succeeded"` | `"failed"`.
    pub status: String,
    /// The last step's output (or `Null` if everything was skipped /
    /// failed).
    pub final_output: JsonValue,
    /// One entry per dispatched step, in execution order.
    pub activities: Vec<ActivitySummary>,
    /// Error message when `status == "failed"`.
    pub error: Option<String>,
}

/// What the workflow collected about one step's execution. The CLI
/// promotes this into the user-facing trace row. Builtin steps produce
/// exactly one summary regardless of how many nested activities they
/// dispatched — the nested outcomes are folded into `output`, `notes`,
/// `cost_eur`, and `usage`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivitySummary {
    pub activity_id: String,
    pub step_name: String,
    pub kind: StepKind,
    pub status: String,
    pub started_at: Option<DateTime<Utc>>,
    pub ended_at: Option<DateTime<Utc>>,
    pub duration_ms: u64,
    pub attempts: u32,
    pub route: Option<String>,
    pub input: JsonValue,
    pub output: JsonValue,
    pub cost_eur: Option<f64>,
    pub usage: Option<cori_broker::TokenUsage>,
    pub error: Option<String>,
    pub notes: Vec<String>,
}

/// Signal payload for [`CoriWorkflow::reauth_completed`].
///
/// `cori login <capability>` sends this signal to every open workflow
/// owned by the same user after a successful sign-in. The workflow
/// records the `server_id` in its `completed_reauths` set; any
/// suspended step waiting on that capability wakes up and retries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReauthSignalArgs {
    /// Stable id of the capability the user just signed in to —
    /// matches the `server_id` carried by `BrokerError::NeedsReauth`.
    pub server_id: String,
}

/// Signal payload for [`CoriWorkflow::event_received`].
///
/// A `wait` builtin with `for: { signal: "<name>" }` suspends until an
/// event with that name arrives (or its deadline elapses). Events are
/// delivered by name so one workflow can hold several distinct waits.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventSignalArgs {
    /// The event name a suspended `wait` step is listening for.
    pub name: String,
}

/// The single generic workflow type Cori registers with Temporal.
#[workflow]
#[derive(Default)]
pub struct CoriWorkflow {
    /// Set of capability `server_id`s for which a `reauth_completed`
    /// signal has been received and not yet consumed by a retry.
    completed_reauths: BTreeSet<String>,
    /// Event names delivered by `event_received` and not yet consumed
    /// by a suspended `wait` step.
    received_events: BTreeSet<String>,
}

#[workflow_methods]
impl CoriWorkflow {
    #[run]
    pub async fn run(
        ctx: &mut WorkflowContext<Self>,
        input: WorkflowInput,
    ) -> WorkflowResult<WorkflowOutput> {
        let run_id = ctx.workflow_id().to_string();

        // Accumulator: spread of initial params + each successful step's
        // object output. Each step receives this as its input.
        let mut accumulated: JsonMap<String, JsonValue> = match input.user_params.clone() {
            JsonValue::Object(m) => m,
            _ => JsonMap::new(),
        };
        let mut activities: Vec<ActivitySummary> =
            Vec::with_capacity(input.compiled_dag.steps.len());
        let mut last_output: JsonValue = JsonValue::Null;
        let mut run_status: &'static str = "succeeded";
        let mut run_error: Option<String> = None;

        let reauth_timeout = Duration::from_secs(
            input
                .reauth_timeout_secs
                .unwrap_or(DEFAULT_REAUTH_TIMEOUT_SECS),
        );

        let steps = &input.compiled_dag.steps;
        let mut step_idx: usize = 0;
        while step_idx < steps.len() {
            let step = &steps[step_idx];
            let step_input = JsonValue::Object(accumulated.clone());

            let (summary, jump) = if matches!(step.kind, StepKind::Builtin) {
                run_builtin_step(
                    ctx,
                    &input,
                    step,
                    step_input,
                    &run_id,
                    &accumulated,
                    reauth_timeout,
                )
                .await
            } else {
                let activity_in =
                    activity_input_for_step(&input, step, step_input.clone(), &run_id);
                let opts_spec = opts_spec_for_step(step);
                let mut attempts: u32 = 0;
                let summary = match dispatch_activity(
                    ctx,
                    step.kind,
                    &activity_in,
                    &opts_spec,
                    reauth_timeout,
                    &mut attempts,
                )
                .await
                {
                    Ok(out) => summary_from_output(step, step_input, out, attempts),
                    Err(failure) => failed_summary(step, step_input, failure, attempts),
                };
                (summary, None)
            };

            let failed = summary.status == "failed";
            if contributes_to_dataflow(&summary.status, input.dry_run) {
                if let JsonValue::Object(m) = &summary.output {
                    for (k, v) in m {
                        accumulated.insert(k.clone(), v.clone());
                    }
                }
                last_output = summary.output.clone();
            }
            if failed {
                run_status = "failed";
                run_error = summary.error.clone();
            }
            let router_id = summary.activity_id.clone();
            activities.push(summary);
            if failed {
                break;
            }

            // Routing: a branch/switch path that chose `goto` jumps
            // forward. Steps between here and the target were decided
            // against — record each with an honest `not_taken` row so
            // the trace keeps one row per step in plan order.
            let next_idx = match jump {
                Some(target) => resolve_jump_index(steps, step_idx, &target),
                None => step_idx + 1,
            };
            for skipped in steps
                .iter()
                .take(next_idx.min(steps.len()))
                .skip(step_idx + 1)
            {
                activities.push(not_taken_summary(skipped, &router_id));
            }
            step_idx = next_idx;
        }

        Ok(WorkflowOutput {
            run_id,
            status: run_status.to_string(),
            final_output: last_output,
            activities,
            error: run_error,
        })
    }

    /// Signal handler invoked by `cori login <capability>` after a
    /// successful sign-in. Records the capability so any suspended
    /// step waiting on it wakes up. Idempotent: receiving the signal
    /// multiple times has the same effect as once.
    #[signal]
    pub fn reauth_completed(
        &mut self,
        _ctx: &mut SyncWorkflowContext<Self>,
        args: ReauthSignalArgs,
    ) {
        self.completed_reauths.insert(args.server_id);
    }

    /// Signal handler for external events awaited by `wait` builtins
    /// (`for: { signal: "<name>" }`). Idempotent per name; a suspended
    /// wait consumes its name when it resumes.
    #[signal]
    pub fn event_received(&mut self, _ctx: &mut SyncWorkflowContext<Self>, args: EventSignalArgs) {
        self.received_events.insert(args.name);
    }
}

fn contributes_to_dataflow(status: &str, dry_run: bool) -> bool {
    status == "ok" || (dry_run && status == "skipped")
}

/// The failure shape shared by regular and nested dispatch: a
/// human-readable error plus any trace notes (e.g. the re-auth hint).
struct DispatchFailure {
    error: String,
    notes: Vec<String>,
}

/// Deterministic "now" for trace timestamps: Temporal's workflow time,
/// which is recorded in history and stable across replays.
fn wf_now(ctx: &WorkflowContext<CoriWorkflow>) -> Option<DateTime<Utc>> {
    ctx.workflow_time().map(DateTime::<Utc>::from)
}

// ---------------------------------------------------------------------------
// Activity dispatch (shared by top-level steps and builtin internals)
// ---------------------------------------------------------------------------

/// Everything needed to (re)build one dispatch's `ActivityOptions`.
/// Rebuilt per attempt because `ActivityOptions` is consumed on start.
struct OptsSpec {
    activity_id: String,
    summary: String,
    timeout: Duration,
    max_attempts: i32,
    backoff_coefficient: f64,
    task_queue: Option<String>,
}

impl OptsSpec {
    fn build(&self) -> ActivityOptions {
        let retry_policy = temporalio_common::protos::temporal::api::common::v1::RetryPolicy {
            initial_interval: Some(prost_duration_from_secs(1)),
            backoff_coefficient: self.backoff_coefficient,
            maximum_interval: Some(prost_duration_from_secs(60)),
            maximum_attempts: self.max_attempts,
            non_retryable_error_types: vec![
                "MissingCapabilityError".to_string(),
                "AuthenticationError".to_string(),
                "InvalidInputError".to_string(),
                "SchemaValidationError".to_string(),
                "StepFailedError".to_string(),
                "SourceBundleError".to_string(),
                "RuntimeUnavailableError".to_string(),
                "MissingEnvelopeError".to_string(),
            ],
        };
        // 30s schedule_to_start surfaces missing-worker fast with an
        // actionable error rather than blocking the workflow.
        ActivityOptions::with_start_to_close_timeout(self.timeout)
            // Preserve Cori's stable step id in Temporal history. Besides
            // making the history inspectable, this lets the initiating
            // Console map live activity events back onto its already-
            // rendered step rows.
            .activity_id(self.activity_id.clone())
            .summary(self.summary.clone())
            .retry_policy(retry_policy)
            .maybe_task_queue(self.task_queue.clone())
            .schedule_to_start_timeout(Duration::from_secs(30))
            .build()
    }
}

/// Build the `OptsSpec` for one dispatch from a step kind + metadata.
///
/// Recognised metadata keys:
/// - `timeout_ms` (number): overrides `start_to_close_timeout`.
/// - `retries.max` (number): overrides the default attempt cap.
/// - `retries.backoff` (`"exponential"` | `"linear"`): retry backoff
///   strategy. Defaults to exponential.
fn opts_spec_for(
    kind: StepKind,
    metadata: &JsonMap<String, JsonValue>,
    activity_id: String,
    summary: String,
    task_queue: Option<String>,
) -> OptsSpec {
    let default_secs: u64 = match kind {
        StepKind::Cli => 60,
        StepKind::McpTool => 30,
        StepKind::Code => 30,
        StepKind::Llm => 120,
        StepKind::Builtin => 30,
    };
    let timeout = Duration::from_millis(bounded_activity_timeout_ms(
        metadata.get("timeout_ms").and_then(|v| v.as_u64()),
        default_secs * 1000,
    ));

    // Default: cli/mcp_tool mutate external state, so we only attempt
    // once unless the step explicitly opts in. Pure (code, builtin
    // evaluation) and idempotent-ish (llm) kinds get a small retry
    // budget.
    let default_attempts: i32 = match kind {
        StepKind::Cli | StepKind::McpTool => 1,
        StepKind::Code | StepKind::Llm => 3,
        StepKind::Builtin => 3,
    };
    let retries = metadata.get("retries");
    let configured_attempts = retries.and_then(|r| r.get("max")).and_then(|v| v.as_u64());
    let max_attempts = i32::try_from(bounded_activity_attempts(
        configured_attempts,
        u32::try_from(default_attempts).unwrap_or(1),
    ))
    .unwrap_or(default_attempts);

    // Backoff strategy mirrors the SDK's `retries.backoff` field. Linear
    // backoff keeps a constant interval (coefficient 1.0); exponential
    // (the default) doubles each attempt.
    let backoff_coefficient = match retries
        .and_then(|r| r.get("backoff"))
        .and_then(|v| v.as_str())
    {
        Some("linear") => 1.0,
        _ => 2.0,
    };

    OptsSpec {
        activity_id,
        summary,
        timeout,
        max_attempts,
        backoff_coefficient,
        task_queue,
    }
}

fn opts_spec_for_step(step: &CompiledStep) -> OptsSpec {
    opts_spec_for(
        step.kind,
        &step.metadata,
        step.activity_id.clone(),
        step.name.clone(),
        step.task_queue.clone(),
    )
}

/// Base `ActivityInput` for a top-level step.
fn activity_input_for_step(
    input: &WorkflowInput,
    step: &CompiledStep,
    step_input: JsonValue,
    run_id: &str,
) -> ActivityInput {
    ActivityInput {
        step_id: step.activity_id.clone(),
        step_name: step.name.clone(),
        step_kind: step.kind,
        source_path: std::path::PathBuf::from(&step.source_path),
        route: step.route.clone(),
        input: step_input,
        workflow_id: input.workflow_id.clone(),
        run_id: run_id.to_string(),
        user_id: input.user_id.clone(),
        dry_run: input.dry_run,
        source_root: input.source_root.clone(),
        source_bundle: input.source_bundle.clone(),
        llm_config: input.llm_config.clone(),
        frozen_step: step.source_sha256.as_ref().map(|source_sha256| FrozenStep {
            source_sha256: source_sha256.clone(),
            workflow_content_hash: input.workflow_content_hash.clone(),
            metadata: step.metadata.clone(),
        }),
        nested_slot: None,
        builtin_eval: None,
    }
}

/// Dispatch one activity, suspending on `NeedsReauth` until the matching
/// `reauth_completed` signal arrives (retrying the same activity) or the
/// re-auth timeout elapses (failing the dispatch). `attempts` counts every
/// dispatch attempt including reauth retries.
async fn dispatch_activity(
    ctx: &mut WorkflowContext<CoriWorkflow>,
    kind: StepKind,
    activity_in: &ActivityInput,
    opts_spec: &OptsSpec,
    reauth_timeout: Duration,
    attempts: &mut u32,
) -> Result<ActivityOutput, DispatchFailure> {
    loop {
        *attempts = attempts.saturating_add(1);
        let opts = opts_spec.build();
        let result: Result<ActivityOutput, _> = match kind {
            // Builtin selector evaluations ride the pure `cori_code`
            // activity — the four activity kinds stay a closed set.
            StepKind::Code | StepKind::Builtin => {
                ctx.start_activity(CoriActivities::cori_code, activity_in.clone(), opts)
                    .await
            }
            StepKind::Cli => {
                ctx.start_activity(CoriActivities::cori_cli, activity_in.clone(), opts)
                    .await
            }
            StepKind::McpTool => {
                ctx.start_activity(CoriActivities::cori_mcp_tool, activity_in.clone(), opts)
                    .await
            }
            StepKind::Llm => {
                ctx.start_activity(CoriActivities::cori_llm, activity_in.clone(), opts)
                    .await
            }
        };

        match result {
            Ok(out) => return Ok(out),
            Err(e) => {
                // Phase 6: when an activity surfaces a `NeedsReauth`
                // application failure, suspend the workflow until
                // either (a) a `reauth_completed` signal arrives for
                // the matching capability, in which case we retry
                // the same dispatch, or (b) `reauth_timeout` elapses,
                // in which case we fail cleanly.
                if let Some(details) = needs_reauth_details(&e) {
                    let server_id = details.server_id.clone();
                    let server_for_wait = server_id.clone();
                    let mut signal_arrived = false;
                    temporalio_sdk::workflows::select! {
                        _ = ctx.timer(reauth_timeout) => {}
                        _ = ctx.wait_condition(move |s: &CoriWorkflow| {
                            s.completed_reauths.contains(&server_for_wait)
                        }) => {
                            signal_arrived = true;
                        }
                    }
                    if signal_arrived {
                        // Consume the marker so a future failure on
                        // the same capability waits afresh.
                        ctx.state_mut(|s| {
                            s.completed_reauths.remove(&server_id);
                        });
                        continue;
                    }
                    // Timed out — fail the dispatch.
                    return Err(DispatchFailure {
                        error: format!(
                            "timed out after {}s waiting for `cori login {}` (capability: {})",
                            reauth_timeout.as_secs(),
                            details.server_id,
                            details.server_id,
                        ),
                        notes: vec![format!(
                            "needs sign-in for `{}` — {}",
                            details.server_id, details.hint
                        )],
                    });
                }

                return Err(DispatchFailure {
                    error: describe_activity_error(&e),
                    notes: Vec::new(),
                });
            }
        }
    }
}

fn summary_from_output(
    step: &CompiledStep,
    step_input: JsonValue,
    out: ActivityOutput,
    attempts: u32,
) -> ActivitySummary {
    ActivitySummary {
        activity_id: step.activity_id.clone(),
        step_name: step.name.clone(),
        kind: step.kind,
        status: out.status,
        started_at: out.started_at,
        ended_at: out.ended_at,
        duration_ms: out.duration_ms,
        attempts,
        route: step.route.clone(),
        input: step_input,
        output: out.output,
        cost_eur: out.cost_eur,
        usage: out.usage,
        error: None,
        notes: out.notes,
    }
}

fn failed_summary(
    step: &CompiledStep,
    step_input: JsonValue,
    failure: DispatchFailure,
    attempts: u32,
) -> ActivitySummary {
    ActivitySummary {
        activity_id: step.activity_id.clone(),
        step_name: step.name.clone(),
        kind: step.kind,
        status: "failed".to_string(),
        started_at: None,
        ended_at: None,
        duration_ms: 0,
        attempts,
        route: step.route.clone(),
        input: step_input,
        output: JsonValue::Null,
        cost_eur: None,
        usage: None,
        error: Some(failure.error),
        notes: failure.notes,
    }
}

// ---------------------------------------------------------------------------
// Builtin control flow
// ---------------------------------------------------------------------------

/// Execute one builtin step and produce its single trace summary.
///
/// Every path through here is deterministic: timers via `ctx.timer`,
/// external events via recorded signals, and user-TS evaluation via
/// recorded `cori_code` activities.
async fn run_builtin_step(
    ctx: &mut WorkflowContext<CoriWorkflow>,
    input: &WorkflowInput,
    step: &CompiledStep,
    step_input: JsonValue,
    run_id: &str,
    accumulated: &JsonMap<String, JsonValue>,
    reauth_timeout: Duration,
) -> (ActivitySummary, Option<String>) {
    let started_at = wf_now(ctx);
    let sub_kind = step
        .metadata
        .get("builtin")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let mut tracker = BuiltinTracker::new(step, step_input.clone(), started_at);

    let result = match sub_kind.as_str() {
        "wait" => run_wait(ctx, input, step, &mut tracker).await,
        "branch" | "switch" => {
            run_branch_or_switch(
                ctx,
                input,
                step,
                &sub_kind,
                step_input.clone(),
                run_id,
                reauth_timeout,
                &mut tracker,
            )
            .await
        }
        "for_each" => {
            run_for_each(
                ctx,
                input,
                step,
                step_input.clone(),
                run_id,
                accumulated,
                reauth_timeout,
                &mut tracker,
            )
            .await
        }
        "loop" => {
            run_loop(
                ctx,
                input,
                step,
                step_input.clone(),
                run_id,
                accumulated,
                reauth_timeout,
                &mut tracker,
            )
            .await
        }
        // `map` / `parallel` (and unknown sub-kinds from newer
        // compilers): preserved trace shape, skipped with a notice.
        other => {
            let label = if other.is_empty() { "builtin" } else { other };
            tracker
                .notes
                .push(format!("builtin `{label}` is not implemented — skipping"));
            Ok(BuiltinResult::skipped())
        }
    };

    let jump = result.as_ref().ok().and_then(|result| result.jump.clone());
    (tracker.finish(ctx, result), jump)
}

/// What a builtin execution produced, before it is folded into the
/// step's single `ActivitySummary`.
struct BuiltinResult {
    status: &'static str,
    output: JsonValue,
    /// Forward routing decision: the target step's activity id, or
    /// `"end"` to finish the run after this step.
    jump: Option<String>,
}

impl BuiltinResult {
    fn ok(output: JsonValue) -> Self {
        Self {
            status: "ok",
            output,
            jump: None,
        }
    }
    fn ok_with_jump(output: JsonValue, target: String) -> Self {
        Self {
            status: "ok",
            output,
            jump: Some(target),
        }
    }
    fn skipped() -> Self {
        Self {
            status: "skipped",
            output: JsonValue::Null,
            jump: None,
        }
    }
    fn skipped_with(output: JsonValue) -> Self {
        Self {
            status: "skipped",
            output,
            jump: None,
        }
    }
}

/// Accumulates trace facts (attempt counts, notes, cost, usage) across a
/// builtin's evaluator and nested dispatches.
struct BuiltinTracker {
    activity_id: String,
    step_name: String,
    route: Option<String>,
    input: JsonValue,
    started_at: Option<DateTime<Utc>>,
    attempts: u32,
    notes: Vec<String>,
    cost_eur: Option<f64>,
    usage: Option<cori_broker::TokenUsage>,
}

impl BuiltinTracker {
    fn new(step: &CompiledStep, input: JsonValue, started_at: Option<DateTime<Utc>>) -> Self {
        Self {
            activity_id: step.activity_id.clone(),
            step_name: step.name.clone(),
            route: step.route.clone(),
            input,
            started_at,
            attempts: 0,
            notes: Vec::new(),
            cost_eur: None,
            usage: None,
        }
    }

    /// Fold one nested dispatch's cost/usage/notes into the builtin trace.
    fn absorb(&mut self, out: &ActivityOutput) {
        if let Some(cost) = out.cost_eur {
            self.cost_eur = Some(self.cost_eur.unwrap_or(0.0) + cost);
        }
        if let Some(usage) = out.usage {
            self.usage = Some(match self.usage {
                Some(total) => total + usage,
                None => usage,
            });
        }
        self.notes.extend(out.notes.iter().cloned());
    }

    fn finish(
        self,
        ctx: &WorkflowContext<CoriWorkflow>,
        result: Result<BuiltinResult, DispatchFailure>,
    ) -> ActivitySummary {
        let ended_at = wf_now(ctx);
        let duration_ms = match (self.started_at, ended_at) {
            (Some(start), Some(end)) => {
                u64::try_from((end - start).num_milliseconds().max(0)).unwrap_or(u64::MAX)
            }
            _ => 0,
        };
        let mut notes = self.notes;
        let (status, output, error) = match result {
            Ok(result) => (result.status.to_string(), result.output, None),
            Err(failure) => {
                notes.extend(failure.notes);
                ("failed".to_string(), JsonValue::Null, Some(failure.error))
            }
        };
        ActivitySummary {
            activity_id: self.activity_id,
            step_name: self.step_name,
            kind: StepKind::Builtin,
            status,
            started_at: self.started_at,
            ended_at,
            duration_ms,
            attempts: self.attempts,
            route: self.route,
            input: self.input,
            output,
            cost_eur: self.cost_eur,
            usage: self.usage,
            error,
            notes,
        }
    }
}

/// `wait`: pause until a delay elapses, an absolute time is reached, or
/// an external event arrives. Pure workflow code — no activities.
async fn run_wait(
    ctx: &mut WorkflowContext<CoriWorkflow>,
    input: &WorkflowInput,
    step: &CompiledStep,
    tracker: &mut BuiltinTracker,
) -> Result<BuiltinResult, DispatchFailure> {
    let spec = step
        .metadata
        .get("wait")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();
    let timeout_ms = spec
        .get("timeout_ms")
        .and_then(|v| v.as_u64())
        .filter(|ms| (1..=MAX_WAIT_TIMEOUT_MS).contains(ms));
    let until = spec
        .get("until")
        .and_then(|v| v.as_str())
        .and_then(parse_wait_until);
    let signal = spec
        .get("signal")
        .and_then(|v| v.as_str())
        .map(str::to_string);

    if timeout_ms.is_none() && until.is_none() && signal.is_none() {
        return Err(DispatchFailure {
            error:
                "wait step has no usable `for` spec (expected `timeout_ms`, `until`, or `signal`)"
                    .to_string(),
            notes: Vec::new(),
        });
    }

    // The effective deadline is the earliest of the relative delay and
    // the absolute timestamp. An `until` already in the past is a
    // zero-length wait, not an error — re-runs of dated workflows
    // should proceed, not wedge.
    let mut deadline_ms: Option<u64> = timeout_ms;
    if let Some(until_ts) = until {
        let now = wf_now(ctx);
        let remaining = now
            .map(|now| {
                u64::try_from((until_ts - now).num_milliseconds().max(0)).unwrap_or(u64::MAX)
            })
            .unwrap_or(0)
            .min(MAX_WAIT_TIMEOUT_MS);
        deadline_ms = Some(deadline_ms.map_or(remaining, |t| t.min(remaining)));
    }

    if input.dry_run {
        tracker
            .notes
            .push("wait mocked by --dry-run — not pausing".to_string());
        return Ok(BuiltinResult::skipped_with(json!({ "waited_ms": 0 })));
    }

    match signal {
        Some(name) => {
            let bound_ms = deadline_ms.unwrap_or(MAX_WAIT_TIMEOUT_MS).max(1);
            let name_for_wait = name.clone();
            let mut event_arrived = false;
            temporalio_sdk::workflows::select! {
                _ = ctx.timer(Duration::from_millis(bound_ms)) => {}
                _ = ctx.wait_condition(move |s: &CoriWorkflow| {
                    s.received_events.contains(&name_for_wait)
                }) => {
                    event_arrived = true;
                }
            }
            if event_arrived {
                // Consume the event so a later wait on the same name
                // suspends afresh.
                let name_to_remove = name.clone();
                ctx.state_mut(move |s| {
                    s.received_events.remove(&name_to_remove);
                });
                tracker.notes.push(format!("resumed by event `{name}`"));
                Ok(BuiltinResult::ok(json!({ "event": name })))
            } else {
                Err(DispatchFailure {
                    error: format!(
                        "no `{name}` event arrived within {}s — deliver it with the \
                         `event_received` signal ({{\"name\": \"{name}\"}}) on this run",
                        bound_ms / 1000
                    ),
                    notes: Vec::new(),
                })
            }
        }
        None => {
            let ms = deadline_ms.unwrap_or(0);
            if ms > 0 {
                let _ = ctx.timer(Duration::from_millis(ms)).await;
            }
            tracker.notes.push(format!("paused {}s", ms / 1000));
            Ok(BuiltinResult::ok(json!({ "waited_ms": ms })))
        }
    }
}

/// `branch` (if / else) and `switch`: evaluate the selector, pick the
/// nested slot, dispatch it, and pass its output downstream.
#[allow(clippy::too_many_arguments)]
async fn run_branch_or_switch(
    ctx: &mut WorkflowContext<CoriWorkflow>,
    input: &WorkflowInput,
    step: &CompiledStep,
    sub_kind: &str,
    step_input: JsonValue,
    run_id: &str,
    reauth_timeout: Duration,
    tracker: &mut BuiltinTracker,
) -> Result<BuiltinResult, DispatchFailure> {
    let eval_fn = if sub_kind == "branch" { "if" } else { "on" };
    let value = dispatch_eval(
        ctx,
        input,
        step,
        eval_fn,
        &format!("{}#{eval_fn}", step.activity_id),
        step_input.clone(),
        run_id,
        reauth_timeout,
        tracker,
    )
    .await?;

    let slot: String = if sub_kind == "branch" {
        let condition = value.as_bool().unwrap_or(false);
        if condition {
            "then".to_string()
        } else if nested_slot_meta(step, "else").is_some() {
            "else".to_string()
        } else {
            tracker.notes.push(
                "condition was false and no `else` step is declared — continuing".to_string(),
            );
            return Ok(BuiltinResult::ok(json!({ "condition": false })));
        }
    } else {
        let label = value.as_str().unwrap_or_default().to_string();
        let case_slot = format!("cases.{label}");
        if nested_slot_meta(step, &case_slot).is_some() {
            case_slot
        } else if nested_slot_meta(step, "default").is_some() {
            tracker
                .notes
                .push(format!("label `{label}` has no case — using `default`"));
            "default".to_string()
        } else {
            let declared = declared_case_labels(step).join("`, `");
            return Err(DispatchFailure {
                error: format!(
                    "switch `on` returned `{label}`, which matches no case (declared: `{declared}`) and no `default` step exists"
                ),
                notes: Vec::new(),
            });
        }
    };

    // A routing path (`goto`) dispatches nothing: the decision is the
    // whole work. The compiler resolved the target activity id (or
    // `end`) at compile time; the main loop turns it into the jump.
    if let Some(target) = nested_slot_meta(step, &slot)
        .and_then(|meta| meta.get("goto"))
        .and_then(|v| v.as_str())
    {
        tracker.notes.push(match sub_kind {
            "branch" => format!("took `{slot}` → routed to `{target}`"),
            _ => format!("matched `{slot}` → routed to `{target}`"),
        });
        return Ok(BuiltinResult::ok_with_jump(
            json!({ "routed_to": target }),
            target.to_string(),
        ));
    }

    let out = dispatch_nested(
        ctx,
        input,
        step,
        &slot,
        &format!("{}#{slot}", step.activity_id),
        step_input,
        run_id,
        reauth_timeout,
        tracker,
    )
    .await?;
    tracker.notes.push(match sub_kind {
        "branch" => format!("took `{slot}`"),
        _ => format!("matched `{slot}`"),
    });
    let status_skipped = input.dry_run && out.status == "skipped";
    let output = out.output.clone();
    if status_skipped {
        Ok(BuiltinResult::skipped_with(output))
    } else {
        Ok(BuiltinResult::ok(output))
    }
}

/// Map a resolved jump target onto the index execution continues at.
/// `end` (and, defensively, an unknown id) jumps past the last step.
fn resolve_jump_index(steps: &[CompiledStep], from_idx: usize, target: &str) -> usize {
    if target == "end" {
        return steps.len();
    }
    steps
        .iter()
        .enumerate()
        .skip(from_idx + 1)
        .find(|(_, s)| s.activity_id == target)
        .map(|(j, _)| j)
        .unwrap_or(steps.len())
}

/// Trace row for a step a routing decision jumped past. Zero work, zero
/// attempts — but the row keeps the trace at one entry per step in plan
/// order, which medians, result resolution, and the Console key on.
fn not_taken_summary(step: &CompiledStep, router_id: &str) -> ActivitySummary {
    ActivitySummary {
        activity_id: step.activity_id.clone(),
        step_name: step.name.clone(),
        kind: step.kind,
        status: "not_taken".to_string(),
        started_at: None,
        ended_at: None,
        duration_ms: 0,
        attempts: 0,
        route: step.route.clone(),
        input: JsonValue::Null,
        output: JsonValue::Null,
        cost_eur: None,
        usage: None,
        error: None,
        notes: vec![format!(
            "not on the taken path — `{router_id}` routed past it"
        )],
    }
}

/// `for_each`: evaluate `over`, then apply the nested step to each item
/// sequentially. Item outputs are collected, not folded into each other.
#[allow(clippy::too_many_arguments)]
async fn run_for_each(
    ctx: &mut WorkflowContext<CoriWorkflow>,
    input: &WorkflowInput,
    step: &CompiledStep,
    step_input: JsonValue,
    run_id: &str,
    accumulated: &JsonMap<String, JsonValue>,
    reauth_timeout: Duration,
    tracker: &mut BuiltinTracker,
) -> Result<BuiltinResult, DispatchFailure> {
    let items = dispatch_eval(
        ctx,
        input,
        step,
        "over",
        &format!("{}#over", step.activity_id),
        step_input,
        run_id,
        reauth_timeout,
        tracker,
    )
    .await?;
    let items = items.as_array().cloned().unwrap_or_default();
    let max_items = usize::try_from(
        step.metadata
            .get("max_items")
            .and_then(|v| v.as_u64())
            .unwrap_or(DEFAULT_FOR_EACH_ITEMS),
    )
    .unwrap_or(usize::MAX);
    if items.len() > max_items {
        return Err(DispatchFailure {
            error: format!(
                "`over` returned {} items but `max_items` is {max_items} — raise `max_items` on the step or narrow the list",
                items.len()
            ),
            notes: Vec::new(),
        });
    }

    let mut outputs: Vec<JsonValue> = Vec::with_capacity(items.len());
    let mut any_skipped = false;
    for (index, item) in items.iter().enumerate() {
        let mut per_item = accumulated.clone();
        per_item.insert("item".to_string(), item.clone());
        per_item.insert("item_index".to_string(), json!(index));
        let out = dispatch_nested(
            ctx,
            input,
            step,
            "apply",
            &format!("{}#apply[{index}]", step.activity_id),
            JsonValue::Object(per_item),
            run_id,
            reauth_timeout,
            tracker,
        )
        .await
        .map_err(|failure| DispatchFailure {
            error: format!("item {index}: {}", failure.error),
            notes: failure.notes,
        })?;
        any_skipped = any_skipped || out.status == "skipped";
        outputs.push(out.output);
    }

    tracker
        .notes
        .push(format!("applied to {} item(s)", outputs.len()));
    // Only `items` goes downstream: a generic sibling key like `count`
    // would merge into the accumulator and can silently overwrite a
    // workflow parameter of the same name.
    let output = json!({ "items": outputs });
    if input.dry_run && any_skipped {
        Ok(BuiltinResult::skipped_with(output))
    } else {
        Ok(BuiltinResult::ok(output))
    }
}

/// `loop`: repeat the nested body, merging its output into the working
/// input, until `until` returns true or `max_iterations` is exhausted.
#[allow(clippy::too_many_arguments)]
async fn run_loop(
    ctx: &mut WorkflowContext<CoriWorkflow>,
    input: &WorkflowInput,
    step: &CompiledStep,
    _step_input: JsonValue,
    run_id: &str,
    accumulated: &JsonMap<String, JsonValue>,
    reauth_timeout: Duration,
    tracker: &mut BuiltinTracker,
) -> Result<BuiltinResult, DispatchFailure> {
    let max_iterations = step
        .metadata
        .get("max_iterations")
        .and_then(|v| v.as_u64())
        .unwrap_or(DEFAULT_LOOP_ITERATIONS);

    let mut working = accumulated.clone();
    let mut any_skipped = false;

    for iteration in 1..=max_iterations {
        let mut body_input = working.clone();
        body_input.insert("iteration".to_string(), json!(iteration));
        let out = dispatch_nested(
            ctx,
            input,
            step,
            "body",
            &format!("{}#body[{iteration}]", step.activity_id),
            JsonValue::Object(body_input),
            run_id,
            reauth_timeout,
            tracker,
        )
        .await
        .map_err(|failure| DispatchFailure {
            error: format!("iteration {iteration}: {}", failure.error),
            notes: failure.notes,
        })?;
        any_skipped = any_skipped || out.status == "skipped";
        if let JsonValue::Object(m) = &out.output {
            for (k, v) in m {
                working.insert(k.clone(), v.clone());
            }
        }

        let mut until_input = working.clone();
        until_input.insert("iteration".to_string(), json!(iteration));
        let done = dispatch_eval(
            ctx,
            input,
            step,
            "until",
            &format!("{}#until[{iteration}]", step.activity_id),
            JsonValue::Object(until_input),
            run_id,
            reauth_timeout,
            tracker,
        )
        .await?;
        if done.as_bool().unwrap_or(false) {
            tracker
                .notes
                .push(format!("goal met after {iteration} iteration(s)"));
            let mut output = match out.output {
                JsonValue::Object(m) => m,
                other => {
                    let mut m = JsonMap::new();
                    if !other.is_null() {
                        m.insert("value".to_string(), other);
                    }
                    m
                }
            };
            output.insert("iterations".to_string(), json!(iteration));
            let output = JsonValue::Object(output);
            return if input.dry_run && any_skipped {
                Ok(BuiltinResult::skipped_with(output))
            } else {
                Ok(BuiltinResult::ok(output))
            };
        }
    }

    Err(DispatchFailure {
        error: format!(
            "loop goal not met after {max_iterations} iteration(s) — raise `max_iterations` on the step or fix its `until` condition"
        ),
        notes: Vec::new(),
    })
}

/// Metadata map of one nested slot (`then`, `cases.big`, `apply`, …).
fn nested_slot_meta<'a>(
    step: &'a CompiledStep,
    slot: &str,
) -> Option<&'a JsonMap<String, JsonValue>> {
    step.metadata
        .get("nested")
        .and_then(|v| v.as_object())
        .and_then(|slots| slots.get(slot))
        .and_then(|v| v.as_object())
}

fn declared_case_labels(step: &CompiledStep) -> Vec<String> {
    step.metadata
        .get("nested")
        .and_then(|v| v.as_object())
        .map(|slots| {
            slots
                .keys()
                .filter_map(|slot| slot.strip_prefix("cases."))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Evaluate a builtin selector function (`if` / `on` / `over` / `until`)
/// via the pure `cori_code` activity. Recorded in history — deterministic
/// on replay.
#[allow(clippy::too_many_arguments)]
async fn dispatch_eval(
    ctx: &mut WorkflowContext<CoriWorkflow>,
    input: &WorkflowInput,
    step: &CompiledStep,
    eval_fn: &str,
    activity_id: &str,
    step_input: JsonValue,
    run_id: &str,
    reauth_timeout: Duration,
    tracker: &mut BuiltinTracker,
) -> Result<JsonValue, DispatchFailure> {
    let mut activity_in = activity_input_for_step(input, step, step_input, run_id);
    activity_in.step_id = activity_id.to_string();
    activity_in.builtin_eval = Some(eval_fn.to_string());
    let opts_spec = opts_spec_for(
        StepKind::Builtin,
        &JsonMap::new(),
        activity_id.to_string(),
        format!("{} · {eval_fn}", step.name),
        step.task_queue.clone(),
    );
    let out = dispatch_activity(
        ctx,
        StepKind::Builtin,
        &activity_in,
        &opts_spec,
        reauth_timeout,
        &mut tracker.attempts,
    )
    .await
    .map_err(|failure| DispatchFailure {
        error: format!("evaluating `{eval_fn}`: {}", failure.error),
        notes: failure.notes,
    })?;
    Ok(out.output)
}

/// Dispatch a builtin's nested step (`then`, `cases.<label>`, `apply`,
/// `body`) as an activity of its own kind, routed on the per-slot task
/// queue the planner resolved.
#[allow(clippy::too_many_arguments)]
async fn dispatch_nested(
    ctx: &mut WorkflowContext<CoriWorkflow>,
    input: &WorkflowInput,
    step: &CompiledStep,
    slot: &str,
    activity_id: &str,
    step_input: JsonValue,
    run_id: &str,
    reauth_timeout: Duration,
    tracker: &mut BuiltinTracker,
) -> Result<ActivityOutput, DispatchFailure> {
    let Some(meta) = nested_slot_meta(step, slot) else {
        return Err(DispatchFailure {
            error: format!(
                "compiled metadata for `{}` is missing its `{slot}` nested step — recompile the workflow",
                step.activity_id
            ),
            notes: Vec::new(),
        });
    };
    let kind = match meta.get("kind").and_then(|v| v.as_str()) {
        Some("cli") => StepKind::Cli,
        Some("mcp_tool") => StepKind::McpTool,
        Some("llm") => StepKind::Llm,
        Some("code") => StepKind::Code,
        other => {
            return Err(DispatchFailure {
                error: format!(
                    "nested step `{slot}` has unsupported kind `{}`",
                    other.unwrap_or("missing")
                ),
                notes: Vec::new(),
            });
        }
    };

    let meta = meta.clone();
    let task_queue = meta
        .get("task_queue")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .or_else(|| step.task_queue.clone());

    let mut activity_in = activity_input_for_step(input, step, step_input, run_id);
    activity_in.step_id = activity_id.to_string();
    activity_in.step_kind = kind;
    activity_in.nested_slot = Some(slot.to_string());
    // The frozen boundary for a nested dispatch is the slot's own
    // metadata: the activity re-checks the CLI binary / MCP target /
    // LLM level against what the compiler saw inside this very slot.
    activity_in.frozen_step = step.source_sha256.as_ref().map(|source_sha256| FrozenStep {
        source_sha256: source_sha256.clone(),
        workflow_content_hash: input.workflow_content_hash.clone(),
        metadata: meta.clone(),
    });

    let opts_spec = opts_spec_for(
        kind,
        &meta,
        activity_id.to_string(),
        format!("{} · {slot}", step.name),
        task_queue,
    );
    let out = dispatch_activity(
        ctx,
        kind,
        &activity_in,
        &opts_spec,
        reauth_timeout,
        &mut tracker.attempts,
    )
    .await
    .map_err(|failure| DispatchFailure {
        error: format!("nested step `{slot}`: {}", failure.error),
        notes: failure.notes,
    })?;
    tracker.absorb(&out);
    Ok(out)
}

/// Render an [`ActivityExecutionError`] into a human-readable string by
/// walking the cause chain. The outer `Display` impl only surfaces the
/// generic wrapper message Temporal stamps on every activity failure
/// (typically `"Activity task failed"`); the broker-side error text and
/// `type_name` we packed in [`crate::activities::broker_error_to_activity_error`]
/// live one layer deeper as an [`IncomingError::Application`].
fn describe_activity_error(err: &ActivityExecutionError) -> String {
    let mut parts: Vec<String> = vec![format!("{err}")];
    let mut cur = err.cause();
    while let Some(inner) = cur {
        let prefix = match inner {
            IncomingError::Application(a) => {
                a.type_name().map(|t| format!("[{t}] ")).unwrap_or_default()
            }
            IncomingError::Timeout(_) => "[timeout] ".to_string(),
            IncomingError::Cancelled(_) => "[cancelled] ".to_string(),
            IncomingError::Terminated(_) => "[terminated] ".to_string(),
            IncomingError::Server(_) => "[server] ".to_string(),
            _ => String::new(),
        };
        let message = &inner.failure().message;
        if !message.is_empty() {
            parts.push(format!("{prefix}{message}"));
        }
        cur = inner.cause();
    }
    parts.join(" — ")
}

/// Extract [`NeedsReauthDetails`] from an activity execution error
/// whose underlying [`ApplicationFailure`] was tagged with
/// `type_name = "NeedsReauth"`. Returns `None` for any other failure
/// shape so the dispatch loop falls back to the regular fail path.
fn needs_reauth_details(err: &ActivityExecutionError) -> Option<NeedsReauthDetails> {
    let failed = match err {
        ActivityExecutionError::Failed(f) => f,
        _ => return None,
    };
    let cause = failed.cause()?;
    let app = match cause {
        IncomingError::Application(a) => a,
        _ => return None,
    };
    if app.type_name() != Some("NeedsReauth") {
        return None;
    }
    app.details::<NeedsReauthDetails>().ok().flatten()
}

fn prost_duration_from_secs(s: i64) -> prost_wkt_types::Duration {
    prost_wkt_types::Duration {
        seconds: s,
        nanos: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::contributes_to_dataflow;

    #[test]
    fn dry_run_stubs_continue_schema_dataflow() {
        assert!(contributes_to_dataflow("ok", false));
        assert!(contributes_to_dataflow("ok", true));
        assert!(contributes_to_dataflow("skipped", true));
        assert!(!contributes_to_dataflow("skipped", false));
        assert!(!contributes_to_dataflow("failed", true));
    }
}
