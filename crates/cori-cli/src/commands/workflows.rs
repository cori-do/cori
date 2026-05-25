//! `cori workflows register|list|show`.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use comfy_table::{presets::UTF8_FULL, ContentArrangement, Table};

use crate::registry::{self, RegisterOutcome, WorkflowDetail};

pub fn register(path: &Path) -> Result<()> {
    let abs = path
        .canonicalize()
        .with_context(|| format!("resolving runbook path `{}`", path.display()))?;
    let compiled = match cori_compiler::compile(&abs) {
        Ok(c) => c,
        Err(errors) => {
            for e in &errors {
                eprintln!("✗ {}", format_error(&abs, e));
            }
            eprintln!(
                "\n{} error{} — workflow not registered.",
                errors.len(),
                if errors.len() == 1 { "" } else { "s" }
            );
            std::process::exit(2);
        }
    };

    let mut reg = registry::open()?;
    let outcome = reg.register(&abs, &compiled)?;
    let id = &compiled.manifest.id;
    match outcome {
        RegisterOutcome::Created { version } => {
            println!("✓ Registered {id} (v{version})");
        }
        RegisterOutcome::Updated { version } => {
            println!("✓ Re-registered {id} (v{version}, content changed)");
        }
        RegisterOutcome::Unchanged { version } => {
            println!("· {id} unchanged (v{version})");
        }
    }
    Ok(())
}

pub fn list(json: bool) -> Result<()> {
    let reg = registry::open()?;
    let rows = reg.list()?;
    if json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }
    if rows.is_empty() {
        println!("No workflows registered. Try: cori workflows register <path>");
        return Ok(());
    }
    let mut table = Table::new();
    table
        .load_preset(UTF8_FULL)
        .set_content_arrangement(ContentArrangement::Dynamic)
        .set_header(vec!["ID", "NAME", "VERSION", "REGISTERED", "SOURCE"]);
    let now = chrono::Utc::now();
    for r in rows {
        let registered = chrono_humanize::HumanTime::from(r.registered_at - now).to_string();
        table.add_row(vec![
            r.id,
            truncate(&r.name, 40),
            r.version.to_string(),
            registered,
            r.source_path,
        ]);
    }
    println!("{table}");
    Ok(())
}

pub fn show(id: &str, field: Option<&str>, json: bool) -> Result<()> {
    let reg = registry::open()?;
    let Some(detail) = reg.get(id)? else {
        bail!("no workflow with id `{id}`. Run `cori workflows list` to see registered workflows.");
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&detail.compiled)?);
        return Ok(());
    }
    if let Some(field) = field {
        return show_field(&detail, field);
    }
    println!("{}", detail.manifest_yaml);
    Ok(())
}

fn show_field(detail: &WorkflowDetail, field: &str) -> Result<()> {
    // Top-level frontmatter fields go through the manifest struct.
    let manifest = &detail.compiled.manifest;
    let val: Option<String> = match field {
        "id" => Some(manifest.id.clone()),
        "name" => Some(manifest.name.clone()),
        "description" => Some(manifest.description.clone()),
        "version" => Some(manifest.version.to_string()),
        "created" => Some(manifest.created.to_string()),
        "updated" => manifest.updated.map(|d| d.to_string()),
        "schedule" => manifest.schedule.clone(),
        "schedule_tz" => manifest.schedule_tz.clone(),
        "tools_required" => Some(manifest.tools_required.join(", ")),
        "mcp_servers" => Some(manifest.mcp_servers.join(", ")),
        "tags" => Some(manifest.tags.join(", ")),
        _ => None,
    };
    if let Some(v) = val {
        println!("{v}");
        return Ok(());
    }
    // Otherwise look for a Markdown section heading like `## <field>` in the
    // prose body (matches the spec: `cori workflows show <id> --field=goal`).
    if let Some(section) = extract_section(&manifest.body, field) {
        println!("{}", section.trim());
        return Ok(());
    }
    bail!(
        "no field or section `{field}` in workflow `{}`. \
         Known top-level fields: id, name, description, version, created, updated, \
         schedule, schedule_tz, tools_required, mcp_servers, tags. \
         Section lookups match `## <name>` headings in the manifest body.",
        detail.id
    );
}

/// Pull out the Markdown body under `## <name>` (case-insensitive), stopping
/// at the next `##` heading.
fn extract_section(body: &str, name: &str) -> Option<String> {
    let mut out = String::new();
    let mut capturing = false;
    let needle = name.to_ascii_lowercase();
    for line in body.lines() {
        if let Some(rest) = line.strip_prefix("## ") {
            if capturing {
                break;
            }
            if rest.trim().to_ascii_lowercase() == needle {
                capturing = true;
                continue;
            }
        }
        if capturing {
            out.push_str(line);
            out.push('\n');
        }
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

fn format_error(runbook_root: &Path, e: &cori_compiler::CompileError) -> String {
    // Render as `<absolute>/<file>:<line>: <reason>` so editors with
    // hyperlinked terminals jump to the right place.
    let path: PathBuf = runbook_root.join(&e.file);
    match e.line {
        Some(l) => format!("{}:{l}: {}", path.display(), e.reason),
        None => format!("{}: {}", path.display(), e.reason),
    }
}
