// SPDX-License-Identifier: Apache-2.0
//! `deno check` as the dataflow prover.
use std::path::Path;
use std::process::Command;

pub struct CheckOutcome { pub ok: bool, pub diagnostics: Vec<String>, pub raw: String }

/// How long `deno check` and the schema export may run (they run on the server at publish, on builder code):
/// `CORI_CHECK_TIMEOUT_SECS`, default 180.
fn check_timeout() -> std::time::Duration {
    std::time::Duration::from_secs(std::env::var("CORI_CHECK_TIMEOUT_SECS").ok().and_then(|v| v.parse().ok()).filter(|s: &u64| *s > 0).unwrap_or(180))
}

/// `Command::output` with a deadline: the process is killed when it runs longer. Cori's own variables are removed from
/// its environment.
fn output_within(cmd: &mut Command, limit: std::time::Duration) -> std::io::Result<std::process::Output> {
    for k in crate::runtime::cori_env_vars() { cmd.env_remove(k); }
    let mut child = cmd.stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).spawn()?;
    // drain the pipes on threads so a chatty process cannot block on a full pipe while we wait
    let mut so = child.stdout.take(); let mut se = child.stderr.take();
    let t_out = std::thread::spawn(move || { let mut b = Vec::new(); if let Some(p) = so.as_mut() { let _ = std::io::Read::read_to_end(p, &mut b); } b });
    let t_err = std::thread::spawn(move || { let mut b = Vec::new(); if let Some(p) = se.as_mut() { let _ = std::io::Read::read_to_end(p, &mut b); } b });
    let start = std::time::Instant::now();
    let status = loop {
        if let Some(st) = child.try_wait()? { break st; }
        if start.elapsed() > limit {
            let _ = child.kill(); let _ = child.wait();
            return Err(std::io::Error::new(std::io::ErrorKind::TimedOut, format!("timed out after {}s (CORI_CHECK_TIMEOUT_SECS)", limit.as_secs())));
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    };
    Ok(std::process::Output { status, stdout: t_out.join().unwrap_or_default(), stderr: t_err.join().unwrap_or_default() })
}

pub fn deno_check(workflow_dir: &Path, step_files: &[String]) -> CheckOutcome {
    let files: Vec<std::path::PathBuf> = step_files.iter().map(|f| workflow_dir.join("steps").join(f)).collect();
    deno_check_paths(workflow_dir, workflow_dir, &files)
}

/// `deno check` of `files` with the generated configuration of the workflow folder `config_dir`; diagnostics name the
/// files from `root`. The workflows of one app share their configuration, so one call can check them all.
pub fn deno_check_paths(config_dir: &Path, root: &Path, files: &[std::path::PathBuf]) -> CheckOutcome {
    let config = config_dir.join(".cori").join("deno.json");
    let mut cmd = Command::new("deno");
    // no remote module: the server checks builder code at deploy and fetches no URL it names (npm packages resolve)
    cmd.arg("check").arg("--quiet").arg("--no-remote").arg("--config").arg(&config).arg("--no-lock");
    for f in files { cmd.arg(f); }
    match output_within(&mut cmd, check_timeout()) {
        Ok(out) => {
            let raw = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
            let raw = strip_ansi(&raw);
            let ok = out.status.success();
            let diagnostics = if ok { vec![] } else { parse_diagnostics(&raw, root) };
            CheckOutcome { ok, diagnostics, raw }
        }
        Err(e) => CheckOutcome { ok: false, diagnostics: vec![crate::runtime::deno_not_found(&e).unwrap_or_else(|| format!("could not run `deno check`: {e}"))], raw: String::new() },
    }
}

fn strip_ansi(s: &str) -> String {
    let mut out = String::new(); let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' { while let Some(n) = chars.next() { if n.is_ascii_alphabetic() { break; } } } else { out.push(c); }
    }
    out
}

/// Turn Deno's diagnostics into `file:line: message` lines.
fn parse_diagnostics(raw: &str, root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let lines: Vec<&str> = raw.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let l = lines[i].trim_start();
        if let Some(rest) = l.strip_prefix("TS") {
            // "TS2339 [ERROR]: Property 'x' does not exist on type ..."
            let msg = rest.splitn(2, ": ").nth(1).unwrap_or(rest).to_string();
            let code = rest.split(' ').next().unwrap_or("");
            // location follows a few lines later: "    at file:///.../steps/load_ticket.ts:9:31"
            let mut loc = String::new();
            for j in i + 1..(i + 12).min(lines.len()) {
                let t = lines[j].trim_start();
                if let Some(at) = t.strip_prefix("at file://") { loc = at.to_string(); break; }
            }
            let loc = loc.strip_prefix(&root.display().to_string()).map(|s| s.trim_start_matches('/').to_string()).unwrap_or(loc);
            out.push(format!("{loc}: TS{code} {msg}"));
        }
        i += 1;
    }
    if out.is_empty() { out.push(raw.trim().to_string()); }
    out
}

/// Runs `schemas.ts` in the sandbox to export every step's output JSON Schema.
pub fn dump_schemas(workflow_dir: &Path, step_files: &[String]) -> Result<std::collections::BTreeMap<String, serde_json::Value>, String> {
    let rt = crate::runtime::runtime()?;
    // the runner's sandbox (it runs the steps' module code); steps may import the app's `lib/`, which is readable
    let app_root = crate::app_root_of(workflow_dir);
    let out = output_within(Command::new("deno").args(crate::runtime::sandbox_run_args(workflow_dir)?)
        .arg(&rt.schemas).arg(workflow_dir).args(step_files), check_timeout()).map_err(|e| crate::runtime::deno_not_found(&e).unwrap_or_else(|| format!("exporting the output schemas (runs the steps' module code): {e}")))?;
    if !out.status.success() {
        let err = strip_ansi(&String::from_utf8_lossy(&out.stderr)).trim().to_string();
        if err.contains("Requires read access") || err.contains("NotCapable") {
            return Err(format!("a step imports a file outside the app folder {} (steps may import the workflow's own files and the app's lib/, nothing above cori.toml): {}", app_root.display(), err.lines().next().unwrap_or("")));
        }
        return Err(err);
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text.lines().rev().find(|l| l.trim_start().starts_with('{')).ok_or("no schema output")?;
    serde_json::from_str(line).map_err(|e| e.to_string())
}

/// `dump_schemas` of several workflows of one app in one process (`schemas.ts --workflows`): each one's, in order. The
/// process may read the app folder (the first workflow's sandbox: its app folder is theirs too).
pub fn dump_schemas_all(workflows: &[(std::path::PathBuf, Vec<String>)]) -> Result<Vec<std::collections::BTreeMap<String, serde_json::Value>>, String> {
    let Some((first, _)) = workflows.first() else { return Ok(Vec::new()) };
    let rt = crate::runtime::runtime()?;
    let each = serde_json::to_string(&workflows.iter().map(|(d, files)| serde_json::json!([d.display().to_string(), files])).collect::<Vec<_>>()).map_err(|e| e.to_string())?;
    let out = output_within(Command::new("deno").args(crate::runtime::sandbox_run_args(first)?).arg(&rt.schemas).arg("--workflows").arg(&each), check_timeout())
        .map_err(|e| crate::runtime::deno_not_found(&e).unwrap_or_else(|| format!("exporting the output schemas (runs the steps' module code): {e}")))?;
    if !out.status.success() { return Err(strip_ansi(&String::from_utf8_lossy(&out.stderr)).trim().to_string()); }
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text.lines().rev().find(|l| l.trim_start().starts_with('[')).ok_or("no schema output")?;
    let all: Vec<std::collections::BTreeMap<String, serde_json::Value>> = serde_json::from_str(line).map_err(|e| e.to_string())?;
    if all.len() != workflows.len() { return Err(format!("{} schema sets for {} workflows", all.len(), workflows.len())); }
    Ok(all)
}
