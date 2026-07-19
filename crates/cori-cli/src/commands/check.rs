//! `cori check <path>` — Phase 6 preflight.

use anyhow::{Context, Result};
use cori_broker::capabilities::{self, Capabilities, Capability, CapabilityKind, CapabilityReport};
use cori_broker::identity::{IdentitySource, OsUser};
use cori_protocol::{CompiledWorkflow, Placement, StepKind, WorkerIdentity, task_queue_for};

use cori_run::remote;
use cori_run::{paths, planner, runtime as cli_runtime, temporal_endpoint, workflow_loader};

use crate::commands::run::resolve_llm_credentials;

pub struct PreflightReport {
    pub ready: bool,
    pub steps: Vec<StepReadiness>,
    pub capabilities: Vec<CapabilityReadiness>,
    pub temporal_reachable: bool,
    pub endpoint: String,
    pub user_task_queue: String,
}

pub struct StepReadiness {
    #[allow(dead_code)]
    pub activity_id: String,
    pub step_name: String,
    pub kind: StepKind,
    pub task_queue: String,
    #[allow(dead_code)]
    pub placement: Placement,
    pub missing: Option<String>,
}

pub struct CapabilityReadiness {
    pub id: String,
    pub kind: CapabilityKind,
    pub authed: bool,
    pub detail: Option<String>,
    pub login_command: Option<String>,
}

pub fn check(path: String, update: bool, assume_yes: bool) -> Result<()> {
    let report = preflight(&path, update, assume_yes)?;
    print_report(&report);
    if !report.ready {
        std::process::exit(2);
    }
    Ok(())
}

pub fn preflight(arg: &str, update: bool, assume_yes: bool) -> Result<PreflightReport> {
    let (resolved, mut loaded) = workflow_loader::resolve_arg(arg, update)?;

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
            anyhow::bail!("consent declined; not preflighting remote workflow");
        }
        remote::trust::record_consent(
            &rr.spec,
            &rr.sha,
            remote::trust::declared_capability_strings(&loaded.compiled),
        )?;
    }

    let runtime = cli_runtime::resolve()?;
    cli_runtime::validate_workflow_sources(&runtime, &loaded.absolute_path, &loaded.compiled)?;

    let credentials = resolve_llm_credentials();
    let home = paths::home()?;
    let caps = capabilities::discover(&home, &loaded.compiled.required_cli_binaries, &credentials);

    let identity = OsUser
        .resolve()
        .context("resolving OS user identity for preflight")?;
    let user_task_queue = task_queue_for(&identity);

    let mut cluster = planner::ClusterView::load().unwrap_or_default();
    let self_report = CapabilityReport::from_capabilities_with(
        identity.clone(),
        &caps,
        Some(&paths::credentials_dir()?),
    );
    cluster.add_self(self_report.clone());

    let plan_result = planner::assign_queues(&mut loaded.compiled, &identity, &cluster);

    let steps = build_step_readiness(&loaded.compiled, &plan_result, &cluster);
    let capabilities = build_capability_readiness(&loaded.compiled, &caps, &self_report);
    let mut ready = steps.iter().all(|s| s.missing.is_none())
        && capabilities.iter().all(|c| c.authed)
        && plan_result.is_ok();

    let endpoint = temporal_endpoint::resolve()?;
    let temporal_reachable = cori_worker::runtime::preflight_check(
        &endpoint.target,
        std::time::Duration::from_millis(500),
    )
    .is_ok();
    if !temporal_reachable {
        ready = false;
    }

    Ok(PreflightReport {
        ready,
        steps,
        capabilities,
        temporal_reachable,
        endpoint: endpoint.target,
        user_task_queue,
    })
}

fn build_step_readiness(
    compiled: &CompiledWorkflow,
    plan_result: &Result<Vec<planner::StepAssignment>, planner::PlacementError>,
    cluster: &planner::ClusterView,
) -> Vec<StepReadiness> {
    let assignments: std::collections::BTreeMap<&str, &planner::StepAssignment> = match plan_result
    {
        Ok(asns) => asns.iter().map(|a| (a.activity_id.as_str(), a)).collect(),
        Err(_) => Default::default(),
    };

    compiled
        .steps
        .iter()
        .map(|step| {
            let (task_queue, missing) = match assignments.get(step.activity_id.as_str()) {
                Some(a) => {
                    let m = capability_missing_for(&step.placement, &a.task_queue, cluster);
                    (a.task_queue.clone(), m)
                }
                None => (
                    "(unplaced)".to_string(),
                    Some(match plan_result {
                        Err(e) => format!("placement failed: {e}"),
                        Ok(_) => "missing from plan".to_string(),
                    }),
                ),
            };
            StepReadiness {
                activity_id: step.activity_id.clone(),
                step_name: step.name.clone(),
                kind: step.kind,
                task_queue,
                placement: step.placement.clone(),
                missing,
            }
        })
        .collect()
}

fn capability_missing_for(
    placement: &Placement,
    task_queue: &str,
    cluster: &planner::ClusterView,
) -> Option<String> {
    let Placement::RequiresCapability { id } = placement else {
        return None;
    };
    let report = cluster
        .reports
        .iter()
        .find(|r| r.task_queue == task_queue)?;
    let cap = report.capabilities.iter().find(|c| &c.id == id);
    match cap {
        Some(c) if c.authed => None,
        Some(_) => Some(format!("`{id}` needs sign-in — run: cori login {id}")),
        None => Some(format!(
            "worker on `{task_queue}` does not advertise `{id}`"
        )),
    }
}

fn build_capability_readiness(
    compiled: &CompiledWorkflow,
    caps: &Capabilities,
    self_report: &CapabilityReport,
) -> Vec<CapabilityReadiness> {
    let mut out: Vec<CapabilityReadiness> = Vec::new();

    let lookup =
        |id: &str| -> Option<&Capability> { self_report.capabilities.iter().find(|c| c.id == id) };

    for cli in &compiled.required_cli_binaries {
        let present = caps.has_cli(cli);
        let entry = lookup(cli);
        let authed = present && entry.map(|c| c.authed).unwrap_or(true);
        out.push(CapabilityReadiness {
            id: cli.clone(),
            kind: CapabilityKind::Cli,
            authed,
            detail: entry.and_then(|c| c.detail.clone()),
            login_command: if !authed {
                Some(format!("cori login {cli}"))
            } else {
                None
            },
        });
    }
    for mcp in &compiled.required_mcp_servers {
        let entry = lookup(mcp);
        let (kind, authed) = match entry {
            Some(c) => (c.kind, c.authed),
            None => (CapabilityKind::McpStatic, caps.has_mcp(mcp)),
        };
        out.push(CapabilityReadiness {
            id: mcp.clone(),
            kind,
            authed,
            detail: entry.and_then(|c| c.detail.clone()),
            login_command: if !authed {
                Some(format!("cori login {mcp}"))
            } else {
                None
            },
        });
    }
    for llm in &compiled.required_llm_providers {
        let authed = caps.llm_providers.contains(llm);
        out.push(CapabilityReadiness {
            id: llm.clone(),
            kind: CapabilityKind::Llm,
            authed,
            detail: None,
            login_command: if !authed {
                Some(format!("cori login {llm}"))
            } else {
                None
            },
        });
    }
    out
}

fn print_report(report: &PreflightReport) {
    let identity_str = match OsUser.resolve().ok() {
        Some(WorkerIdentity::Person { user_id }) => format!("{user_id} (OS user)"),
        Some(WorkerIdentity::Service { pool }) => format!("service:{pool}"),
        None => "<unknown>".to_string(),
    };
    println!("Cori check");
    println!();
    println!("Identity:   {identity_str}");
    println!(
        "Endpoint:   {} ({})",
        report.endpoint,
        if report.temporal_reachable {
            "reachable"
        } else {
            "✗ not reachable — `temporal server start-dev` or set temporal.host"
        },
    );
    println!("User queue: {}", report.user_task_queue);
    println!();
    println!("Steps:");
    for (i, s) in report.steps.iter().enumerate() {
        let marker = if s.missing.is_some() { "✗" } else { "✓" };
        println!(
            "  {marker} step {n} {name} ({kind}) → {q}",
            n = i + 1,
            name = s.step_name,
            kind = kind_label(s.kind),
            q = s.task_queue,
        );
        if let Some(reason) = &s.missing {
            println!("      {reason}");
        }
    }
    println!();
    println!("Capabilities:");
    if report.capabilities.is_empty() {
        println!("  (none required)");
    } else {
        for c in &report.capabilities {
            let marker = if c.authed { "✓" } else { "✗" };
            let detail = c
                .detail
                .as_deref()
                .map(|d| format!(" — {d}"))
                .unwrap_or_default();
            println!(
                "  {marker} {id} ({kind}){detail}",
                id = c.id,
                kind = cap_kind_label(c.kind),
            );
            if let Some(cmd) = &c.login_command {
                println!("      needs sign-in — run: {cmd}");
            }
        }
    }
    println!();
    if report.ready {
        println!("Result: ✓ ready");
    } else {
        let missing_count = report.steps.iter().filter(|s| s.missing.is_some()).count()
            + report.capabilities.iter().filter(|c| !c.authed).count();
        println!(
            "Result: ✗ {missing_count} item{plural} need{verb} attention",
            plural = if missing_count == 1 { "" } else { "s" },
            verb = if missing_count == 1 { "s" } else { "" },
        );
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

fn cap_kind_label(kind: CapabilityKind) -> &'static str {
    match kind {
        CapabilityKind::Cli => "CLI",
        CapabilityKind::McpOauth => "MCP, OAuth",
        CapabilityKind::McpStatic => "MCP",
        CapabilityKind::Llm => "LLM",
        CapabilityKind::LocalFs => "local_fs",
    }
}
