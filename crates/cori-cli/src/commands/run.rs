//! `cori run <id> [--dry-run] [<param>=<value>...]` — Phase 6.
//!
//! Executes a registered workflow step-by-step. `code`, `cli`, `mcp_tool`,
//! and `llm` steps are dispatched via the broker; `builtin` steps remain
//! unimplemented (deferred) and short-circuit with a clear notice.
//!
//! `--dry-run` (Phase 6 §6.3): the entire pipeline runs except that every
//! step that would touch the outside world returns a placeholder annotated
//! with `mocked: true`. `code` and `builtin` steps still execute for real
//! since they have no external side effects. The trace marks the run with
//! a top-level `dry_run: true` field and every mocked activity status is
//! `skipped`.
//!
//! Parameter resolution:
//! - Manifest defaults are applied first.
//! - CLI arguments of the form `key=value` override the defaults. JSON-shaped
//!   values (`true`, numbers, `null`, `[...]`, `{...}`, `"..."`) are decoded
//!   as such; everything else is treated as a string.
//! - Every step receives `{ ...initialParams, ...accumulatedOutputs }`.
//!
//! Output:
//! - With `--json`, the run trace is emitted as pretty-printed JSON,
//!   matching the canonical schema in `skill/references/trace_interpretation.md`.
//! - Otherwise a friendly per-step summary is printed.

use std::path::PathBuf;
use std::time::Instant;

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use cori_broker::capabilities::{self, Capabilities};
use cori_broker::llm::{LlmCredentials, LlmOptions};
use cori_broker::{runtime, TokenUsage, TriggerContext};
use cori_protocol::{CompiledWorkflow, StepKind};
use cori_worker::broker_ctx::{set_broker_ctx, BrokerCtx};
use cori_worker::runner::run_workflow_once;
use cori_worker::runtime::{
    preflight_check, CoriTemporalRuntime, DEFAULT_NAMESPACE, DEFAULT_TASK_QUEUE,
    DEFAULT_TEMPORAL_TARGET,
};
use cori_worker::workflow::{WorkflowInput, WorkflowOutput};
use serde::Serialize;
use serde_json::{Map as JsonMap, Value as JsonValue};

use crate::{config::Config, paths, registry};

#[derive(Debug, Clone, Serialize)]
pub(crate) struct RunTrace {
    run_id: String,
    workflow_id: String,
    workflow_version: u32,
    status: &'static str,
    trigger: &'static str,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    dry_run: bool,
    started_at: DateTime<Utc>,
    ended_at: DateTime<Utc>,
    duration_ms: u128,
    params: JsonValue,
    activities: Vec<ActivityTrace>,
    cost: CostSummary,
    error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Default)]
struct CostSummary {
    total_eur: f64,
    input_tokens: u64,
    output_tokens: u64,
}

#[derive(Debug, Clone, Serialize)]
struct ActivityTrace {
    activity_id: String,
    step_name: String,
    kind: StepKind,
    status: &'static str,
    started_at: DateTime<Utc>,
    ended_at: DateTime<Utc>,
    duration_ms: u128,
    attempts: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    route: Option<String>,
    input_summary: JsonValue,
    output_summary: JsonValue,
    output: JsonValue,
    #[serde(skip_serializing_if = "Option::is_none")]
    cost_eur: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tokens: Option<TokenUsage>,
    error: Option<String>,
    notes: Option<String>,
}

/// Outcome of `execute_workflow` — used by both the CLI verb and the
/// HTTP server (`cori serve start`).
#[allow(dead_code)] // `run_id` mirrors what the caller passed in.
pub(crate) struct RunResult {
    pub run_id: String,
    pub trace: RunTrace,
    pub status: &'static str,
}

/// Generate a fresh run id. Exposed so the HTTP server can return it to
/// the client before `execute_workflow` actually starts.
pub(crate) fn new_run_id() -> String {
    format!("run_{}", uuid_like())
}

/// Execute one workflow run, recording the trace into the registry.
///
/// `verbose=true` prints per-step status to stdout (CLI behaviour);
/// `verbose=false` is silent (HTTP server behaviour). The returned
/// `RunResult` always contains the same trace that was persisted.
pub(crate) fn execute_workflow(
    workflow_id: &str,
    initial_params: JsonValue,
    dry_run_mode: bool,
    verbose: bool,
    run_id: Option<String>,
) -> Result<RunResult> {
    let mut reg = registry::open()?;
    let Some(detail) = reg.get(workflow_id)? else {
        bail!(
            "no workflow with id `{workflow_id}`. Run `cori workflows list` to see registered workflows."
        );
    };
    let compiled = detail.compiled;
    let source_path = PathBuf::from(&detail.source_path);
    if !source_path.is_dir() {
        bail!(
            "registered source path `{}` no longer exists — re-run `cori workflows register {}`",
            source_path.display(),
            workflow_id
        );
    }

    // Resolve the Deno runtime once.
    let runtime_root = paths::runtime_dir()?;
    let runtime = runtime::Runtime::resolve(&runtime_root).map_err(|e| {
        anyhow::anyhow!(
            "{e}\n\nIf you have Deno installed, you can also point Cori at it with:\n  export CORI_DENO=$(which deno)"
        )
    })?;

    // Resolve LLM credentials: env > ~/.cori/config.toml.
    let credentials = resolve_llm_credentials()?;
    let llm_opts = LlmOptions {
        credentials: credentials.clone(),
        trigger: Some(TriggerContext::Cli),
    };

    // Discover + validate capabilities before running anything.
    let home = paths::home()?;
    let caps = capabilities::discover(&home, &compiled.required_cli_binaries, &credentials);
    let missing = capabilities::validate(
        &caps,
        &compiled.required_cli_binaries,
        &compiled.required_mcp_servers,
        &compiled.required_llm_providers,
    );

    if verbose {
        print_capability_banner(&caps, &compiled);
        if dry_run_mode {
            println!("DRY RUN — no external calls (cli/mcp_tool/llm steps return mocked output)");
        }
    }

    if !missing.is_empty() {
        if dry_run_mode {
            if verbose {
                eprintln!(
                    "ℹ workflow `{workflow_id}` is missing capabilities; --dry-run ignores them:"
                );
                for m in &missing {
                    eprintln!("  · {m}");
                }
            }
        } else {
            if verbose {
                eprintln!(
                    "✗ workflow `{workflow_id}` requires capabilities that are not available:"
                );
                for m in &missing {
                    eprintln!("  · {m}");
                }
            }
            bail!("missing capabilities — install the listed tools and try again");
        }
    }

    // Pre-flight: verify the Temporal dev server is reachable before we
    // record a placeholder "running" row. This turns the buried tonic
    // transport error into a clean actionable message and avoids
    // littering the ledger with failed-before-it-started runs.
    let temporal_target = std::env::var("CORI_TEMPORAL_TARGET")
        .unwrap_or_else(|_| DEFAULT_TEMPORAL_TARGET.to_string());
    if let Err(e) = preflight_check(&temporal_target, std::time::Duration::from_millis(500)) {
        if verbose {
            eprintln!("✗ Temporal not reachable at {temporal_target}");
            for line in format!("{e:#}").lines() {
                eprintln!("  {line}");
            }
        }
        bail!("Temporal server unavailable — start it with `temporal server start-dev`");
    }

    let run_id = run_id.unwrap_or_else(new_run_id);
    let started_at = Utc::now();
    let start_instant = Instant::now();

    // Insert a placeholder "running" row so callers polling
    // `cori runs show <run_id>` (and the HTTP API) can see the run mid-flight.
    let placeholder = serde_json::json!({
        "run_id": run_id,
        "workflow_id": compiled.manifest.id,
        "workflow_version": detail.version,
        "status": "running",
        "trigger": "cli",
        "dry_run": dry_run_mode,
        "started_at": started_at,
        "activities": [],
    });
    let _ = reg.record_run(
        &run_id,
        &compiled.manifest.id,
        started_at,
        None,
        "running",
        &placeholder.to_string(),
    );

    let mut activities: Vec<ActivityTrace> = Vec::with_capacity(compiled.steps.len());
    let mut last_output: JsonValue = JsonValue::Null;
    let mut run_status: &'static str = "succeeded";
    let mut run_error: Option<String> = None;
    let mut total_cost_eur: f64 = 0.0;
    let mut had_cost: bool = false;
    let mut total_tokens: TokenUsage = TokenUsage::default();

    if verbose {
        println!("Running {} (v{})…", compiled.manifest.id, detail.version);
    }

    // Set the process-wide broker context. Activities reach into this
    // when Temporal invokes them on the worker side.
    let broker_ctx = BrokerCtx {
        runtime: runtime.clone(),
        caps: caps.clone(),
        llm_opts: llm_opts.clone(),
        source_root: source_path.clone(),
    };
    // First-call wins. Subsequent runs in the same process reuse the
    // existing slot — that's intentional because the daemon also calls
    // this code path.
    let _ = set_broker_ctx(broker_ctx);

    // Build a fresh tokio runtime per `cori run`. The Temporal worker's
    // poll future is !Send and we don't want it to interleave with the
    // CLI's outer thread.
    let tokio_rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("starting tokio runtime for Temporal worker")?;

    let workflow_input = WorkflowInput {
        workflow_id: compiled.manifest.id.clone(),
        workflow_version: detail.version,
        compiled_dag: compiled.clone(),
        user_params: initial_params.clone(),
        dry_run: dry_run_mode,
    };

    let workflow_output_res: Result<WorkflowOutput> = tokio_rt.block_on(async {
        let temporal_rt = CoriTemporalRuntime::connect(
            temporal_target.clone(),
            DEFAULT_NAMESPACE,
            DEFAULT_TASK_QUEUE,
        )
        .await?;
        run_workflow_once(&temporal_rt, run_id.clone(), workflow_input).await
    });

    let workflow_output = match workflow_output_res {
        Ok(o) => o,
        Err(e) => {
            // Connectivity / startup failures show as a failed run.
            run_status = "failed";
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

    // Map workflow-level summaries into the user-facing trace rows.
    // Each `ActivitySummary` carries wall-clock timestamps recorded
    // inside the activity (the workflow body itself never reads a
    // clock). For builtin/failed steps without timestamps we fall back
    // to a cursor walked forward from the run's started_at.
    let mut cursor = started_at;
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
            had_cost = true;
        }
        if let Some(u) = summary.usage {
            total_tokens = total_tokens + u;
        }
        if summary.status == "ok" {
            last_output = summary.output.clone();
        }

        if verbose {
            print_step_summary(&summary);
        }

        let status_str: &'static str = match summary.status.as_str() {
            "ok" => "ok",
            "skipped" => "skipped",
            _ => "failed",
        };
        let note = summary.notes.first().cloned();

        activities.push(ActivityTrace {
            activity_id: summary.activity_id,
            step_name: summary.step_name,
            kind: summary.kind,
            status: status_str,
            started_at: step_started_at,
            ended_at: step_ended_at,
            duration_ms,
            attempts: summary.attempts,
            route: summary.route,
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
        run_status = "failed";
        run_error = workflow_output.error;
    }

    let ended_at = Utc::now();
    let total = start_instant.elapsed();
    let _ = had_cost;
    let trace = RunTrace {
        run_id: run_id.clone(),
        workflow_id: compiled.manifest.id.clone(),
        workflow_version: detail.version,
        status: run_status,
        trigger: "cli",
        dry_run: dry_run_mode,
        started_at,
        ended_at,
        duration_ms: total.as_millis(),
        params: initial_params,
        activities,
        cost: CostSummary {
            total_eur: total_cost_eur,
            input_tokens: total_tokens.input_tokens,
            output_tokens: total_tokens.output_tokens,
        },
        error: run_error.clone(),
    };

    let trace_json = serde_json::to_string(&trace).context("serializing run trace")?;
    if let Err(e) = reg.record_run(
        &run_id,
        &compiled.manifest.id,
        started_at,
        Some(ended_at),
        run_status,
        &trace_json,
    ) {
        if verbose {
            eprintln!("warning: could not store run trace: {e}");
        }
    }

    if verbose {
        println!();
        if run_status == "succeeded" {
            print_final_output(&last_output);
            if had_cost {
                println!(
                    "✓ run {run_id} completed in {ms}ms (cost: €{cost:.4})",
                    ms = total.as_millis(),
                    cost = total_cost_eur,
                );
            } else {
                println!("✓ run {run_id} completed in {ms}ms", ms = total.as_millis());
            }
        } else {
            println!("✗ run {run_id} failed after {ms}ms", ms = total.as_millis());
        }
    }

    Ok(RunResult {
        run_id,
        trace,
        status: run_status,
    })
}

/// CLI entry point — wraps `execute_workflow` with param parsing and
/// optional JSON-output rendering.
pub fn run(id: &str, params: Vec<String>, json_out: bool, dry_run_mode: bool) -> Result<()> {
    // Pre-parse params so a bad CLI invocation fails before we spin up
    // the Deno runtime or write a placeholder run row.
    let reg = registry::open()?;
    let Some(detail) = reg.get(id)? else {
        bail!("no workflow with id `{id}`. Run `cori workflows list` to see registered workflows.");
    };
    let initial_params = build_initial_input(&detail.compiled, &params)?;
    drop(reg);

    let result = execute_workflow(id, initial_params, dry_run_mode, !json_out, None)?;
    if json_out {
        println!("{}", serde_json::to_string_pretty(&result.trace)?);
    }
    if result.status == "failed" {
        std::process::exit(1);
    }
    Ok(())
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

fn print_step_summary(summary: &cori_worker::workflow::ActivitySummary) {
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

/// Read LLM keys from `~/.cori/config.toml` and overlay env vars on top.
pub(crate) fn resolve_llm_credentials() -> Result<LlmCredentials> {
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
    Ok(LlmCredentials::from_env().or_fill_from(&from_config))
}

fn print_final_output(value: &JsonValue) {
    let pretty = serde_json::to_string(value).unwrap_or_else(|_| value.to_string());
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
        let value = parse_arg_value(v);
        obj.insert(k.to_string(), value);
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

/// Compact summary of a JSON value for inclusion in trace `input_summary`
/// / `output_summary` fields. Mirrors the same shape used by
/// `commands::runs::show`.
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

/// Tiny RFC-4122 v4-ish id without pulling in a uuid dep.
fn uuid_like() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let pid = std::process::id() as u128;
    let mixed = nanos.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(pid);
    format!("{mixed:032x}")
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
