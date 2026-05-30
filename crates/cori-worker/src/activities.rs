//! Temporal activity handlers.
//!
//! Retry / idempotency expectations:
//!
//! - `cori_code` — pure, no side effects. Safe to retry. Default
//!   `max_attempts = 3`.
//! - `cori_llm` — calling twice charges twice but produces an
//!   equivalent result. Safe to retry; default `max_attempts = 3`.
//!   The cost ledger keys on `(run_id, activity_id, attempt)` to avoid
//!   double-billing summaries.
//! - `cori_cli`, `cori_mcp_tool` — may mutate external state. v1 defaults
//!   to `max_attempts = 1`. Steps can opt into retries explicitly via
//!   `retries.max` in metadata.
//!
//! All four handlers share the same input / output shape — the workflow
//! decides which activity to invoke based on `step.kind`. They are async
//! so they can `tokio::task::spawn_blocking` the sync broker entry points
//! without blocking the Temporal worker's poll thread.

use std::path::PathBuf;

use anyhow::anyhow;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use temporalio_macros::activities;
use temporalio_sdk::activities::{ActivityContext, ActivityError};
use temporalio_sdk::error::ApplicationFailure;

use cori_broker::{
    ActivityOutcome, ActivityStatus, BrokerError, TokenUsage, cli as cli_broker, code, dry_run,
    llm, mcp,
};
use cori_protocol::StepKind;

use crate::broker_ctx::broker_ctx;

/// Typed payload attached to a `NeedsReauth` [`ApplicationFailure`].
///
/// Carried on the activity-failure boundary so the workflow side can
/// decide which capability to wait for without re-parsing strings.
/// Phase 6's dispatch loop suspends the step until a matching
/// `reauth_completed` signal arrives or the wait times out.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NeedsReauthDetails {
    pub server_id: String,
    pub user_id: String,
    pub auth_kind: String,
    pub hint: String,
}

/// Per-activity input. The workflow builds this from its in-memory step
/// outputs and passes it through Temporal as a JSON payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivityInput {
    /// Stable id for this step (matches `CompiledStep::activity_id`).
    pub step_id: String,
    /// Friendly name for tracing (matches `CompiledStep::name`).
    pub step_name: String,
    /// Step kind — informational; the workflow already dispatched on it.
    pub step_kind: StepKind,
    /// Relative source path under the workflow root.
    pub source_path: PathBuf,
    /// Optional route key (for diagnostics).
    pub route: Option<String>,
    /// Resolved input object for this step.
    pub input: JsonValue,
    /// Cori workflow id (the registered workflow id, not a Temporal type name).
    pub workflow_id: String,
    /// Cori run id (== Temporal workflow execution id).
    pub run_id: String,
    /// Stable id of the user who originated this run. Used by the
    /// broker to scope credential / OAuth-token lookup. Empty string
    /// for legacy traces that predate Phase 4.
    #[serde(default)]
    pub user_id: String,
    /// When true, this activity should return a mocked outcome without
    /// touching the outside world.
    pub dry_run: bool,
}

/// Per-activity output. Mirrors what the in-process executor previously
/// stored in `ActivityTrace`, minus the trace-only metadata.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ActivityOutput {
    /// Status string: `"ok"` | `"failed"` | `"skipped"`.
    pub status: String,
    /// The activity's decoded JSON output (or `Null` when failed/skipped).
    pub output: JsonValue,
    /// Captured stderr from the broker subprocess (truncated upstream).
    pub stderr: String,
    /// Total wall time as observed by the broker.
    pub duration_ms: u64,
    /// Wall-clock start time recorded on the activity worker (safe —
    /// the workflow body itself never reads a clock).
    pub started_at: Option<DateTime<Utc>>,
    /// Wall-clock end time recorded on the activity worker.
    pub ended_at: Option<DateTime<Utc>>,
    /// Monetary cost in EUR when the activity paid for an external call.
    pub cost_eur: Option<f64>,
    /// LLM token usage.
    pub usage: Option<TokenUsage>,
    /// Free-form notes (e.g. `"mocked by --dry-run"`).
    pub notes: Vec<String>,
}

/// Marker struct registered with `WorkerOptions::register_activities`.
pub struct CoriActivities;

#[activities]
impl CoriActivities {
    /// Executes a `code` step via the Deno runner. Pure, retryable.
    #[activity]
    pub async fn cori_code(
        _ctx: ActivityContext,
        input: ActivityInput,
    ) -> Result<ActivityOutput, ActivityError> {
        run_step(input, BrokerKind::Code).await
    }

    /// Executes a `cli` step via the configured CLI binary. Defaults to
    /// `max_attempts = 1`; mutating commands are opt-in retryable.
    #[activity]
    pub async fn cori_cli(
        _ctx: ActivityContext,
        input: ActivityInput,
    ) -> Result<ActivityOutput, ActivityError> {
        run_step(input, BrokerKind::Cli).await
    }

    /// Executes an `mcp_tool` step. Defaults to `max_attempts = 1`.
    #[activity]
    pub async fn cori_mcp_tool(
        _ctx: ActivityContext,
        input: ActivityInput,
    ) -> Result<ActivityOutput, ActivityError> {
        run_step(input, BrokerKind::Mcp).await
    }

    /// Executes an `llm` step. Retryable; cost is tracked per attempt.
    #[activity]
    pub async fn cori_llm(
        _ctx: ActivityContext,
        input: ActivityInput,
    ) -> Result<ActivityOutput, ActivityError> {
        run_step(input, BrokerKind::Llm).await
    }
}

#[derive(Debug, Clone, Copy)]
enum BrokerKind {
    Cli,
    Mcp,
    Code,
    Llm,
}

/// Common body for all four activities. Bridges from the async Temporal
/// activity boundary to the sync broker via `spawn_blocking`.
async fn run_step(input: ActivityInput, kind: BrokerKind) -> Result<ActivityOutput, ActivityError> {
    let ctx = broker_ctx();
    let absolute_path = ctx.source_root.join(&input.source_path);
    let dry_run = input.dry_run;
    let step_input = input.input.clone();
    let user_id = input.user_id.clone();
    let credentials_dir = ctx.credentials_dir.clone();
    let started_at = Utc::now();

    let outcome: Result<ActivityOutcome, BrokerError> =
        tokio::task::spawn_blocking(move || match (kind, dry_run) {
            (BrokerKind::Code, _) => code::run(&ctx.runtime, &absolute_path, &step_input),
            (BrokerKind::Cli, false) => cli_broker::run(
                &ctx.runtime,
                &ctx.caps,
                &absolute_path,
                &step_input,
                &user_id,
            ),
            (BrokerKind::Cli, true) => dry_run::cli(&ctx.runtime, &absolute_path, &step_input),
            (BrokerKind::Mcp, false) => mcp::run(
                &ctx.runtime,
                &ctx.caps,
                &absolute_path,
                &step_input,
                &user_id,
                &credentials_dir,
            ),
            (BrokerKind::Mcp, true) => dry_run::mcp(&ctx.runtime, &absolute_path, &step_input),
            (BrokerKind::Llm, false) => {
                llm::run(&ctx.runtime, &absolute_path, &step_input, &ctx.llm_opts)
            }
            (BrokerKind::Llm, true) => dry_run::llm(&ctx.runtime, &absolute_path, &step_input),
        })
        .await
        .map_err(|join_err| {
            // A panic inside the broker is non-retryable — re-raising won't
            // help and Temporal would otherwise loop forever.
            ActivityError::application(ApplicationFailure::non_retryable(anyhow!(
                "broker task panicked: {join_err}"
            )))
        })?;

    match outcome {
        Ok(o) => Ok(map_outcome(o, dry_run, started_at)),
        Err(err) => Err(broker_error_to_activity_error(err)),
    }
}

fn map_outcome(o: ActivityOutcome, dry_run: bool, started_at: DateTime<Utc>) -> ActivityOutput {
    let status = match o.status {
        ActivityStatus::Ok => "ok",
        ActivityStatus::Skipped => "skipped",
        ActivityStatus::Failed => "failed",
    };
    let mocked = dry_run && matches!(o.status, ActivityStatus::Skipped);
    let duration_ms = u64::try_from(o.duration.as_millis()).unwrap_or(u64::MAX);
    let ended_at =
        started_at + chrono::Duration::milliseconds(duration_ms.min(i64::MAX as u64) as i64);
    ActivityOutput {
        status: status.to_string(),
        output: o.output,
        stderr: o.stderr,
        duration_ms,
        started_at: Some(started_at),
        ended_at: Some(ended_at),
        cost_eur: o.cost_eur,
        usage: o.usage,
        notes: if mocked {
            vec!["mocked by --dry-run".to_string()]
        } else {
            Vec::new()
        },
    }
}

/// Classify a [`BrokerError`] into a Temporal [`ActivityError`] so
/// Temporal's retry policy can apply the right behaviour. Permanent
/// failures (bad capability, bad input, auth) are marked
/// non-retryable; transient I/O / 5xx / rate limits stay retryable.
fn broker_error_to_activity_error(err: BrokerError) -> ActivityError {
    let category = classify(&err);
    let (type_name, non_retryable) = match category {
        Category::Retryable { type_name } => (type_name, false),
        Category::NonRetryable { type_name } => (type_name, true),
    };
    // Phase 6: NeedsReauth carries a typed payload the workflow side
    // decodes via `ApplicationFailure::details::<NeedsReauthDetails>()`.
    // Extract before moving `err` into the source chain.
    let reauth_details = match &err {
        BrokerError::NeedsReauth {
            server_id,
            owner_id,
            auth_kind,
            hint,
            ..
        } => Some(NeedsReauthDetails {
            server_id: server_id.clone(),
            user_id: owner_id.clone(),
            auth_kind: (*auth_kind).to_string(),
            hint: hint.clone(),
        }),
        _ => None,
    };
    let builder = ApplicationFailure::builder(anyhow::Error::new(err))
        .type_name(type_name.to_string())
        .non_retryable(non_retryable);
    let af = match reauth_details {
        Some(details) => builder.details(details).build(),
        None => builder.build(),
    };
    ActivityError::application(af)
}

enum Category {
    Retryable { type_name: &'static str },
    NonRetryable { type_name: &'static str },
}

fn classify(err: &BrokerError) -> Category {
    use BrokerError::*;
    match err {
        // Permanent — never retryable, all four kinds.
        CapabilityDenied { .. } => Category::NonRetryable {
            type_name: "MissingCapabilityError",
        },
        // Phase 5: missing/expired OAuth or CLI auth. Surfaced as a
        // distinct type_name so Phase 6's workflow-side signal handler
        // can catch it specifically and suspend the run instead of
        // failing it outright.
        NeedsReauth { .. } => Category::NonRetryable {
            type_name: "NeedsReauth",
        },
        LlmMissingCredentials { .. } => Category::NonRetryable {
            type_name: "AuthenticationError",
        },
        LlmUnknownModel { .. } => Category::NonRetryable {
            type_name: "InvalidInputError",
        },
        LlmSchemaMismatch { .. } => Category::NonRetryable {
            type_name: "SchemaValidationError",
        },
        BadEnvelope { .. } => Category::NonRetryable {
            type_name: "SchemaValidationError",
        },
        RuntimeUnavailable(_) => Category::NonRetryable {
            type_name: "RuntimeUnavailableError",
        },
        StepFailed { .. } => Category::NonRetryable {
            type_name: "StepFailedError",
        },
        MissingEnvelope { .. } => Category::NonRetryable {
            type_name: "MissingEnvelopeError",
        },

        // Transient — retryable.
        Spawn(_) | Io(_) | CliSpawn { .. } | McpSpawn { .. } => Category::Retryable {
            type_name: "IoError",
        },
        CliExitNonZero { .. } => Category::Retryable {
            type_name: "CliExitNonZeroError",
        },
        McpProtocol(_) => Category::Retryable {
            type_name: "McpProtocolError",
        },
        LlmHttp(_) => Category::Retryable {
            type_name: "LlmHttpError",
        },
        // 4xx (auth/perm) won't recover; 5xx + 429 may. Without finer
        // detail in the error we treat as retryable; the per-step retry
        // cap bounds the damage.
        LlmProviderError { .. } => Category::Retryable {
            type_name: "LlmProviderError",
        },
    }
}
