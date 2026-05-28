//! `cori runs list|show` — disk-backed run history.
//!
//! Phase 2 reads from `~/.cori/runs/<run_history_key>/<utc>.json`.
//! Each JSON file is a [`super::run::RunTrace`]. We discover files by
//! walking `runs_dir()`; there is no index. For typical hobby use
//! (hundreds of runs) this is plenty fast; if directories ever grow
//! into the tens of thousands a tiny SQLite index can be added later
//! without changing the on-disk format.

use std::cmp::Reverse;
use std::path::PathBuf;

use anyhow::{Context, Result, anyhow, bail};
use chrono::{DateTime, Utc};
use chrono_humanize::HumanTime;
use comfy_table::{Cell, Table, presets::UTF8_FULL};

use super::run::RunTrace;
use crate::paths;

struct RunEntry {
    trace: RunTrace,
    path: PathBuf,
}

fn collect_runs(workflow_filter: Option<&str>) -> Result<Vec<RunEntry>> {
    let root = paths::runs_dir()?;
    let mut out: Vec<RunEntry> = Vec::new();
    if !root.exists() {
        return Ok(out);
    }
    for hist_dir in
        std::fs::read_dir(&root).with_context(|| format!("reading `{}`", root.display()))?
    {
        let Ok(hist_dir) = hist_dir else { continue };
        let dir_path = hist_dir.path();
        if !dir_path.is_dir() {
            continue;
        }
        let Ok(files) = std::fs::read_dir(&dir_path) else {
            continue;
        };
        for file in files.flatten() {
            let path = file.path();
            if path
                .extension()
                .and_then(|s| s.to_str())
                .is_some_and(|s| s == "json")
                && let Some(name) = path.file_name().and_then(|s| s.to_str())
                && !name.starts_with('.')
                && let Ok(bytes) = std::fs::read(&path)
                && let Ok(trace) = serde_json::from_slice::<RunTrace>(&bytes)
            {
                if let Some(filter) = workflow_filter
                    && trace.workflow_id != filter
                {
                    continue;
                }
                out.push(RunEntry { trace, path });
            }
        }
    }
    out.sort_by_key(|e| Reverse(e.trace.started_at));
    Ok(out)
}

pub fn list(workflow_filter: Option<&str>, limit: u32, json_out: bool) -> Result<()> {
    let mut entries = collect_runs(workflow_filter)?;
    entries.truncate(limit as usize);

    if json_out {
        let arr: Vec<_> = entries.iter().map(|e| &e.trace).collect();
        println!("{}", serde_json::to_string_pretty(&arr)?);
        return Ok(());
    }

    if entries.is_empty() {
        println!("No runs recorded yet.");
        return Ok(());
    }

    let mut table = Table::new();
    table.load_preset(UTF8_FULL);
    table.set_header(vec![
        "When", "Workflow", "Status", "Duration", "Cost", "Run id",
    ]);
    for e in &entries {
        let when = HumanTime::from(e.trace.started_at).to_string();
        let duration = format_duration_ms(e.trace.duration_ms);
        let cost = if e.trace.cost.total_eur > 0.0 {
            format!("€{:.4}", e.trace.cost.total_eur)
        } else {
            "—".into()
        };
        table.add_row(vec![
            Cell::new(when),
            Cell::new(&e.trace.workflow_id),
            Cell::new(&e.trace.status),
            Cell::new(duration),
            Cell::new(cost),
            Cell::new(&e.trace.run_id),
        ]);
    }
    println!("{table}");
    Ok(())
}

pub fn show(run_id: &str, activity_filter: Option<&str>, full: bool, json_out: bool) -> Result<()> {
    let entries = collect_runs(None)?;
    let entry = entries
        .into_iter()
        .find(|e| e.trace.run_id == run_id)
        .ok_or_else(|| anyhow!("no run found with id `{run_id}`"))?;

    if json_out {
        if let Some(name) = activity_filter {
            let act = entry
                .trace
                .activities
                .iter()
                .find(|a| a.step_name == name || a.activity_id == name)
                .ok_or_else(|| anyhow!("no activity matching `{name}` in run `{run_id}`"))?;
            println!("{}", serde_json::to_string_pretty(act)?);
        } else {
            println!("{}", serde_json::to_string_pretty(&entry.trace)?);
        }
        return Ok(());
    }

    let trace = &entry.trace;
    println!("Run {} ({})", trace.run_id, trace.workflow_id);
    if let Some(h) = &trace.workflow_content_hash {
        println!("  content    {}", &h[..8.min(h.len())]);
    }
    println!("  status     {}", trace.status);
    println!("  trigger    {}", trace.trigger);
    println!(
        "  started    {} ({})",
        trace.started_at.format("%Y-%m-%d %H:%M:%S UTC"),
        HumanTime::from(trace.started_at),
    );
    println!("  duration   {}", format_duration_ms(trace.duration_ms));
    if trace.cost.total_eur > 0.0 {
        println!(
            "  cost       €{:.4} ({} in / {} out tokens)",
            trace.cost.total_eur, trace.cost.input_tokens, trace.cost.output_tokens
        );
    }
    if let Some(err) = &trace.error {
        println!("  error      {err}");
    }
    println!("  trace      {}", entry.path.display());

    let acts: Vec<_> = trace
        .activities
        .iter()
        .filter(|a| match activity_filter {
            Some(name) => a.step_name == name || a.activity_id == name,
            None => true,
        })
        .collect();
    if acts.is_empty() {
        if let Some(name) = activity_filter {
            bail!("no activity matching `{name}` in run `{run_id}`");
        }
        return Ok(());
    }

    println!();
    let mut table = Table::new();
    table.load_preset(UTF8_FULL);
    table.set_header(vec!["#", "Step", "Kind", "Status", "Duration", "Cost"]);
    for (i, a) in acts.iter().enumerate() {
        let cost = match a.cost_eur {
            Some(c) if c > 0.0 => format!("€{c:.4}"),
            _ => "—".into(),
        };
        table.add_row(vec![
            Cell::new(i + 1),
            Cell::new(&a.step_name),
            Cell::new(format!("{:?}", a.kind).to_lowercase()),
            Cell::new(&a.status),
            Cell::new(format_duration_ms(a.duration_ms)),
            Cell::new(cost),
        ]);
    }
    println!("{table}");

    if full {
        for a in &acts {
            println!("\n— {} —", a.step_name);
            println!("{}", serde_json::to_string_pretty(&a.output)?);
            if let Some(err) = &a.error {
                println!("error: {err}");
            }
        }
    }

    Ok(())
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

#[allow(dead_code)]
fn _date_helper(_dt: DateTime<Utc>) {}
