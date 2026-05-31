//! First-run consent for remote workflows.

use std::collections::BTreeMap;
use std::io::{BufRead, Write};
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::paths;
use cori_protocol::CompiledWorkflow;

use super::refspec::RemoteRef;

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Trust {
    #[serde(flatten)]
    pub entries: BTreeMap<String, TrustEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrustEntry {
    pub consented_at: chrono::DateTime<chrono::Utc>,
    #[serde(default)]
    pub consented_capabilities: Vec<String>,
}

pub fn trust_key(spec: &RemoteRef, sha: &str) -> String {
    format!("{}/{}@{sha}", spec.host, spec.repo)
}

pub fn load() -> Result<Trust> {
    load_from(&paths::trust_file()?)
}

pub fn load_from(path: &Path) -> Result<Trust> {
    if !path.exists() {
        return Ok(Trust::default());
    }
    let bytes = std::fs::read(path).with_context(|| format!("reading `{}`", path.display()))?;
    if bytes.is_empty() {
        return Ok(Trust::default());
    }
    serde_json::from_slice(&bytes).with_context(|| format!("parsing `{}`", path.display()))
}

pub fn save(trust: &Trust) -> Result<()> {
    save_to(&paths::trust_file()?, trust)
}

pub fn save_to(path: &Path, trust: &Trust) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating `{}`", parent.display()))?;
    }
    let bytes = serde_json::to_vec_pretty(trust).context("serializing trust.json")?;
    let tmp = path.with_extension("partial");
    std::fs::write(&tmp, &bytes).with_context(|| format!("writing `{}`", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("renaming into `{}`", path.display()))?;
    Ok(())
}

pub fn is_trusted(spec: &RemoteRef, sha: &str) -> Result<bool> {
    let t = load()?;
    Ok(t.entries.contains_key(&trust_key(spec, sha)))
}

pub fn record_consent(spec: &RemoteRef, sha: &str, caps: Vec<String>) -> Result<()> {
    let mut t = load()?;
    t.entries.insert(
        trust_key(spec, sha),
        TrustEntry {
            consented_at: chrono::Utc::now(),
            consented_capabilities: caps,
        },
    );
    save(&t)
}

/// Print the consent banner and read a y/N answer from stdin.
pub fn prompt_consent(
    spec: &RemoteRef,
    sha: &str,
    workflow_dir: &Path,
    compiled: &CompiledWorkflow,
) -> Result<bool> {
    let stderr = std::io::stderr();
    let mut out = stderr.lock();
    writeln!(out, "\nRunning remote workflow for the first time:")?;
    writeln!(out, "  {}/{}{}", spec.host, spec.repo, format_subpath(spec))?;
    writeln!(out, "  ref:      {}", spec.ref_str_display())?;
    writeln!(out, "  sha:      {}", short_sha(sha))?;
    writeln!(out, "  fetched:  just now")?;
    writeln!(out)?;

    writeln!(out, "This workflow declares it will use:")?;
    let caps = capability_lines(compiled);
    if caps.is_empty() {
        writeln!(out, "  • (no external capabilities declared)")?;
    } else {
        for line in &caps {
            writeln!(out, "  • {line}")?;
        }
    }
    writeln!(out)?;

    writeln!(out, "It also runs these step files:")?;
    let step_files = list_step_files(workflow_dir);
    if step_files.is_empty() {
        writeln!(out, "  (none)")?;
    } else {
        for f in &step_files {
            writeln!(out, "  {f}")?;
        }
    }
    writeln!(out)?;

    write!(out, "Trust this workflow at this version? [y/N] ")?;
    out.flush()?;
    drop(out);

    let mut answer = String::new();
    let stdin = std::io::stdin();
    stdin.lock().read_line(&mut answer)?;
    let trimmed = answer.trim();
    Ok(matches!(trimmed, "y" | "Y" | "yes" | "Yes" | "YES"))
}

pub fn declared_capability_strings(compiled: &CompiledWorkflow) -> Vec<String> {
    let mut out = Vec::new();
    for c in &compiled.required_cli_binaries {
        out.push(format!("cli:{c}"));
    }
    for s in &compiled.required_mcp_servers {
        out.push(format!("mcp:{s}"));
    }
    for l in &compiled.required_llm_providers {
        out.push(format!("llm:{l}"));
    }
    out
}

fn capability_lines(compiled: &CompiledWorkflow) -> Vec<String> {
    let mut out = Vec::new();
    for c in &compiled.required_cli_binaries {
        out.push(format!("{c}        (CLI on your PATH)"));
    }
    for s in &compiled.required_mcp_servers {
        out.push(format!(
            "{s}        (MCP, requires OAuth sign-in if not already authorized)"
        ));
    }
    for l in &compiled.required_llm_providers {
        out.push(format!("{l}        (LLM provider)"));
    }
    out
}

fn list_step_files(workflow_dir: &Path) -> Vec<String> {
    let steps_dir = workflow_dir.join("steps");
    if !steps_dir.is_dir() {
        return Vec::new();
    }
    let mut files: Vec<String> = std::fs::read_dir(&steps_dir)
        .map(|it| {
            it.flatten()
                .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
                .filter_map(|e| e.file_name().to_str().map(|s| s.to_string()))
                .filter(|n| n.ends_with(".ts"))
                .collect()
        })
        .unwrap_or_default();
    files.sort();
    files
}

fn format_subpath(spec: &RemoteRef) -> String {
    if spec.subpath.is_empty() {
        String::new()
    } else {
        format!("/{}", spec.subpath)
    }
}

fn short_sha(sha: &str) -> String {
    let n = 10.min(sha.len());
    format!("{}...", &sha[..n])
}

pub fn assume_yes_env() -> bool {
    matches!(
        std::env::var("CORI_ASSUME_YES").as_deref(),
        Ok("1") | Ok("true") | Ok("yes") | Ok("YES")
    )
}
