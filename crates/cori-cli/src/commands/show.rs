//! `cori show <path>` — Phase 7 workflow-scoped inspector.
//!
//! Prints the manifest (frontmatter summary + prose body), each step's
//! kind / placement / declared capability, and the most recent runs
//! recorded under `~/.cori/runs/<key>/`.
//!
//! Accepts either a local path or a remote git ref. For a remote ref
//! that has not been fetched locally, only the run history is shown
//! (since the run-history key doesn't depend on the resolved sha).

use anyhow::{Context, Result};
use chrono_humanize::HumanTime;
use cori_protocol::{Placement, RunTrace, StepKind};

use cori_run::remote::{self, ArgClass};
use cori_run::{paths, workflow_loader};

pub fn show(path: String) -> Result<()> {
    let class = remote::classify_arg(&path)?;
    match class {
        ArgClass::Local(p) => {
            let loaded = workflow_loader::load(&p)?;
            print_loaded(&loaded)?;
        }
        ArgClass::Remote(spec) => {
            // Don't fetch on `show`. If a pin exists, try to load from
            // the cached checkout; otherwise just print history.
            let pins = remote::pins::load()?;
            let cached = pins.get(&spec.pin_key()).and_then(|sha| {
                let dir = paths::remote_cache_dir()
                    .ok()?
                    .join(&spec.host)
                    .join(&spec.repo)
                    .join(sha)
                    .join(if spec.subpath.is_empty() {
                        std::path::PathBuf::new()
                    } else {
                        std::path::PathBuf::from(&spec.subpath)
                    });
                if dir.join("manifest.md").is_file() {
                    Some(dir)
                } else {
                    None
                }
            });
            match cached {
                Some(dir) => {
                    let loaded = workflow_loader::load(&dir)?;
                    print_loaded(&loaded)?;
                }
                None => {
                    println!("Workflow: {}", spec.display());
                    println!(
                        "  (workflow not fetched locally — run `cori run {}` first to inspect steps)",
                        spec.display()
                    );
                    println!();
                    print_remote_history(&spec)?;
                }
            }
        }
    }
    Ok(())
}

fn print_loaded(loaded: &workflow_loader::LoadedWorkflow) -> Result<()> {
    let manifest = &loaded.compiled.manifest;

    println!("Workflow: {}", manifest.id);
    println!("  name        {}", manifest.name);
    if !manifest.description.is_empty() {
        println!("  description {}", manifest.description);
    }
    println!("  path        {}", loaded.absolute_path.display());
    println!(
        "  content     {}",
        &loaded.content_hash[..8.min(loaded.content_hash.len())]
    );
    if loaded.from_cache {
        println!("  cache       hit");
    }

    println!();
    print_requirements(&loaded.compiled);
    println!();
    print_steps(&loaded.compiled);
    println!();
    print_recent_runs(loaded)?;

    if !manifest.body.trim().is_empty() {
        println!();
        println!("--- manifest body ---");
        println!("{}", manifest.body.trim_end());
    }
    Ok(())
}

fn print_remote_history(spec: &remote::RemoteRef) -> Result<()> {
    let key = remote::remote_run_history_key(spec);
    let dir = paths::runs_dir()?.join(&key);
    println!("Recent runs (history key: {key})");
    if !dir.is_dir() {
        println!("  (no runs yet)");
        return Ok(());
    }
    print_runs_dir(&dir)
}

fn print_requirements(compiled: &cori_protocol::CompiledWorkflow) {
    println!("Requires:");
    let mut any = false;
    if !compiled.required_cli_binaries.is_empty() {
        any = true;
        println!("  CLIs : {}", compiled.required_cli_binaries.join(", "));
    }
    if !compiled.required_mcp_servers.is_empty() {
        any = true;
        println!("  MCPs : {}", compiled.required_mcp_servers.join(", "));
    }
    if !compiled.required_llm_providers.is_empty() {
        any = true;
        println!("  LLMs : {}", compiled.required_llm_providers.join(", "));
    }
    if !any {
        println!("  (nothing — pure code workflow)");
    }
}

fn print_steps(compiled: &cori_protocol::CompiledWorkflow) {
    println!("Steps:");
    for (i, step) in compiled.steps.iter().enumerate() {
        let placement = match &step.placement {
            Placement::Anywhere => "anywhere".to_string(),
            Placement::RequiresLocalFs => "local_fs".to_string(),
            Placement::RequiresCapability { id } => format!("needs:{id}"),
        };
        println!(
            "  {n}. {name} ({kind}, {placement})",
            n = i + 1,
            name = step.name,
            kind = kind_label(step.kind),
        );
        if !step.description.is_empty() {
            println!("       {}", step.description);
        }
    }
}

fn print_recent_runs(loaded: &workflow_loader::LoadedWorkflow) -> Result<()> {
    let key = workflow_loader::loaded_run_history_key(loaded);
    let dir = paths::runs_dir()?.join(&key);
    println!("Recent runs (history key: {key})");
    if !dir.is_dir() {
        println!("  (no runs yet)");
        return Ok(());
    }
    print_runs_dir(&dir)
}

fn print_runs_dir(dir: &std::path::Path) -> Result<()> {
    let mut entries: Vec<(std::path::PathBuf, RunTrace)> = std::fs::read_dir(dir)
        .with_context(|| format!("reading `{}`", dir.display()))?
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            if p.extension().and_then(|s| s.to_str()) != Some("json") {
                return None;
            }
            if p.file_name()
                .and_then(|s| s.to_str())
                .map(|s| s.starts_with('.'))
                .unwrap_or(false)
            {
                return None;
            }
            let bytes = std::fs::read(&p).ok()?;
            let t: RunTrace = serde_json::from_slice(&bytes).ok()?;
            Some((p, t))
        })
        .collect();
    entries.sort_by_key(|(_, t)| std::cmp::Reverse(t.started_at));
    if entries.is_empty() {
        println!("  (no runs yet)");
        return Ok(());
    }
    for (_, t) in entries.iter().take(10) {
        let when = HumanTime::from(t.started_at).to_string();
        let dur = format_duration_ms(t.duration_ms);
        let cost = if t.cost.total_eur > 0.0 {
            format!("  €{:.4}", t.cost.total_eur)
        } else {
            String::new()
        };
        println!(
            "  · {when:<20}  {status:<10}  {dur:>8}{cost}  {id}",
            status = t.status,
            id = t.run_id,
        );
    }
    Ok(())
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

fn format_duration_ms(ms: u128) -> String {
    if ms < 1_000 {
        format!("{ms}ms")
    } else if ms < 60_000 {
        format!("{:.2}s", ms as f64 / 1_000.0)
    } else {
        let secs = ms / 1_000;
        let m = secs / 60;
        let s = secs % 60;
        format!("{m}m{s:02}s")
    }
}
