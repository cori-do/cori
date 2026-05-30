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
//! workflow never reads from SQLite. Builtins (`map` / `for_each` /
//! `branch` / `parallel` / `wait`) run as workflow code, not activities.

use std::collections::BTreeSet;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map as JsonMap, Value as JsonValue};
use temporalio_common::error::{ActivityExecutionError, IncomingError};
use temporalio_macros::{workflow, workflow_methods};
use temporalio_sdk::{ActivityOptions, SyncWorkflowContext, WorkflowContext, WorkflowResult};

use cori_protocol::{CompiledWorkflow, StepKind};

use crate::activities::{ActivityInput, ActivityOutput, CoriActivities, NeedsReauthDetails};

/// Default mid-run re-auth timeout when [`WorkflowInput::reauth_timeout_secs`]
/// is not set. Matches the redesign-migration-plan §Phase 6 default.
const DEFAULT_REAUTH_TIMEOUT_SECS: u64 = 15 * 60;

/// Input to [`CoriWorkflow`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowInput {
    /// Cori workflow id (the user-facing slug — not a Temporal type name).
    pub workflow_id: String,
    /// Content hash of the workflow folder (manifest + steps). Carried
    /// for trace / debugging only; the workflow body does not use it.
    /// Optional because the smoke test constructs an in-memory DAG
    /// without a source folder.
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
/// promotes this into the user-facing trace row.
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

/// The single generic workflow type Cori registers with Temporal.
#[workflow]
#[derive(Default)]
pub struct CoriWorkflow {
    /// Set of capability `server_id`s for which a `reauth_completed`
    /// signal has been received and not yet consumed by a retry.
    completed_reauths: BTreeSet<String>,
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

        let mut step_idx: usize = 0;
        let mut attempts_this_step: u32 = 1;
        'outer: while step_idx < input.compiled_dag.steps.len() {
            let step = &input.compiled_dag.steps[step_idx];
            let step_input = JsonValue::Object(accumulated.clone());

            // Builtins are not activities — they run in workflow code.
            // v1 has no real builtin implementation; we emit a "skipped"
            // summary to preserve trace shape, matching the in-process
            // executor's previous behaviour.
            if matches!(step.kind, StepKind::Builtin) {
                activities.push(ActivitySummary {
                    activity_id: step.activity_id.clone(),
                    step_name: step.name.clone(),
                    kind: step.kind,
                    status: "skipped".to_string(),
                    started_at: None,
                    ended_at: None,
                    duration_ms: 0,
                    attempts: 0,
                    route: step.route.clone(),
                    input: step_input,
                    output: JsonValue::Null,
                    cost_eur: None,
                    usage: None,
                    error: None,
                    notes: vec![format!("kind `builtin` is not implemented — skipping")],
                });
                step_idx += 1;
                attempts_this_step = 1;
                continue;
            }

            let activity_in = ActivityInput {
                step_id: step.activity_id.clone(),
                step_name: step.name.clone(),
                step_kind: step.kind,
                source_path: std::path::PathBuf::from(&step.source_path),
                route: step.route.clone(),
                input: step_input.clone(),
                workflow_id: input.workflow_id.clone(),
                run_id: run_id.clone(),
                user_id: input.user_id.clone(),
                dry_run: input.dry_run,
            };

            let opts = activity_options_for_step(step);

            let result: Result<ActivityOutput, _> = match step.kind {
                StepKind::Code => {
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
                StepKind::Builtin => unreachable!("handled above"),
            };

            match result {
                Ok(out) => {
                    let is_ok = out.status == "ok";
                    // Only merge real outputs into the accumulator —
                    // mocked outputs are synthetic and would poison
                    // downstream steps' inputs.
                    if is_ok {
                        if let JsonValue::Object(m) = &out.output {
                            for (k, v) in m {
                                accumulated.insert(k.clone(), v.clone());
                            }
                        }
                        last_output = out.output.clone();
                    }
                    activities.push(ActivitySummary {
                        activity_id: step.activity_id.clone(),
                        step_name: step.name.clone(),
                        kind: step.kind,
                        status: out.status,
                        started_at: out.started_at,
                        ended_at: out.ended_at,
                        duration_ms: out.duration_ms,
                        attempts: attempts_this_step,
                        route: step.route.clone(),
                        input: activity_in.input,
                        output: out.output,
                        cost_eur: out.cost_eur,
                        usage: out.usage,
                        error: None,
                        notes: out.notes,
                    });
                    step_idx += 1;
                    attempts_this_step = 1;
                }
                Err(e) => {
                    // Phase 6: when an activity surfaces a `NeedsReauth`
                    // application failure, suspend the workflow until
                    // either (a) a `reauth_completed` signal arrives for
                    // the matching capability, in which case we retry
                    // the same step, or (b) `reauth_timeout` elapses, in
                    // which case we fail the run cleanly.
                    if let Some(details) = needs_reauth_details(&e) {
                        let server_id = details.server_id.clone();
                        let server_for_wait = server_id.clone();
                        let mut signal_arrived = false;
                        temporalio_sdk::workflows::select! {
                            _ = ctx.timer(reauth_timeout) => {}
                            _ = ctx.wait_condition(move |s: &Self| {
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
                            attempts_this_step = attempts_this_step.saturating_add(1);
                            // Re-enter the loop without advancing the
                            // step index — retry the same activity with
                            // the same input.
                            continue 'outer;
                        }
                        // Timed out — fail the run.
                        let msg = format!(
                            "timed out after {}s waiting for `cori login {}` (capability: {})",
                            reauth_timeout.as_secs(),
                            details.server_id,
                            details.server_id,
                        );
                        activities.push(ActivitySummary {
                            activity_id: step.activity_id.clone(),
                            step_name: step.name.clone(),
                            kind: step.kind,
                            status: "failed".to_string(),
                            started_at: None,
                            ended_at: None,
                            duration_ms: 0,
                            attempts: attempts_this_step,
                            route: step.route.clone(),
                            input: activity_in.input,
                            output: JsonValue::Null,
                            cost_eur: None,
                            usage: None,
                            error: Some(msg.clone()),
                            notes: vec![format!(
                                "needs sign-in for `{}` — {}",
                                details.server_id, details.hint
                            )],
                        });
                        run_status = "failed";
                        run_error = Some(msg);
                        break;
                    }

                    let msg = format!("{e}");
                    activities.push(ActivitySummary {
                        activity_id: step.activity_id.clone(),
                        step_name: step.name.clone(),
                        kind: step.kind,
                        status: "failed".to_string(),
                        started_at: None,
                        ended_at: None,
                        duration_ms: 0,
                        attempts: attempts_this_step,
                        route: step.route.clone(),
                        input: activity_in.input,
                        output: JsonValue::Null,
                        cost_eur: None,
                        usage: None,
                        error: Some(msg.clone()),
                        notes: Vec::new(),
                    });
                    run_status = "failed";
                    run_error = Some(msg);
                    break;
                }
            }
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

/// Build per-step `ActivityOptions`: default timeout + retry policy per
/// kind, with optional overrides from `step.metadata`.
///
/// Recognised metadata keys:
/// - `timeout_ms` (number): overrides `start_to_close_timeout`.
/// - `retries.max` (number): overrides the default attempt cap.
/// - `retries.backoff` (`"exponential"` | `"linear"`): retry backoff
///   strategy. Defaults to exponential.
fn activity_options_for_step(step: &cori_protocol::CompiledStep) -> ActivityOptions {
    let default_secs: u64 = match step.kind {
        StepKind::Cli => 60,
        StepKind::McpTool => 30,
        StepKind::Code => 30,
        StepKind::Llm => 120,
        StepKind::Builtin => 30,
    };
    let timeout = step
        .metadata
        .get("timeout_ms")
        .and_then(|v| v.as_u64())
        .map(Duration::from_millis)
        .unwrap_or_else(|| Duration::from_secs(default_secs));

    // Default: cli/mcp_tool mutate external state, so we only attempt
    // once unless the step explicitly opts in. Pure (code) and
    // idempotent-ish (llm) kinds get a small retry budget.
    let default_attempts: i32 = match step.kind {
        StepKind::Cli | StepKind::McpTool => 1,
        StepKind::Code | StepKind::Llm => 3,
        StepKind::Builtin => 1,
    };
    let retries = step.metadata.get("retries");
    let max_attempts: i32 = retries
        .and_then(|r| r.get("max"))
        .and_then(|v| v.as_i64())
        .map(|n| n as i32)
        .unwrap_or(default_attempts);

    // Backoff strategy mirrors the SDK's `retries.backoff` field. Linear
    // backoff keeps a constant interval (coefficient 1.0); exponential
    // (the default) doubles each attempt.
    let backoff_coefficient = match retries.and_then(|r| r.get("backoff")).and_then(|v| v.as_str()) {
        Some("linear") => 1.0,
        _ => 2.0,
    };

    let retry_policy = temporalio_common::protos::temporal::api::common::v1::RetryPolicy {
        initial_interval: Some(prost_duration_from_secs(1)),
        backoff_coefficient,
        maximum_interval: Some(prost_duration_from_secs(60)),
        maximum_attempts: max_attempts,
        non_retryable_error_types: vec![
            "MissingCapabilityError".to_string(),
            "AuthenticationError".to_string(),
            "InvalidInputError".to_string(),
            "SchemaValidationError".to_string(),
            "StepFailedError".to_string(),
            "RuntimeUnavailableError".to_string(),
            "MissingEnvelopeError".to_string(),
        ],
    };

    // Per-step task queue (Phase 4). When unset (e.g. legacy callers
    // building a DAG by hand, like the smoke test), fall through to the
    // workflow's own queue.
    //
    // 30s schedule_to_start surfaces missing-worker fast with an
    // actionable error rather than blocking the workflow.
    ActivityOptions::with_start_to_close_timeout(timeout)
        .retry_policy(retry_policy)
        .maybe_task_queue(step.task_queue.clone())
        .schedule_to_start_timeout(Duration::from_secs(30))
        .build()
}

fn prost_duration_from_secs(s: i64) -> prost_wkt_types::Duration {
    prost_wkt_types::Duration {
        seconds: s,
        nanos: 0,
    }
}
