//! `cori run <path> [--dry-run] [--json] [<param>=<value>...]`.
//!
//! Thin wrapper over `cori-run::run_workflow`. All orchestration logic
//! lives in `cori-run`; this module handles the CLI-specific
//! presentation (printing, process exit) and passes callbacks for
//! consent + progress.

use std::sync::Arc;

use anyhow::Result;
use cori_protocol::RunTrace;
use serde_json::Value as JsonValue;

/// Re-export for commands that still call this helper (status, work, check).
pub use cori_run::resolve_llm_credentials;

pub fn run(
    path: String,
    params: Vec<String>,
    json_out: bool,
    dry_run_mode: bool,
    update: bool,
    assume_yes: bool,
) -> Result<()> {
    // Preflight: load the workflow to build params from manifest defaults
    let pf = cori_run::preflight(&path, update, assume_yes)?;
    let initial_params = cori_run::build_initial_input(&pf.loaded.compiled, &params)?;

    if !json_out {
        print_capability_banner(&pf.caps, &pf.loaded.compiled);
        if dry_run_mode {
            println!("DRY RUN — no external calls (cli/mcp_tool/llm steps return mocked output)");
        }
        if pf.loaded.from_cache {
            tracing::debug!("compiled workflow loaded from cache");
        }
    }

    let consent = if assume_yes || cori_run::remote::trust::assume_yes_env() {
        cori_run::ConsentCallback::AssumeYes
    } else {
        cori_run::ConsentCallback::Prompt(Box::new(|prompt| {
            match cori_run::remote::trust::prompt_consent(
                prompt.spec,
                prompt.sha,
                prompt.workflow_dir,
                prompt.compiled,
            ) {
                Ok(true) => cori_run::ConsentDecision::Granted,
                Ok(false) => cori_run::ConsentDecision::Denied,
                Err(_) => cori_run::ConsentDecision::Denied,
            }
        }))
    };

    let progress: Arc<dyn cori_run::ProgressSink> = if json_out {
        Arc::new(cori_run::NoopSink)
    } else {
        Arc::new(CliProgressSink)
    };

    let tokio_rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| anyhow::anyhow!("starting tokio runtime: {e}"))?;

    let trace = tokio_rt.block_on(cori_run::run_workflow(
        cori_run::RunRequest {
            source: path,
            params: initial_params,
            dry_run: dry_run_mode,
            update,
            trigger: cori_run::Trigger::Cli,
            run_id: None,
        },
        consent,
        progress,
    ))?;

    if json_out {
        println!("{}", serde_json::to_string_pretty(&trace)?);
    } else if let Some(err) = &trace.error {
        eprintln!("\n✗ run failed: {err}");
        print_final_output(&trace);
    } else {
        print_final_output(&trace);
    }

    if trace.status == "failed" {
        std::process::exit(1);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// CLI-specific progress sink
// ---------------------------------------------------------------------------

struct CliProgressSink;

impl cori_run::ProgressSink for CliProgressSink {
    fn on_plan(&self, plan: &[cori_run::planner::StepAssignment]) {
        print_plan_summary(plan);
    }

    fn on_step_start(&self, _summary: &cori_worker::workflow::ActivitySummary) {}

    fn on_step_finish(&self, summary: &cori_worker::workflow::ActivitySummary) {
        print_step_summary(summary);
    }
}

// ---------------------------------------------------------------------------
// Display helpers
// ---------------------------------------------------------------------------

fn print_plan_summary(assignments: &[cori_run::planner::StepAssignment]) {
    use cori_protocol::Placement;
    let mut by_queue: std::collections::BTreeMap<&str, Vec<&str>> = Default::default();
    for a in assignments {
        by_queue
            .entry(a.task_queue.as_str())
            .or_default()
            .push(a.step_name.as_str());
    }
    if by_queue.len() <= 1 {
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

fn print_capability_banner(
    caps: &cori_broker::capabilities::Capabilities,
    workflow: &cori_protocol::CompiledWorkflow,
) {
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

fn kind_label(kind: cori_protocol::StepKind) -> &'static str {
    match kind {
        cori_protocol::StepKind::Cli => "cli",
        cori_protocol::StepKind::McpTool => "mcp_tool",
        cori_protocol::StepKind::Code => "code",
        cori_protocol::StepKind::Llm => "llm",
        cori_protocol::StepKind::Builtin => "builtin",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_json_and_string_args() {
        use serde_json::json;
        assert_eq!(cori_run::parse_arg_value("12"), json!(12));
        assert_eq!(cori_run::parse_arg_value("true"), json!(true));
        assert_eq!(cori_run::parse_arg_value("\"hi\""), json!("hi"));
        assert_eq!(cori_run::parse_arg_value("hi"), json!("hi"));
        assert_eq!(cori_run::parse_arg_value("[1,2]"), json!([1, 2]));
    }
}
