//! `cori-run` — run orchestration library.
//!
//! Provides the full `cori run` pipeline as a library, used by both
//! the `cori` CLI and Cori Console. Neither front-end contains run
//! logic; both call into this crate, which has no UI dependencies.
//!
//! ## Dependency chain (no cycles)
//! `cori-cli` → `cori-run`; `cori-console` → `cori-run`;
//! both → `cori-protocol`. `cori-run` depends on `cori-broker`,
//! `cori-compiler`, `cori-manifest`, `cori-worker`, `cori-protocol`.

pub mod config;
pub mod paths;
pub mod planner;
pub mod remote;
pub mod runtime;
pub mod temporal_endpoint;
pub mod workflow_loader;

use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use chrono::Utc;
use cori_broker::capabilities::{self, CapabilityReport};
use cori_broker::identity::{IdentitySource, OsUser};
use cori_broker::llm::{LlmCredentials, LlmOptions};
use cori_broker::{TriggerContext, runtime as broker_runtime};
use cori_protocol::{
    ActivityTrace, CostSummary, RunTrace, StepKind, TokenUsage, WorkerIdentity,
    identity_from_queue, task_queue_for,
};
use cori_worker::broker_ctx::{BrokerCtx, set_broker_ctx};
use cori_worker::runner::run_workflow_once;
use cori_worker::runtime::{CoriTemporalRuntime, DEFAULT_NAMESPACE, preflight_check};
use cori_worker::workflow::{ActivitySummary, WorkflowInput};
use serde_json::{Map as JsonMap, Value as JsonValue};

// ---------------------------------------------------------------------------
// Public API types
// ---------------------------------------------------------------------------

/// Which surface triggered this run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    Cli,
    Console,
    Schedule,
}

impl Trigger {
    fn as_str(self) -> &'static str {
        match self {
            Trigger::Cli => "cli",
            Trigger::Console => "console",
            Trigger::Schedule => "schedule",
        }
    }
}

/// Information presented to the user when first-run consent is needed.
pub struct ConsentPrompt<'a> {
    pub spec: &'a remote::RemoteRef,
    pub sha: &'a str,
    pub workflow_dir: &'a std::path::Path,
    pub compiled: &'a cori_protocol::CompiledWorkflow,
}

/// The user's decision in response to a consent prompt.
pub enum ConsentDecision {
    /// User agreed; `run_workflow` will call `record_consent` and proceed.
    Granted,
    /// User refused; `run_workflow` returns an error.
    Denied,
    /// Caller wants to handle consent out-of-band (Console → 409).
    /// `run_workflow` returns `ConsentRequiredError`.
    Defer,
}

/// How `run_workflow` should handle a first-run consent gate.
pub enum ConsentCallback {
    /// Skip the prompt and proceed as if the user said yes.
    AssumeYes,
    /// Call the provided closure when consent is needed.
    Prompt(Box<dyn Fn(&ConsentPrompt<'_>) -> ConsentDecision + Send + Sync>),
}

/// Sink for live step events during a run.
/// Phase 0: called in sequence after the workflow completes.
/// Phase 3: will be called live as each activity starts/finishes.
pub trait ProgressSink: Send + Sync {
    fn on_plan(&self, plan: &[planner::StepAssignment]);
    fn on_step_start(&self, summary: &ActivitySummary);
    fn on_step_finish(&self, summary: &ActivitySummary);
}

/// A no-op [`ProgressSink`] for callers that don't need live events.
pub struct NoopSink;

impl ProgressSink for NoopSink {
    fn on_plan(&self, _plan: &[planner::StepAssignment]) {}
    fn on_step_start(&self, _summary: &ActivitySummary) {}
    fn on_step_finish(&self, _summary: &ActivitySummary) {}
}

// ---------------------------------------------------------------------------
// Preflight
// ---------------------------------------------------------------------------

/// Lightweight outcome of [`preflight`] — used by `cori check` and
/// `GET /api/workflow` to inspect a workflow before running it.
pub struct PreflightOutcome {
    pub loaded: workflow_loader::LoadedWorkflow,
    pub caps: cori_broker::capabilities::Capabilities,
    pub cap_report: CapabilityReport,
    pub missing_caps: Vec<String>,
    /// `Some` when first-run consent is required before running.
    pub consent_required: Option<ConsentRequired>,
}

pub struct ConsentRequired {
    pub spec: remote::RemoteRef,
    pub sha: String,
}

/// Resolve + compile + plan without touching Temporal.
///
/// Used by `cori check` and `GET /api/workflow`. Does not require
/// Temporal to be reachable. Returns the compiled workflow + capability
/// assessment (+ a `ConsentRequired` marker if a remote ref needs
/// first-run consent).
pub fn preflight(source: &str, update: bool, assume_yes: bool) -> Result<PreflightOutcome> {
    let (resolved, loaded) = workflow_loader::resolve_arg(source, update)?;

    let mut consent_required = None;
    if let Some(rr) = resolved.remote.as_ref()
        && !remote::trust::is_trusted(&rr.spec, &rr.sha)?
    {
        if assume_yes || remote::trust::assume_yes_env() {
            remote::trust::record_consent(
                &rr.spec,
                &rr.sha,
                remote::trust::declared_capability_strings(&loaded.compiled),
            )?;
        } else {
            consent_required = Some(ConsentRequired {
                spec: rr.spec.clone(),
                sha: rr.sha.clone(),
            });
        }
    }

    let credentials = resolve_llm_credentials();
    let home = paths::home()?;
    let caps = capabilities::discover(&home, &loaded.compiled.required_cli_binaries, &credentials);
    let identity = OsUser
        .resolve()
        .context("resolving OS user identity for preflight")?;
    let cap_report = CapabilityReport::from_capabilities_with(
        identity,
        &caps,
        Some(&paths::credentials_dir()?),
    );

    let missing_caps = capabilities::validate(
        &caps,
        &loaded.compiled.required_cli_binaries,
        &loaded.compiled.required_mcp_servers,
        &loaded.compiled.required_llm_providers,
    )
    .into_iter()
    .map(|m| m.to_string())
    .collect();

    Ok(PreflightOutcome {
        loaded,
        caps,
        cap_report,
        missing_caps,
        consent_required,
    })
}

// ---------------------------------------------------------------------------
// run_workflow — the full pipeline
// ---------------------------------------------------------------------------

/// Execute a workflow end-to-end and return the persisted [`RunTrace`].
///
/// This is the single code path shared by `cori run`, Cori Console
/// (`POST /api/runs`), and scheduled runs. It does not exit the
/// process; the caller handles UI/error handling.
pub async fn run_workflow(
    source: String,
    params: JsonValue,
    dry_run: bool,
    update: bool,
    trigger: Trigger,
    consent: ConsentCallback,
    progress: Arc<dyn ProgressSink>,
) -> Result<RunTrace> {
    // 1. Resolve + compile
    let (resolved, mut loaded) = workflow_loader::resolve_arg(&source, update)?;

    // 2. Consent gate
    if let Some(rr) = resolved.remote.as_ref()
        && !remote::trust::is_trusted(&rr.spec, &rr.sha)?
    {
        let caps_strings = remote::trust::declared_capability_strings(&loaded.compiled);
        match &consent {
            ConsentCallback::AssumeYes => {
                remote::trust::record_consent(&rr.spec, &rr.sha, caps_strings)?;
            }
            ConsentCallback::Prompt(f) => {
                let prompt = ConsentPrompt {
                    spec: &rr.spec,
                    sha: &rr.sha,
                    workflow_dir: &loaded.absolute_path,
                    compiled: &loaded.compiled,
                };
                match f(&prompt) {
                    ConsentDecision::Granted => {
                        remote::trust::record_consent(&rr.spec, &rr.sha, caps_strings)?;
                    }
                    ConsentDecision::Denied => {
                        bail!("consent declined; not running remote workflow");
                    }
                    ConsentDecision::Defer => {
                        bail!("consent_required");
                    }
                }
            }
        }
    }

    // 3. Install Deno runtime
    runtime::ensure_installed()?;
    let runtime_root = paths::runtime_dir()?;
    let runtime = broker_runtime::Runtime::resolve(&runtime_root).map_err(|e| {
        anyhow::anyhow!(
            "{e}\n\nIf you have Deno installed, you can also point Cori at it with:\n  \
             export CORI_DENO=$(which deno)"
        )
    })?;

    // 4. Capabilities
    let credentials = resolve_llm_credentials();
    let llm_opts = LlmOptions {
        credentials: credentials.clone(),
        trigger: Some(match trigger {
            Trigger::Cli => TriggerContext::Cli,
            Trigger::Console | Trigger::Schedule => TriggerContext::Cli,
        }),
    };

    let home = paths::home()?;
    let caps = capabilities::discover(&home, &loaded.compiled.required_cli_binaries, &credentials);

    let missing: Vec<String> = capabilities::validate(
        &caps,
        &loaded.compiled.required_cli_binaries,
        &loaded.compiled.required_mcp_servers,
        &loaded.compiled.required_llm_providers,
    )
    .into_iter()
    .map(|m| m.to_string())
    .collect();
    if !missing.is_empty() && !dry_run {
        bail!(
            "missing capabilities — install the listed tools and try again: {}",
            missing.join(", ")
        );
    }

    // 5. Auth preflight
    if !dry_run {
        let credentials_dir_for_check = paths::credentials_dir()?;
        let probe_identity = OsUser
            .resolve()
            .context("resolving OS user identity for preflight auth check")?;
        let probe_report = CapabilityReport::from_capabilities_with(
            probe_identity,
            &caps,
            Some(&credentials_dir_for_check),
        );
        let mut needs_login: Vec<String> = Vec::new();
        for cli in &loaded.compiled.required_cli_binaries {
            if let Some(c) = probe_report.capabilities.iter().find(|c| &c.id == cli)
                && !c.authed
            {
                needs_login.push(cli.clone());
            }
        }
        for mcp in &loaded.compiled.required_mcp_servers {
            if let Some(c) = probe_report.capabilities.iter().find(|c| &c.id == mcp)
                && !c.authed
            {
                needs_login.push(mcp.clone());
            }
        }
        if !needs_login.is_empty() {
            bail!(
                "capabilities need sign-in — run `cori login <id>` and try again: {}",
                needs_login.join(", ")
            );
        }
    }

    // 6. Temporal endpoint
    let endpoint = temporal_endpoint::resolve()?;
    if let Err(e) = preflight_check(&endpoint.target, std::time::Duration::from_millis(500)) {
        bail!(
            "Temporal not reachable at {} — start `temporal server start-dev` or set \
             temporal.host\n  {e:#}",
            endpoint.target
        );
    }

    // 7. Identity + task queue
    let identity = OsUser
        .resolve()
        .context("resolving OS user identity for this run")?;
    let user_id = match &identity {
        WorkerIdentity::Person { user_id } => user_id.clone(),
        WorkerIdentity::Service { pool } => pool.clone(),
    };
    let task_queue = task_queue_for(&identity);

    // 8. Planner
    let mut cluster = planner::ClusterView::load().unwrap_or_default();
    let self_report = CapabilityReport::from_capabilities_with(
        identity.clone(),
        &caps,
        Some(&paths::credentials_dir()?),
    );
    cluster.add_self(self_report);
    let assignments = match planner::assign_queues(&mut loaded.compiled, &identity, &cluster) {
        Ok(a) => a,
        Err(e) => bail!("placement failed: {e}"),
    };

    progress.on_plan(&assignments);

    // 9. Build Temporal worker + run
    let run_id = new_run_id();
    let started_at = Utc::now();
    let start_instant = Instant::now();

    let broker_ctx = BrokerCtx {
        runtime: runtime.clone(),
        caps: caps.clone(),
        llm_opts: llm_opts.clone(),
        source_root: loaded.absolute_path.clone(),
        credentials_dir: paths::credentials_dir()?,
    };
    let _ = set_broker_ctx(broker_ctx);

    let workflow_input = WorkflowInput {
        workflow_id: loaded.compiled.manifest.id.clone(),
        workflow_content_hash: Some(loaded.content_hash.clone()),
        user_id: user_id.clone(),
        compiled_dag: loaded.compiled.clone(),
        user_params: params.clone(),
        dry_run,
        reauth_timeout_secs: std::env::var("CORI_REAUTH_TIMEOUT_SECS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok()),
    };

    let workflow_output_res = {
        let temporal_rt = CoriTemporalRuntime::connect(
            endpoint.target.clone(),
            DEFAULT_NAMESPACE,
            task_queue.clone(),
        )
        .await?;
        run_workflow_once(&temporal_rt, run_id.clone(), workflow_input).await
    };

    let mut run_status = "succeeded".to_string();
    let mut run_error: Option<String> = None;
    let workflow_output = match workflow_output_res {
        Ok(o) => o,
        Err(e) => {
            run_status = "failed".to_string();
            run_error = Some(format!("{e:#}"));
            cori_worker::workflow::WorkflowOutput {
                run_id: run_id.clone(),
                status: "failed".to_string(),
                final_output: JsonValue::Null,
                activities: Vec::new(),
                error: run_error.clone(),
            }
        }
    };

    // 10. Assemble trace
    let mut activities: Vec<ActivityTrace> = Vec::with_capacity(workflow_output.activities.len());
    let mut total_cost_eur: f64 = 0.0;
    let mut total_tokens = TokenUsage::default();
    let mut cursor = started_at;

    let assignment_by_id: std::collections::HashMap<&str, &planner::StepAssignment> = assignments
        .iter()
        .map(|a| (a.activity_id.as_str(), a))
        .collect();

    for summary in &workflow_output.activities {
        progress.on_step_start(summary);
    }

    for summary in workflow_output.activities {
        let step_started_at = summary.started_at.unwrap_or(cursor);
        let duration_ms = summary.duration_ms as u128;
        let step_ended_at = summary.ended_at.unwrap_or_else(|| {
            step_started_at
                + chrono::Duration::milliseconds(summary.duration_ms.min(i64::MAX as u64) as i64)
        });
        cursor = step_ended_at;

        if let Some(c) = summary.cost_eur {
            total_cost_eur += c;
        }
        if let Some(u) = summary.usage {
            total_tokens = total_tokens + u;
        }

        progress.on_step_finish(&summary);

        let note = summary.notes.first().cloned();

        let (task_queue_field, worker_identity_field) =
            match assignment_by_id.get(summary.activity_id.as_str()) {
                Some(a) => (
                    Some(a.task_queue.clone()),
                    identity_from_queue(&a.task_queue),
                ),
                None => (None, None),
            };

        activities.push(ActivityTrace {
            activity_id: summary.activity_id,
            step_name: summary.step_name,
            kind: summary.kind,
            status: summary.status,
            started_at: step_started_at,
            ended_at: step_ended_at,
            duration_ms,
            attempts: summary.attempts,
            route: summary.route,
            task_queue: task_queue_field,
            worker_identity: worker_identity_field,
            input_summary: summarize(&summary.input),
            output_summary: summarize(&summary.output),
            output: summary.output,
            cost_eur: summary.cost_eur,
            tokens: summary.usage,
            error: summary.error,
            notes: note,
        });
    }

    if workflow_output.status == "failed" && run_status != "failed" {
        run_status = "failed".to_string();
        run_error = workflow_output.error;
    }

    let ended_at = Utc::now();
    let total = start_instant.elapsed();
    let trace = RunTrace {
        run_id: run_id.clone(),
        workflow_id: loaded.compiled.manifest.id.clone(),
        workflow_content_hash: Some(loaded.content_hash.clone()),
        status: run_status,
        trigger: trigger.as_str().to_string(),
        dry_run,
        requesting_identity: Some(identity.clone()),
        started_at,
        ended_at,
        duration_ms: total.as_millis(),
        source: Some(loaded.source.clone()),
        params,
        activities,
        cost: CostSummary {
            total_eur: total_cost_eur,
            input_tokens: total_tokens.input_tokens,
            output_tokens: total_tokens.output_tokens,
        },
        error: run_error,
    };

    persist_trace(&loaded, started_at, &trace)?;
    Ok(trace)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

pub fn new_run_id() -> String {
    format!("run_{}", uuid::Uuid::new_v4().simple())
}

pub fn persist_trace(
    loaded: &workflow_loader::LoadedWorkflow,
    started_at: chrono::DateTime<Utc>,
    trace: &RunTrace,
) -> Result<()> {
    let key = workflow_loader::loaded_run_history_key(loaded);
    let dir = paths::runs_dir()?.join(&key);
    std::fs::create_dir_all(&dir).with_context(|| format!("creating `{}`", dir.display()))?;

    let filename = format!("{}.json", started_at.format("%Y-%m-%dT%H-%M-%SZ"));
    let path = dir.join(&filename);

    let bytes = serde_json::to_vec_pretty(trace).context("serializing run trace")?;
    let tmp = dir.join(format!(".tmp-{}-{}", std::process::id(), filename));
    std::fs::write(&tmp, &bytes).with_context(|| format!("writing `{}`", tmp.display()))?;
    std::fs::rename(&tmp, &path).with_context(|| format!("renaming into `{}`", path.display()))?;
    Ok(())
}

/// Read LLM keys from `~/.cori/config.toml` and overlay env vars on top.
pub fn resolve_llm_credentials() -> LlmCredentials {
    let mut from_config = LlmCredentials::default();
    if let Ok(cfg) = config::Config::load() {
        from_config.openai_api_key = cfg
            .get("llm.openai.api_key")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        from_config.anthropic_api_key = cfg
            .get("llm.anthropic.api_key")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        from_config.gemini_api_key = cfg
            .get("llm.gemini.api_key")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
    }
    LlmCredentials::from_env().or_fill_from(&from_config)
}

/// Build the initial params JSON from manifest defaults overlaid with
/// CLI `key=value` arguments.
pub fn build_initial_input(
    workflow: &cori_protocol::CompiledWorkflow,
    cli_args: &[String],
) -> Result<JsonValue> {
    let mut obj: JsonMap<String, JsonValue> = JsonMap::new();

    for param in &workflow.manifest.parameters {
        if let Some(default) = &param.default {
            let v = serde_json::to_value(default)
                .with_context(|| format!("decoding default for parameter `{}`", param.name))?;
            obj.insert(param.name.clone(), v);
        }
    }

    for raw in cli_args {
        let (k, v) = match raw.split_once('=') {
            Some(kv) => kv,
            None => bail!("argument `{raw}` is not in `key=value` form"),
        };
        if k.is_empty() {
            bail!("argument `{raw}` has an empty key");
        }
        obj.insert(k.to_string(), parse_arg_value(v));
    }
    Ok(JsonValue::Object(obj))
}

pub fn parse_arg_value(s: &str) -> JsonValue {
    if let Ok(v) = serde_json::from_str::<JsonValue>(s) {
        v
    } else {
        JsonValue::String(s.to_string())
    }
}

fn summarize(v: &JsonValue) -> JsonValue {
    match v {
        JsonValue::Array(a) => serde_json::json!({ "type": "array", "len": a.len() }),
        JsonValue::Object(o) => serde_json::json!({
            "type": "object",
            "keys": o.keys().cloned().collect::<Vec<_>>(),
        }),
        JsonValue::String(s) if s.len() > 200 => serde_json::json!({
            "type": "string",
            "len": s.len(),
            "preview": s.chars().take(120).collect::<String>(),
        }),
        other => other.clone(),
    }
}

#[allow(dead_code)]
fn kind_label(kind: StepKind) -> &'static str {
    match kind {
        StepKind::Cli => "cli",
        StepKind::McpTool => "mcp_tool",
        StepKind::Code => "code",
        StepKind::Llm => "llm",
        StepKind::Builtin => "builtin",
    }
}
