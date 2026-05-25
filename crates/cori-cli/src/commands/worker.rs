//! `cori worker start`.
//!
//! Boots the long-running worker daemon: supervises a bundled Temporal
//! (or attaches to one declared via `temporal.host`), discovers
//! capabilities, watches `~/.cori/runbooks/` for changes, and keeps the
//! registry in sync until Ctrl-C.

use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use cori_broker::capabilities;
use cori_worker::{RegisterOutcome, WorkerConfig};

use crate::{config::Config, paths, registry};

pub fn start() -> Result<()> {
    let home = paths::home()?;
    let runbooks_dir = paths::runbooks_dir()?;
    let state_dir = paths::state_dir()?.join("temporal");

    // Make sure the directories exist so registry / watcher / Temporal
    // don't immediately fall over.
    std::fs::create_dir_all(&runbooks_dir)
        .with_context(|| format!("creating runbooks directory `{}`", runbooks_dir.display()))?;

    let cfg = Config::load().ok();
    let temporal_host = cfg
        .as_ref()
        .and_then(|c| c.get("temporal.host"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // Capability banner: enumerate registered workflows' requirements so
    // operators see what's wired up before anything runs.
    let reg = registry::open()?;
    let workflows = reg.list().unwrap_or_default();
    let mut wanted_clis: Vec<String> = Vec::new();
    let mut wanted_mcp: Vec<String> = Vec::new();
    for row in &workflows {
        if let Ok(Some(detail)) = reg.get(&row.id) {
            for b in &detail.compiled.required_cli_binaries {
                if !wanted_clis.contains(b) {
                    wanted_clis.push(b.clone());
                }
            }
            for s in &detail.compiled.required_mcp_servers {
                if !wanted_mcp.contains(s) {
                    wanted_mcp.push(s.clone());
                }
            }
        }
    }
    let creds = crate::commands::run::resolve_llm_credentials().unwrap_or_default();
    let caps = capabilities::discover(&home, &wanted_clis, &creds);
    let banner = build_capability_banner(&caps, &workflows);

    let config = WorkerConfig {
        temporal_host,
        temporal_state_dir: state_dir,
        runbooks_dir,
        register: Arc::new(register_runbook),
        capability_banner: banner,
    };

    cori_worker::run(config)
}

fn register_runbook(path: &Path) -> Result<RegisterOutcome> {
    let abs = path
        .canonicalize()
        .with_context(|| format!("resolving runbook path `{}`", path.display()))?;
    let compiled = cori_compiler::compile(&abs).map_err(|errors| {
        let summary = errors
            .iter()
            .map(|e| format!("{}: {}", e.file, e.reason))
            .collect::<Vec<_>>()
            .join("; ");
        anyhow::anyhow!("compile failed: {summary}")
    })?;
    let mut reg = registry::open()?;
    let outcome = reg.register(&abs, &compiled)?;
    let workflow_id = compiled.manifest.id.clone();
    let (version, note) = match outcome {
        registry::RegisterOutcome::Created { version } => (version, "created"),
        registry::RegisterOutcome::Updated { version } => (version, "updated"),
        registry::RegisterOutcome::Unchanged { version } => (version, "unchanged"),
    };
    Ok(RegisterOutcome {
        workflow_id,
        version,
        note,
    })
}

fn build_capability_banner(
    caps: &capabilities::Capabilities,
    workflows: &[registry::WorkflowRow],
) -> Vec<String> {
    let mut lines = Vec::new();
    lines.push(format!("workflows: {}", workflows.len()));
    let cli_line = if caps.cli_binaries.is_empty() {
        "CLIs: (none)".to_string()
    } else {
        let names: Vec<String> = caps.cli_binaries.keys().cloned().collect();
        format!("CLIs: {}", names.join(", "))
    };
    lines.push(cli_line);
    let mcp_line = if caps.mcp_servers.is_empty() {
        "MCP servers: (none configured in ~/.cori/mcp-servers.json)".to_string()
    } else {
        let names: Vec<String> = caps.mcp_servers.keys().cloned().collect();
        format!("MCP servers: {}", names.join(", "))
    };
    lines.push(mcp_line);
    let llm_line = if caps.llm_providers.is_empty() {
        "LLM providers: (none — set OPENAI_API_KEY / ANTHROPIC_API_KEY / GEMINI_API_KEY)"
            .to_string()
    } else {
        let names: Vec<String> = caps.llm_providers.iter().cloned().collect();
        format!("LLM providers: {}", names.join(", "))
    };
    lines.push(llm_line);
    lines
}
