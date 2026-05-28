//! `cori run <path> [--dry-run] [--json] [<param>=<value>...]`.
//!
//! Pipeline (Phase 2 of the redesign):
//!
//! 1. Resolve the workflow folder (`workflow_loader::load`).
//! 2. Apply manifest parameter defaults + CLI `key=value` overrides.
//! 3. Lazy-install the Deno runtime under `~/.cori/runtime/`.
//! 4. Discover + validate broker capabilities.
//! 5. Resolve the Temporal endpoint (config / preflight / auto-spawn).
//! 6. Connect to Temporal, register an ephemeral in-process worker on
//!    `task_queue_for(&Person { user_id })` (Phase 3). Phase 4 will
//!    move queue selection per-step.
//! 7. Start the workflow, await its result.
//! 8. Serialize the trace as JSON to
//!    `~/.cori/runs/<run_history_key>/<utc>.json`, atomically.
//! 9. Print a friendly summary or the JSON trace.

use std::time::Instant;

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use cori_broker::capabilities::{self, Capabilities, CapabilityReport};
use cori_broker::identity::{IdentitySource, OsUser};
use cori_broker::llm::{LlmCredentials, LlmOptions};
use cori_broker::{TokenUsage, TriggerContext, runtime as broker_runtime};
use cori_protocol::{
    CompiledWorkflow, StepKind, WorkerIdentity, identity_from_queue, task_queue_for,
};
use cori_worker::broker_ctx::{BrokerCtx, set_broker_ctx};
use cori_worker::runner::run_workflow_once;
use cori_worker::runtime::{CoriTemporalRuntime, DEFAULT_NAMESPACE, preflight_check};
use cori_worker::workflow::{ActivitySummary, WorkflowInput, WorkflowOutput};
use serde::{Deserialize, Serialize};
use serde_json::{Map as JsonMap, Value as JsonValue};

use crate::remote::{self, WorkflowSource};
use crate::{
    config::Config, paths, planner, runtime as cli_runtime, temporal_endpoint, workflow_loader,
};

// Phase 3: the requesting user's identity derives the task queue. A
// solo dev with no `cori work` running still works — `run` spins up an
// ephemeral in-process worker on `cori.user.<id>`. Phase 4 will move
// queue selection per-step.

// ---------------------------------------------------------------------------
// Trace types (persisted to ~/.cori/runs/<key>/<utc>.json)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct RunTrace {
    pub(crate) run_id: String,
    pub(crate) workflow_id: String,
    /// 16-hex-char hash of the workflow folder contents at run time.
    /// Replaces the old `workflow_version` (no registry → no version).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) workflow_content_hash: Option<String>,
    pub(crate) status: String,
    pub(crate) trigger: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) dry_run: bool,
    /// Identity of the user who started this run. Routed steps inherit
    /// this for credential lookup; recorded in the trace so `cori
    /// runs show` can attribute each run. Phase 7 addition.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) requesting_identity: Option<WorkerIdentity>,
    pub(crate) started_at: DateTime<Utc>,
    pub(crate) ended_at: DateTime<Utc>,
    pub(crate) duration_ms: u128,
    /// Origin of the workflow: local path or remote git ref + sha.
    /// Added by the remote-workflows feature.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) source: Option<WorkflowSource>,
    pub(crate) params: JsonValue,
    pub(crate) activities: Vec<ActivityTrace>,
    pub(crate) cost: CostSummary,
    pub(crate) error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct CostSummary {
    pub(crate) total_eur: f64,
    pub(crate) input_tokens: u64,
    pub(crate) output_tokens: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ActivityTrace {
    pub(crate) activity_id: String,
    pub(crate) step_name: String,
    pub(crate) kind: StepKind,
    pub(crate) status: String,
    pub(crate) started_at: DateTime<Utc>,
    pub(crate) ended_at: DateTime<Utc>,
    pub(crate) duration_ms: u128,
    pub(crate) attempts: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) route: Option<String>,
    /// Task queue the activity was dispatched to (Phase 4 routing).
    /// Phase 7 records it so traces document where each step actually
    /// ran. `None` for legacy traces.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) task_queue: Option<String>,
    /// Worker identity derived from `task_queue` (Phase 7). `None` for
    /// legacy traces or unrecognised queue names.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) worker_identity: Option<WorkerIdentity>,
    pub(crate) input_summary: JsonValue,
    pub(crate) output_summary: JsonValue,
    pub(crate) output: JsonValue,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) cost_eur: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) tokens: Option<TokenUsage>,
    pub(crate) error: Option<String>,
    pub(crate) notes: Option<String>,
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

pub fn run(
    path: String,
    params: Vec<String>,
    json_out: bool,
    dry_run_mode: bool,
    update: bool,
    assume_yes: bool,
) -> Result<()> {
    let (resolved, mut loaded) = workflow_loader::resolve_arg(&path, update)?;

    // Consent gate (remote only): runs after compilation (cheap, no
    // user code) but before any broker / Temporal / network work.
    if let Some(rr) = resolved.remote.as_ref()
        && !remote::trust::is_trusted(&rr.spec, &rr.sha)?
    {
        let auto_yes = assume_yes || remote::trust::assume_yes_env();
        let agreed = if auto_yes {
            true
        } else {
            remote::trust::prompt_consent(
                &rr.spec,
                &rr.sha,
                &loaded.absolute_path,
                &loaded.compiled,
            )?
        };
        if !agreed {
            bail!("consent declined; not running remote workflow");
        }
        remote::trust::record_consent(
            &rr.spec,
            &rr.sha,
            remote::trust::declared_capability_strings(&loaded.compiled),
        )?;
    }

    let initial_params = build_initial_input(&loaded.compiled, &params)?;

    cli_runtime::ensure_installed()?;
    let runtime_root = paths::runtime_dir()?;
    let runtime = broker_runtime::Runtime::resolve(&runtime_root).map_err(|e| {
        anyhow::anyhow!(
            "{e}\n\nIf you have Deno installed, you can also point Cori at it with:\n  \
             export CORI_DENO=$(which deno)"
        )
    })?;

    let credentials = resolve_llm_credentials();
    let llm_opts = LlmOptions {
        credentials: credentials.clone(),
        trigger: Some(TriggerContext::Cli),
    };

    let home = paths::home()?;
    let caps = capabilities::discover(&home, &loaded.compiled.required_cli_binaries, &credentials);

    if !json_out {
        print_capability_banner(&caps, &loaded.compiled);
        if dry_run_mode {
            println!("DRY RUN — no external calls (cli/mcp_tool/llm steps return mocked output)");
        }
        if loaded.from_cache {
            tracing::debug!("compiled workflow loaded from cache");
        }
    }

    let missing = capabilities::validate(
        &caps,
        &loaded.compiled.required_cli_binaries,
        &loaded.compiled.required_mcp_servers,
        &loaded.compiled.required_llm_providers,
    );
    if !missing.is_empty() {
        if dry_run_mode {
            if !json_out {
                eprintln!(
                    "ℹ workflow `{}` is missing capabilities; --dry-run ignores them:",
                    loaded.folder_name
                );
                for m in &missing {
                    eprintln!("  · {m}");
                }
            }
        } else {
            if !json_out {
                eprintln!(
                    "✗ workflow `{}` requires capabilities that are not available:",
                    loaded.folder_name
                );
                for m in &missing {
                    eprintln!("  · {m}");
                }
            }
            bail!("missing capabilities — install the listed tools and try again");
        }
    }

    // Phase 6 preflight: presence is necessary but not sufficient —
    // CLI / OAuth tokens may have expired since `cori login`. Probe
    // authed state via the local capability report and bail with the
    // same actionable lines `cori check` prints.
    if !dry_run_mode {
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
            if !json_out {
                eprintln!(
                    "✗ workflow `{}` has capabilities that need sign-in:",
                    loaded.folder_name
                );
                for id in &needs_login {
                    eprintln!("  · `{id}` needs sign-in — run: cori login {id}");
                }
            }
            bail!("capabilities need sign-in — run `cori login <id>` and try again");
        }
    }

    let endpoint = temporal_endpoint::resolve()?;
    if let Err(e) = preflight_check(&endpoint.target, std::time::Duration::from_millis(500)) {
        if !json_out {
            eprintln!("✗ Temporal not reachable at {}", endpoint.target);
            for line in format!("{e:#}").lines() {
                eprintln!("  {line}");
            }
        }
        bail!("Temporal server unavailable");
    }

    // Identity of the requesting user → task queue for this run.
    let identity = OsUser
        .resolve()
        .context("resolving OS user identity for this run")?;
    let user_id = match &identity {
        WorkerIdentity::Person { user_id } => user_id.clone(),
        // `OsUser` only ever returns `Person`; unreachable in practice.
        WorkerIdentity::Service { pool } => pool.clone(),
    };
    let task_queue = task_queue_for(&identity);

    // Planner (Phase 4): assign each step's `task_queue` from its
    // declared `Placement` and the cluster view. The ephemeral worker
    // this process spins up always polls `task_queue`, so we register
    // it as a self-report so the planner prefers it for `Anywhere`
    // steps and any capabilities it advertises locally.
    let mut cluster = planner::ClusterView::load().unwrap_or_default();
    let self_report = CapabilityReport::from_capabilities_with(
        identity.clone(),
        &caps,
        Some(&paths::credentials_dir()?),
    );
    cluster.add_self(self_report);
    let assignments = match planner::assign_queues(&mut loaded.compiled, &identity, &cluster) {
        Ok(a) => a,
        Err(e) => {
            if !json_out {
                eprintln!("✗ cannot plan workflow `{}`:", loaded.folder_name);
                eprintln!("  · {e}");
            }
            bail!("placement failed");
        }
    };
    if !json_out {
        print_plan_summary(&assignments);
    }

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

    if !json_out {
        println!(
            "Running {} (content {})…",
            loaded.compiled.manifest.id,
            &loaded.content_hash[..8.min(loaded.content_hash.len())]
        );
    }

    let tokio_rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("starting tokio runtime for Temporal worker")?;

    let workflow_input = WorkflowInput {
        workflow_id: loaded.compiled.manifest.id.clone(),
        workflow_content_hash: Some(loaded.content_hash.clone()),
        user_id: user_id.clone(),
        compiled_dag: loaded.compiled.clone(),
        user_params: initial_params.clone(),
        dry_run: dry_run_mode,
        reauth_timeout_secs: std::env::var("CORI_REAUTH_TIMEOUT_SECS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok()),
    };

    let workflow_output_res: Result<WorkflowOutput> = tokio_rt.block_on(async {
        let temporal_rt = CoriTemporalRuntime::connect(
            endpoint.target.clone(),
            DEFAULT_NAMESPACE,
            task_queue.clone(),
        )
        .await?;
        run_workflow_once(&temporal_rt, run_id.clone(), workflow_input).await
    });

    let mut run_status: String = "succeeded".to_string();
    let mut run_error: Option<String> = None;
    let workflow_output = match workflow_output_res {
        Ok(o) => o,
        Err(e) => {
            run_status = "failed".to_string();
            run_error = Some(format!("{e:#}"));
            WorkflowOutput {
                run_id: run_id.clone(),
                status: "failed".to_string(),
                final_output: JsonValue::Null,
                activities: Vec::new(),
                error: run_error.clone(),
            }
        }
    };

    let mut activities: Vec<ActivityTrace> = Vec::with_capacity(workflow_output.activities.len());
    let mut total_cost_eur: f64 = 0.0;
    let mut total_tokens = TokenUsage::default();
    let mut cursor = started_at;

    // Phase 7: enrich each activity row with the queue/worker it was
    // dispatched to. The planner already assigned task_queue per step;
    // we map back by `activity_id`.
    let assignment_by_id: std::collections::HashMap<&str, &planner::StepAssignment> = assignments
        .iter()
        .map(|a| (a.activity_id.as_str(), a))
        .collect();

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

        if !json_out {
            print_step_summary(&summary);
        }

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
        status: run_status.clone(),
        trigger: "cli".to_string(),
        dry_run: dry_run_mode,
        requesting_identity: Some(identity.clone()),
        started_at,
        ended_at,
        duration_ms: total.as_millis(),
        source: Some(loaded.source.clone()),
        params: initial_params,
        activities,
        cost: CostSummary {
            total_eur: total_cost_eur,
            input_tokens: total_tokens.input_tokens,
            output_tokens: total_tokens.output_tokens,
        },
        error: run_error.clone(),
    };

    persist_trace(&loaded, started_at, &trace)?;

    if json_out {
        println!("{}", serde_json::to_string_pretty(&trace)?);
    } else if let Some(err) = &run_error {
        eprintln!("\n✗ run failed: {err}");
    } else {
        print_final_output(&trace);
    }

    if run_status == "failed" {
        std::process::exit(1);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

pub(crate) fn new_run_id() -> String {
    format!("run_{}", uuid::Uuid::new_v4().simple())
}

fn persist_trace(
    loaded: &workflow_loader::LoadedWorkflow,
    started_at: DateTime<Utc>,
    trace: &RunTrace,
) -> Result<()> {
    let key = workflow_loader::loaded_run_history_key(loaded);
    let dir = paths::runs_dir()?.join(&key);
    std::fs::create_dir_all(&dir).with_context(|| format!("creating `{}`", dir.display()))?;

    // Windows-friendly filename: 2026-05-28T14-22-01Z.json
    let filename = format!("{}.json", started_at.format("%Y-%m-%dT%H-%M-%SZ"));
    let path = dir.join(&filename);

    let bytes = serde_json::to_vec_pretty(trace).context("serializing run trace")?;
    let tmp = dir.join(format!(".tmp-{}-{}", std::process::id(), filename));
    std::fs::write(&tmp, &bytes).with_context(|| format!("writing `{}`", tmp.display()))?;
    std::fs::rename(&tmp, &path).with_context(|| format!("renaming into `{}`", path.display()))?;
    Ok(())
}

fn print_plan_summary(assignments: &[planner::StepAssignment]) {
    use cori_protocol::Placement;
    let mut by_queue: std::collections::BTreeMap<&str, Vec<&str>> = Default::default();
    for a in assignments {
        by_queue
            .entry(a.task_queue.as_str())
            .or_default()
            .push(a.step_name.as_str());
    }
    let one_queue = by_queue.len() == 1;
    if one_queue {
        // Solo / single-queue case: don't clutter the banner.
        return;
    }
    println!("Plan:");
    for a in assignments {
        let p = match &a.placement {
            Placement::Anywhere => "anywhere".to_string(),
            Placement::RequiresLocalFs => "local_fs".to_string(),
            Placement::RequiresCapability { id } => format!("needs:{id}"),
        };
        println!("  · {} ({}) → {}", a.step_name, p, a.task_queue);
    }
}

fn print_capability_banner(caps: &Capabilities, workflow: &CompiledWorkflow) {
    let cli_names: Vec<String> = workflow
        .required_cli_binaries
        .iter()
        .map(|b| {
            if caps.has_cli(b) {
                format!("{b} ✓")
            } else {
                format!("{b} ✗")
            }
        })
        .collect();
    let mcp_names: Vec<String> = workflow
        .required_mcp_servers
        .iter()
        .map(|s| {
            if caps.has_mcp(s) {
                format!("{s} ✓")
            } else {
                format!("{s} ✗")
            }
        })
        .collect();
    println!(
        "Capabilities: CLIs [{}], MCP [{}]",
        if cli_names.is_empty() {
            "—".into()
        } else {
            cli_names.join(", ")
        },
        if mcp_names.is_empty() {
            "—".into()
        } else {
            mcp_names.join(", ")
        },
    );
}

fn print_step_summary(summary: &ActivitySummary) {
    let cost = match summary.cost_eur {
        Some(c) if c > 0.0 => format!(", €{c:.4}"),
        _ => String::new(),
    };
    let marker = match summary.status.as_str() {
        "ok" => "✓",
        "skipped" if summary.notes.iter().any(|n| n.contains("mocked")) => "∘",
        "skipped" => "·",
        "failed" => "✗",
        _ => "·",
    };
    let suffix = if summary.notes.iter().any(|n| n.contains("mocked")) {
        " [mocked]"
    } else {
        ""
    };
    println!(
        "{marker} {name} ({kind}, {ms}ms{cost}){suffix}",
        name = summary.step_name,
        kind = kind_label(summary.kind),
        ms = summary.duration_ms,
    );
    if let Some(err) = &summary.error {
        eprintln!("  error: {err}");
    }
}

fn print_final_output(trace: &RunTrace) {
    let final_output = trace
        .activities
        .iter()
        .rev()
        .find(|a| a.status == "ok")
        .map(|a| &a.output)
        .cloned()
        .unwrap_or(JsonValue::Null);
    let pretty = serde_json::to_string(&final_output).unwrap_or_else(|_| final_output.to_string());
    println!("Output: {pretty}");
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

/// Read LLM keys from `~/.cori/config.toml` and overlay env vars on top.
pub(crate) fn resolve_llm_credentials() -> LlmCredentials {
    let mut from_config = LlmCredentials::default();
    if let Ok(cfg) = Config::load() {
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

/// Build the input to step 1 from manifest defaults overlaid with CLI
/// `key=value` arguments.
fn build_initial_input(workflow: &CompiledWorkflow, cli_args: &[String]) -> Result<JsonValue> {
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

fn parse_arg_value(s: &str) -> JsonValue {
    if let Ok(v) = serde_json::from_str::<JsonValue>(s) {
        v
    } else {
        JsonValue::String(s.to_string())
    }
}

/// Compact summary of a JSON value for inclusion in trace
/// `input_summary` / `output_summary` fields.
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_json_and_string_args() {
        assert_eq!(parse_arg_value("12"), json!(12));
        assert_eq!(parse_arg_value("true"), json!(true));
        assert_eq!(parse_arg_value("\"hi\""), json!("hi"));
        assert_eq!(parse_arg_value("hi"), json!("hi"));
        assert_eq!(parse_arg_value("[1,2]"), json!([1, 2]));
    }
}
