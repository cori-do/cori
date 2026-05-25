//! `cori runs list|show`.
//!
//! Inspects rows in the local `runs` SQLite table. The trace JSON is
//! whatever the run loop stored (see [`crate::commands::run`]).

use anyhow::{bail, Result};
use comfy_table::{presets::UTF8_FULL, ContentArrangement, Table};
use serde_json::Value as JsonValue;

use crate::registry;

pub fn list(workflow_id: Option<&str>, limit: u32, json: bool) -> Result<()> {
    let reg = registry::open()?;
    let runs = reg.list_runs(workflow_id, limit)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&runs)?);
        return Ok(());
    }
    if runs.is_empty() {
        println!("No runs recorded yet. Try: cori run <workflow_id>");
        return Ok(());
    }
    let mut table = Table::new();
    table
        .load_preset(UTF8_FULL)
        .set_content_arrangement(ContentArrangement::Dynamic)
        .set_header(vec!["RUN ID", "WORKFLOW", "STATUS", "STARTED", "ENDED"]);
    let now = chrono::Utc::now();
    for r in runs {
        let started = chrono_humanize::HumanTime::from(r.started_at - now).to_string();
        let ended = r
            .ended_at
            .map(|t| chrono_humanize::HumanTime::from(t - now).to_string())
            .unwrap_or_else(|| "—".to_string());
        table.add_row(vec![r.run_id, r.workflow_id, r.status, started, ended]);
    }
    println!("{table}");
    Ok(())
}

pub fn show(run_id: &str, activity: Option<&str>, full: bool, json: bool) -> Result<()> {
    let reg = registry::open()?;
    let Some(detail) = reg.get_run(run_id)? else {
        bail!("no run with id `{run_id}`. Try `cori runs list`.");
    };
    let trace: JsonValue = serde_json::from_str(&detail.trace_json)?;

    // Activity-scoped view.
    if let Some(act_id) = activity {
        let activities = trace
            .get("activities")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let found = activities
            .into_iter()
            .find(|a| a.get("activity_id").and_then(|v| v.as_str()) == Some(act_id));
        let Some(mut found) = found else {
            bail!("no activity `{act_id}` in run `{run_id}`");
        };
        if !full {
            if let JsonValue::Object(m) = &mut found {
                if let Some(o) = m.get_mut("output") {
                    *o = summarize(o);
                }
            }
        }
        println!("{}", serde_json::to_string_pretty(&found)?);
        return Ok(());
    }

    if json {
        println!("{}", serde_json::to_string_pretty(&trace)?);
        return Ok(());
    }

    // Human view: header + per-activity table.
    println!("Run {run_id}");
    println!("  workflow: {}", detail.workflow_id);
    println!("  status:   {}", detail.status);
    println!("  started:  {}", detail.started_at);
    if let Some(t) = detail.ended_at {
        println!("  ended:    {t}");
    }
    if let Some(dry) = trace.get("dry_run").and_then(|v| v.as_bool()) {
        if dry {
            println!("  mode:     DRY RUN — no external calls");
        }
    }
    if let Some(cost) = trace.get("cost").and_then(|c| c.get("total_eur")) {
        println!("  cost:     €{cost}");
    }

    let activities = trace
        .get("activities")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    if !activities.is_empty() {
        let mut table = Table::new();
        table
            .load_preset(UTF8_FULL)
            .set_content_arrangement(ContentArrangement::Dynamic)
            .set_header(vec!["#", "ACTIVITY", "KIND", "STATUS", "MS", "COST"]);
        for (i, a) in activities.iter().enumerate() {
            let id = a
                .get("activity_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let kind = a
                .get("kind")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let status = a
                .get("status")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let ms = a
                .get("duration_ms")
                .and_then(|v| v.as_u64())
                .map(|v| v.to_string())
                .unwrap_or_else(|| "—".into());
            let cost = a
                .get("cost_eur")
                .and_then(|v| v.as_f64())
                .map(|v| format!("€{v:.4}"))
                .unwrap_or_else(|| "—".into());
            table.add_row(vec![(i + 1).to_string(), id, kind, status, ms, cost]);
        }
        println!("{table}");
    }
    Ok(())
}

/// Render a compact summary of a JSON value: counts for arrays/objects,
/// a truncated preview for strings, primitives unchanged.
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
