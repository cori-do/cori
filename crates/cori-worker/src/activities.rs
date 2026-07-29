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
use std::time::Instant;

use anyhow::anyhow;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map as JsonMap, Value as JsonValue};
use temporalio_macros::activities;
use temporalio_sdk::activities::{ActivityContext, ActivityError};
use temporalio_sdk::error::ApplicationFailure;

use cori_broker::{
    ActivityOutcome, ActivityStatus, BrokerError, TokenUsage, cli as cli_broker, code, dry_run,
    llm, mcp, source_bundle,
};
use cori_protocol::{SourceBundle, StepKind};

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

/// Immutable compile-time facts copied from [`cori_protocol::CompiledStep`].
///
/// Keeping these facts in the activity payload makes the Temporal history the
/// source of truth. An activity must never infer its authorization boundary
/// from a mutable workflow file on disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrozenStep {
    pub source_sha256: String,
    /// Digest of the complete workflow tree, including imported helpers.
    /// Optional only for replaying histories created before tree-boundary
    /// enforcement.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_content_hash: Option<String>,
    #[serde(default)]
    pub metadata: JsonMap<String, JsonValue>,
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
    /// Absolute filesystem path of the workflow folder on the
    /// triggering machine. `run_step` joins it with `source_path` to
    /// locate the step file. Falls back to `BrokerCtx::source_root` if
    /// empty (legacy traces / smoke-test in-memory DAGs).
    #[serde(default)]
    pub source_root: String,
    /// Immutable workflow source for cross-machine dispatch. When present the
    /// worker materializes and verifies it before resolving `source_path`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_bundle: Option<SourceBundle>,
    /// Compile-time source digest and metadata. Missing only for Temporal
    /// histories created before source-boundary enforcement was introduced.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frozen_step: Option<FrozenStep>,
}

/// Per-activity output. Mirrors what the in-process executor previously
/// stored in `ActivityTrace`, minus the trace-only metadata.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ActivityOutput {
    /// Status string: `"ok"` | `"failed"` | `"skipped"`.
    pub status: String,
    /// The activity's decoded JSON output. Dry-run activities use
    /// schema-valid stubs even though their status is `skipped`.
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
    // Prefer the triggering workflow's root (carried in ActivityInput).
    // Falls back to the worker process's startup `cwd` only when the
    // input came from a pre-fix trace or an in-memory smoke-test DAG.
    let workflow_root: std::path::PathBuf = if input.source_root.is_empty() {
        ctx.source_root.clone()
    } else {
        std::path::PathBuf::from(&input.source_root)
    };
    let dry_run = input.dry_run;
    let step_input = input.input.clone();
    let user_id = input.user_id.clone();
    let frozen_step = input.frozen_step.clone();
    let bundled_source = input.source_bundle.clone();
    let source_path = input.source_path.clone();
    let source_cache_dir = ctx.source_cache_dir.clone();
    let credentials_dir = ctx.credentials_dir.clone();
    let started_at = Utc::now();
    let activity_started = Instant::now();

    let outcome: Result<ActivityOutcome, BrokerError> = tokio::task::spawn_blocking(move || {
        let workflow_root = resolve_activity_source_root(
            workflow_root,
            bundled_source.as_ref(),
            &source_cache_dir,
            frozen_step.as_ref(),
        )?;
        let absolute_path = workflow_root.join(&source_path);
        verify_source_boundary(&workflow_root, &absolute_path, frozen_step.as_ref())?;
        match (kind, dry_run) {
            (BrokerKind::Code, _) => code::run(&ctx.runtime, &absolute_path, &step_input),
            (BrokerKind::Cli, false) => {
                let expected_binary = expected_cli_binary(&absolute_path, frozen_step.as_ref())?;
                cli_broker::run(
                    &ctx.runtime,
                    &ctx.caps,
                    &absolute_path,
                    &step_input,
                    &user_id,
                    Some(&expected_binary),
                )
            }
            (BrokerKind::Cli, true) => {
                let expected_binary = expected_cli_binary(&absolute_path, frozen_step.as_ref())?;
                dry_run::cli(
                    &ctx.runtime,
                    &absolute_path,
                    &step_input,
                    Some(&expected_binary),
                )
            }
            (BrokerKind::Mcp, false) => {
                let expected_server =
                    expected_metadata_string(frozen_step.as_ref(), "server", "MCP server")?;
                let expected_tool =
                    expected_metadata_string(frozen_step.as_ref(), "tool", "MCP tool")?;
                let expected_target = expected_server.as_deref().zip(expected_tool.as_deref());
                mcp::run(
                    &ctx.runtime,
                    &ctx.caps,
                    &absolute_path,
                    &step_input,
                    &user_id,
                    &credentials_dir,
                    expected_target,
                )
            }
            (BrokerKind::Mcp, true) => {
                let expected_server =
                    expected_metadata_string(frozen_step.as_ref(), "server", "MCP server")?;
                let expected_tool =
                    expected_metadata_string(frozen_step.as_ref(), "tool", "MCP tool")?;
                dry_run::mcp(
                    &ctx.runtime,
                    &absolute_path,
                    &step_input,
                    expected_server.as_deref(),
                    expected_tool.as_deref(),
                )
            }
            (BrokerKind::Llm, false) => {
                let expected_model =
                    expected_metadata_string(frozen_step.as_ref(), "model", "LLM model")?;
                llm::run(
                    &ctx.runtime,
                    &absolute_path,
                    &step_input,
                    &ctx.llm_opts,
                    expected_model.as_deref(),
                )
            }
            (BrokerKind::Llm, true) => {
                let expected_model =
                    expected_metadata_string(frozen_step.as_ref(), "model", "LLM model")?;
                dry_run::llm(
                    &ctx.runtime,
                    &absolute_path,
                    &step_input,
                    expected_model.as_deref(),
                )
            }
        }
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
        Ok(mut outcome) => {
            // Include source-integrity verification and blocking-pool queue
            // time in the activity wall clock, not only the inner broker call.
            outcome.duration = activity_started.elapsed();
            Ok(map_outcome(outcome, dry_run, started_at))
        }
        Err(err) => Err(broker_error_to_activity_error(err)),
    }
}

fn resolve_activity_source_root(
    legacy_root: PathBuf,
    bundle: Option<&SourceBundle>,
    source_cache_dir: &std::path::Path,
    frozen: Option<&FrozenStep>,
) -> Result<PathBuf, BrokerError> {
    let Some(bundle) = bundle else {
        return Ok(legacy_root);
    };
    if let Some(expected) = frozen.and_then(|frozen| frozen.workflow_content_hash.as_deref())
        && bundle.content_sha256 != expected
    {
        return Err(BrokerError::SourceBundle {
            message: format!(
                "bundle content digest {} does not match workflow history digest {expected}",
                bundle.content_sha256
            ),
        });
    }
    source_bundle::materialize(bundle, source_cache_dir)
}

fn verify_source_boundary(
    workflow_root: &std::path::Path,
    step_path: &std::path::Path,
    frozen: Option<&FrozenStep>,
) -> Result<(), BrokerError> {
    let Some(frozen) = frozen else {
        return Ok(());
    };
    let source = std::fs::read(step_path).map_err(BrokerError::Io)?;
    let actual = cori_compiler::source_sha256(&source);
    if actual != frozen.source_sha256 {
        return Err(BrokerError::StepFailed {
            message: format!(
                "step source changed after compilation for `{}` (expected sha256 {}, got {})",
                step_path.display(),
                frozen.source_sha256,
                actual,
            ),
            stack: None,
        });
    }

    if let Some(expected) = frozen.workflow_content_hash.as_deref() {
        let actual = cori_compiler::workflow_content_hash(workflow_root).map_err(|error| {
            BrokerError::StepFailed {
                message: format!(
                    "could not verify frozen workflow tree `{}`: {error}",
                    workflow_root.display()
                ),
                stack: None,
            }
        })?;
        if actual != expected {
            return Err(BrokerError::StepFailed {
                message: format!(
                    "workflow files changed after compilation under `{}` (expected content hash {}, got {})",
                    workflow_root.display(),
                    expected,
                    actual,
                ),
                stack: None,
            });
        }
    }
    Ok(())
}

fn expected_cli_binary(
    step_path: &std::path::Path,
    frozen: Option<&FrozenStep>,
) -> Result<String, BrokerError> {
    if let Some(frozen) = frozen {
        return frozen
            .metadata
            .get("binary")
            .and_then(JsonValue::as_str)
            .map(str::to_owned)
            .ok_or_else(|| BrokerError::StepFailed {
                message: format!(
                    "compiled CLI metadata for `{}` is missing its frozen `binary`",
                    step_path.display()
                ),
                stack: None,
            });
    }

    // Compatibility only for Temporal histories produced before FrozenStep
    // existed. New runs always use the compiled metadata above.
    let source = std::fs::read_to_string(step_path).map_err(BrokerError::Io)?;
    cori_compiler::cli_binary_from_source(&source).map_err(|message| BrokerError::StepFailed {
        message: format!(
            "could not revalidate CLI capability boundary for `{}`: {message}",
            step_path.display()
        ),
        stack: None,
    })
}

fn expected_metadata_string(
    frozen: Option<&FrozenStep>,
    key: &str,
    label: &str,
) -> Result<Option<String>, BrokerError> {
    let Some(frozen) = frozen else {
        return Ok(None);
    };
    frozen
        .metadata
        .get(key)
        .and_then(JsonValue::as_str)
        .map(|value| Some(value.to_string()))
        .ok_or_else(|| BrokerError::StepFailed {
            message: format!(
                "compiled activity metadata is missing the required `{key}` {label} boundary"
            ),
            stack: None,
        })
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
    let mut notes = o.notes;
    if mocked {
        notes.push("mocked by --dry-run".to_string());
    }
    ActivityOutput {
        status: status.to_string(),
        output: o.output,
        stderr: o.stderr,
        duration_ms,
        started_at: Some(started_at),
        ended_at: Some(ended_at),
        cost_eur: o.cost_eur,
        usage: o.usage,
        notes,
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
        SchemaValidation { .. } => Category::NonRetryable {
            type_name: "SchemaValidationError",
        },
        BadEnvelope { .. } => Category::NonRetryable {
            type_name: "SchemaValidationError",
        },
        RuntimeUnavailable(_) => Category::NonRetryable {
            type_name: "RuntimeUnavailableError",
        },
        SourceBundle { .. } => Category::NonRetryable {
            type_name: "SourceBundleError",
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
        LlmProviderError {
            status: 401 | 403, ..
        } => Category::NonRetryable {
            type_name: "AuthenticationError",
        },
        LlmProviderError {
            status: 408 | 429, ..
        }
        | LlmProviderError {
            status: 500..=599, ..
        } => Category::Retryable {
            type_name: "LlmProviderError",
        },
        LlmProviderError {
            status: 400..=499, ..
        } => Category::NonRetryable {
            type_name: "LlmProviderRequestError",
        },
        // A malformed success envelope can be transient provider behavior.
        // Redirects and other unexpected statuses are bounded by the step's
        // validated retry policy.
        LlmProviderError { .. } => Category::Retryable {
            type_name: "LlmProviderProtocolError",
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn runner_schema_validation_is_non_retryable() {
        let error = BrokerError::SchemaValidation {
            message: "input.messages[0].id is required".to_string(),
            stack: None,
        };
        assert!(matches!(
            classify(&error),
            Category::NonRetryable {
                type_name: "SchemaValidationError"
            }
        ));
    }

    #[test]
    fn llm_http_statuses_follow_retry_taxonomy() {
        let provider_error = |status| BrokerError::LlmProviderError {
            provider: "openai",
            status,
            body: "fixture".to_string(),
        };
        for status in [408, 429, 500, 503] {
            assert!(matches!(
                classify(&provider_error(status)),
                Category::Retryable { .. }
            ));
        }
        for status in [400, 401, 403, 404, 422] {
            assert!(matches!(
                classify(&provider_error(status)),
                Category::NonRetryable { .. }
            ));
        }
    }

    #[test]
    fn frozen_cli_metadata_does_not_reparse_mutable_source() {
        let temp = tempdir().expect("temporary workflow");
        let step = temp.path().join("01_cli.ts");
        std::fs::write(
            &step,
            "export default step.cli({ command: () => [\"unexpected\"] });",
        )
        .expect("step source");
        let frozen = FrozenStep {
            source_sha256: "not needed by metadata lookup".to_string(),
            workflow_content_hash: None,
            metadata: JsonMap::from_iter([(
                "binary".to_string(),
                JsonValue::String("approved".to_string()),
            )]),
        };

        assert_eq!(
            expected_cli_binary(&step, Some(&frozen)).expect("frozen binary"),
            "approved"
        );
    }

    #[test]
    fn source_boundary_detects_step_and_imported_helper_drift() {
        let temp = tempdir().expect("temporary workflow");
        let steps = temp.path().join("steps");
        std::fs::create_dir(&steps).expect("steps directory");
        let step = steps.join("01_code.ts");
        std::fs::write(&step, "export default { kind: \"code\" };\n").expect("step source");
        let helper = temp.path().join("types.ts");
        std::fs::write(&helper, "export type Row = { id: string };\n").expect("helper");

        let source = std::fs::read(&step).expect("step bytes");
        let frozen = FrozenStep {
            source_sha256: cori_compiler::source_sha256(&source),
            workflow_content_hash: Some(
                cori_compiler::workflow_content_hash(temp.path()).expect("workflow hash"),
            ),
            metadata: JsonMap::new(),
        };
        verify_source_boundary(temp.path(), &step, Some(&frozen))
            .expect("unchanged workflow should pass");

        std::fs::write(&helper, "export type Row = { id: number };\n").expect("mutate helper");
        let helper_error = verify_source_boundary(temp.path(), &step, Some(&frozen))
            .expect_err("helper drift must fail");
        assert!(helper_error.to_string().contains("workflow files changed"));

        std::fs::write(&step, "export default { kind: \"llm\" };\n").expect("mutate step");
        let step_error = verify_source_boundary(temp.path(), &step, Some(&frozen))
            .expect_err("step drift must fail");
        assert!(step_error.to_string().contains("step source changed"));
    }

    #[test]
    fn bundled_source_materializes_when_trigger_path_is_unavailable() {
        let source = tempdir().expect("source workflow");
        let cache = tempdir().expect("worker source cache");
        std::fs::create_dir(source.path().join("steps")).expect("steps");
        let step = source.path().join("steps/01_code.ts");
        std::fs::write(&step, "export default { kind: \"code\" };\n").expect("step");
        std::fs::write(source.path().join("manifest.md"), "source transport\n").expect("manifest");
        let tree_hash = cori_compiler::workflow_content_hash(source.path()).expect("workflow hash");
        let bundle =
            cori_broker::source_bundle::build(source.path(), &tree_hash).expect("source bundle");
        let frozen = FrozenStep {
            source_sha256: cori_compiler::source_sha256(&std::fs::read(&step).expect("step bytes")),
            workflow_content_hash: Some(tree_hash),
            metadata: JsonMap::new(),
        };

        let resolved = resolve_activity_source_root(
            PathBuf::from("/trigger/path/not/available/on/worker"),
            Some(&bundle),
            cache.path(),
            Some(&frozen),
        )
        .expect("worker materialization");
        assert!(resolved.join("steps/01_code.ts").is_file());
        verify_source_boundary(&resolved, &resolved.join("steps/01_code.ts"), Some(&frozen))
            .expect("materialized bytes match workflow history");
    }
}
