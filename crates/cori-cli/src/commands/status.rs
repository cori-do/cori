//! `cori status` — Phase 7 machine-scoped overview.
//!
//! Prints the active Temporal endpoint and how it was resolved, the
//! current identity + derived task queue, every capability discovered
//! on this machine with auth state, and the workers currently visible
//! on the cluster.
//!
//! Cluster presence in v1 reads `~/.cori/cluster/<queue>.json` (the
//! files `cori work` publishes on startup). The redesign reserves
//! Temporal `DescribeTaskQueue` for this human-frequency path; the
//! cluster-file fallback is the v1 hack and is replaced when a real
//! `DescribeTaskQueue` client wrapper lands in `cori-worker`.

use anyhow::{Context, Result};
use cori_broker::capabilities::{self, Capability, CapabilityKind, CapabilityReport};
use cori_broker::identity::{IdentitySource, OsUser};
use cori_protocol::{WorkerIdentity, task_queue_for};

use crate::commands::run::resolve_llm_credentials;
use crate::remote;
use crate::{paths, planner, temporal_endpoint};

pub fn status() -> Result<()> {
    let endpoint = temporal_endpoint::resolve()?;
    let reachable = cori_worker::runtime::preflight_check(
        &endpoint.target,
        std::time::Duration::from_millis(500),
    )
    .is_ok();

    let identity = OsUser
        .resolve()
        .context("resolving OS user identity for `cori status`")?;
    let queue = task_queue_for(&identity);

    let credentials = resolve_llm_credentials();
    let home = paths::home()?;
    let caps = capabilities::discover(&home, &[], &credentials);
    let self_report = CapabilityReport::from_capabilities_with(
        identity.clone(),
        &caps,
        Some(&paths::credentials_dir()?),
    );

    let cluster = planner::ClusterView::load().unwrap_or_default();

    print_header();
    print_endpoint(&endpoint.target, reachable);
    print_identity(&identity, &queue);
    println!();
    print_capabilities(&self_report);
    println!();
    print_cluster(&cluster, &queue);
    println!();
    print_pinned_remotes()?;
    Ok(())
}

fn print_header() {
    let host = hostname().unwrap_or_else(|| "unknown".to_string());
    println!("Cori status (machine: {host})");
    println!();
}

fn print_endpoint(target: &str, reachable: bool) {
    let suffix = if reachable {
        ""
    } else {
        " — not reachable (start with `temporal server start-dev` or set temporal.host)"
    };
    println!("Endpoint:   {target}{suffix}");
}

fn print_identity(identity: &WorkerIdentity, queue: &str) {
    match identity {
        WorkerIdentity::Person { user_id } => {
            println!("Identity:   {user_id}  (OS user)        → {queue}");
        }
        WorkerIdentity::Service { pool } => {
            println!("Identity:   service:{pool}              → {queue}");
        }
    }
}

fn print_capabilities(report: &CapabilityReport) {
    println!("Capabilities on this machine:");
    if report.capabilities.is_empty() {
        println!("  (none discovered)");
        return;
    }
    for c in &report.capabilities {
        print_capability(c);
    }
}

fn print_capability(c: &Capability) {
    let marker = if c.authed { "✓" } else { "✗" };
    let kind = cap_kind_label(c.kind);
    let detail = c
        .detail
        .as_deref()
        .map(|d| format!(", {d}"))
        .unwrap_or_default();
    let hint = if !c.authed {
        format!("                — run: cori login {id}", id = c.id)
    } else {
        String::new()
    };
    println!("  {marker} {id:<10} ({kind}{detail}){hint}", id = c.id,);
}

fn print_cluster(cluster: &planner::ClusterView, this_queue: &str) {
    println!("Workers seen on the cluster (cached cluster reports):");
    if cluster.reports.is_empty() {
        println!("  (no workers — start one with `cori work` or `cori work --shared <name>`)");
        return;
    }
    let mut by_queue: std::collections::BTreeMap<&str, &CapabilityReport> = Default::default();
    for r in &cluster.reports {
        by_queue.insert(r.task_queue.as_str(), r);
    }
    for (queue, report) in by_queue {
        let mark = if queue == this_queue {
            "  (this machine)"
        } else {
            ""
        };
        let kind = match &report.identity {
            WorkerIdentity::Person { .. } => "user",
            WorkerIdentity::Service { .. } => "shared",
        };
        println!("  {queue:<32}  ({kind}){mark}");
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

fn print_pinned_remotes() -> anyhow::Result<()> {
    let pins = remote::pins::load()?;
    let trust = remote::trust::load()?;
    println!("Pinned remote workflows:");
    if pins.entries.is_empty() {
        println!("  (none)");
        return Ok(());
    }
    let runs_root = paths::runs_dir().ok();
    for (key, entry) in &pins.entries {
        // key = "host/repo//subpath@ref" — try to extract the (repo, sha) trust key
        let (repo_part, _ref_part) = key.split_once('@').unwrap_or((key.as_str(), ""));
        // repo_part = "host/repo//subpath"
        let host_repo = repo_part
            .split_once("//")
            .map(|(hr, _)| hr)
            .unwrap_or(repo_part);
        let trust_key = format!("{host_repo}@{}", entry.sha);
        let trusted = trust.entries.contains_key(&trust_key);
        let (run_count, last_run) = match runs_root.as_ref() {
            Some(root) => count_runs_for(root, repo_part),
            None => (0, None),
        };
        let trust_word = if trusted { "trusted" } else { "not trusted" };
        let run_str = if run_count == 1 {
            "1 run".to_string()
        } else {
            format!("{run_count} runs")
        };
        let last_str = match last_run {
            Some(when) => format!(", last run {}", chrono_humanize::HumanTime::from(when)),
            None => String::new(),
        };
        println!(
            "  {key:<60}  →  {sha}  ({trust_word}, {run_str}{last_str})",
            sha = short_sha(&entry.sha),
        );
    }
    Ok(())
}

fn short_sha(sha: &str) -> String {
    let n = 7.min(sha.len());
    sha[..n].to_string()
}

fn count_runs_for(
    runs_root: &std::path::Path,
    repo_part: &str,
) -> (usize, Option<chrono::DateTime<chrono::Utc>>) {
    // Re-derive the run-history key the same way `remote_run_history_key` does.
    // `repo_part` = "host/repo//subpath".
    let (host_repo, subpath) = repo_part.split_once("//").unwrap_or((repo_part, ""));
    let (host, repo) = match host_repo.split_once('/') {
        Some((h, rest)) => {
            // rest may itself contain `/` — repo path is everything until end.
            (h.to_string(), rest.to_string())
        }
        None => return (0, None),
    };
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(host.as_bytes());
    h.update(b"/");
    h.update(repo.as_bytes());
    h.update(b"//");
    h.update(subpath.as_bytes());
    let digest = h.finalize();
    let short = hex::encode(&digest[..4]);
    let leaf = if subpath.is_empty() {
        repo.rsplit('/').next().unwrap_or(&repo).to_string()
    } else {
        subpath.rsplit('/').next().unwrap_or(subpath).to_string()
    };
    let key = format!("{leaf}-{short}");
    let dir = runs_root.join(&key);
    if !dir.is_dir() {
        return (0, None);
    }
    let mut count = 0usize;
    let mut latest: Option<chrono::DateTime<chrono::Utc>> = None;
    if let Ok(it) = std::fs::read_dir(&dir) {
        for e in it.flatten() {
            let p = e.path();
            if p.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            count += 1;
            if let Ok(meta) = e.metadata()
                && let Ok(modified) = meta.modified()
                && let Ok(dt) = modified.duration_since(std::time::UNIX_EPOCH)
            {
                let when = chrono::DateTime::<chrono::Utc>::from_timestamp(dt.as_secs() as i64, 0);
                if let Some(when) = when
                    && latest.map(|cur| when > cur).unwrap_or(true)
                {
                    latest = Some(when);
                }
            }
        }
    }
    (count, latest)
}

fn hostname() -> Option<String> {
    std::env::var("HOSTNAME")
        .ok()
        .or_else(|| std::env::var("COMPUTERNAME").ok())
        .or_else(|| {
            std::process::Command::new("hostname")
                .output()
                .ok()
                .and_then(|o| {
                    let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
                    if s.is_empty() { None } else { Some(s) }
                })
        })
}
