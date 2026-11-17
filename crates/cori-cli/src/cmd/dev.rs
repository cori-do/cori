// SPDX-License-Identifier: Apache-2.0
//! Building an app on this machine: `cori check` (compile), `cori run` (a local run on a Temporal dev server), `cori gen
//! client`, and `cori doctor`.
use crate::cli::*;
use crate::client::Api;
use crate::{commands, embedded_pg, session};
use std::path::PathBuf;

pub async fn run(cmd: Cmd, resolved: &session::Resolved) -> anyhow::Result<()> {
    match cmd {
        Cmd::Doctor { conn: Conn { server, token } } => {
            let mut bad = 0;
            let mut check = |name: &str, ok: bool, detail: String| { println!("  {} {:<22} {}", if ok { "●" } else { "✕" }, name, detail); if !ok { bad += 1; } };
            let which = |b: &str| std::process::Command::new(b).arg("--version").output().ok().filter(|o| o.status.success()).map(|o| String::from_utf8_lossy(&o.stdout).lines().next().unwrap_or("").to_string());
            println!("machine");
            let d = which("deno"); check("deno", d.is_some(), d.clone().unwrap_or_else(|| "not found (https://deno.land)".into()));
            let t = which("temporal"); check("temporal cli", t.is_some(), t.clone().unwrap_or_else(|| "not found (only needed for `cori run` / `cori up`)".into()));
            let tcp = std::net::TcpStream::connect_timeout(&"127.0.0.1:7233".parse().unwrap(), std::time::Duration::from_millis(500)).is_ok();
            check("temporal :7233", tcp, if tcp { "reachable".into() } else { "not running (`cori up` starts a dev server)".into() });
            // the connection every server-touching command resolves (flags/env, .cori/session, your logins, local dev default)
            let s = server;
            let answers = commands::server_answers(&s).await;
            let logins = session::global_path().map(|p| p.display().to_string()).unwrap_or_else(|| "~/.cori/config.toml".into());
            let source = match &resolved.source { session::Source::Env => "--token / CORI_TOKEN".to_string(), session::Source::Session(p) => format!("{} (its own token)", p.display()), session::Source::Login => format!("your login to it ({logins})"), session::Source::Default if session::is_local(&s) => format!("local dev default (no CORI_SERVER, .cori/session or login to it in {logins})"), session::Source::Default => format!("not signed in to it: `cori login {s}`") };
            let default = matches!(resolved.source, session::Source::Default);
            let signed_out = default && !session::is_local(&s);
            check("connection", !signed_out && (!default || answers), if default && !answers && !signed_out { format!("none: {source}; nothing answers at {s} (`cori up`, or `cori login <url>`)") } else { format!("{source} · {s}") });
            if answers || !default {
                println!("server {s}");
                let c = cori_engine::http::builder().user_agent(crate::client::USER_AGENT).timeout(std::time::Duration::from_secs(5)).build()?;
                let res = c.get(format!("{}/v1/doctor", s.trim_end_matches('/'))).bearer_auth(&token).send().await;
                // the oldest command line the server serves, against this one
                if let Some(min) = res.as_ref().ok().and_then(|r| r.headers().get("x-cori-cli-min")?.to_str().ok().map(String::from)) {
                    let this = env!("CARGO_PKG_VERSION");
                    let ok = !crate::client::older(this, &min);
                    check("cli", ok, if ok { format!("cori {this} (the server serves {min} or newer)") } else { format!("cori {this}, older than the {min} the server serves: `npm install -g @cori-do/cli`") });
                }
                // on a server of several organizations: the one the token acts in, and the folder's
                // (without the folder's organization in the request: a mismatch is what this tells)
                if let Some(me) = match c.get(format!("{}/v1/me", s.trim_end_matches('/'))).bearer_auth(&token).send().await { Ok(r) if r.status().is_success() => r.json::<serde_json::Value>().await.ok(), _ => None } {
                    let theirs = resolved.session.as_ref().and_then(|f| f.organization.clone());
                    match (session::Organization::of_me(&me), theirs) {
                        (Some(o), Some(f)) if o.id != f.id => check("organization", false, format!("your token is for {o}, and this folder belongs to {f}: `cori login {s} --org {f}`")),
                        (Some(o), Some(_)) => check("organization", true, format!("{o} (this folder's)")),
                        (Some(o), None) => check("organization", true, o.to_string()),
                        (None, _) => {}
                    }
                }
                match res {
                    Ok(r) if r.status().is_success() => { let v: serde_json::Value = r.json().await?; check("reachable", true, "healthz ok".into()); for ch in v["checks"].as_array().cloned().unwrap_or_default() { let detail = ch["detail"].as_str().unwrap_or(""); check(ch["name"].as_str().unwrap_or(""), ch["ok"].as_bool().unwrap_or(false), match ch["fix"].as_str() { Some(f) => format!("{detail} · fix: {f}"), None => detail.to_string() }); } }
                    Ok(r) if answers && r.status() == reqwest::StatusCode::FORBIDDEN => check("reachable", true, "healthz ok · server checks need an admin role".into()),
                    Ok(r) => check("reachable", false, format!("{} {}", r.status(), r.text().await.unwrap_or_default())),
                    Err(e) => check("reachable", false, e.to_string()),
                }
            }
            if bad > 0 { std::process::exit(1); }
        }
        Cmd::Check { dirs, json, no_deno, conventions, conn: Conn { server, token } } => {
            if conventions {
                if json { println!("{}", serde_json::to_string_pretty(&serde_json::json!({ "agents_md": cori_compiler::authoring::AGENTS_MD, "step_kinds": cori_compiler::authoring::step_kinds(), "result": cori_compiler::authoring::result_conventions() }))?); }
                else { println!("{}", cori_compiler::authoring::AGENTS_MD); println!("## Step kinds\n{}", serde_json::to_string_pretty(&cori_compiler::authoring::step_kinds())?); println!("## Result templates\n{}", serde_json::to_string_pretty(&cori_compiler::authoring::result_conventions())?); }
                return Ok(());
            }
            let mut failed = false;
            let (rows, _) = registry(&server, &token).await;
            // an app folder (cori.toml at its root) means every workflow under workflows/, plus the stores against their migrations
            let opts = cori_compiler::Options { run_deno: !no_deno, sdk_path: None, connectors: rows.clone(), deno: None };
            let mut results: Vec<(PathBuf, Result<cori_compiler::Compiled, Vec<String>>)> = Vec::new();
            for d in dirs {
                if !d.join("cori.toml").exists() { let r = cori_compiler::compile(&d, &opts); results.push((d, r)); continue; }
                let app = cori_compiler::compile_app(&d, &opts);
                if app.workflows.is_empty() { eprintln!("{}: no workflow under workflows/", d.display()); failed = true; }
                let r = &app.stores;
                if json { println!("{}", serde_json::to_string_pretty(&serde_json::json!({ "app": d, "stores": r, "warnings": app.warnings }))?); }
                else {
                    for w in &app.warnings { println!("warning       {w}"); }
                    if !r.stores.is_empty() || !r.errors.is_empty() { println!("stores        {} · {} error(s), {} warning(s)", if r.stores.is_empty() { "-".to_string() } else { r.stores.join(", ") }, r.errors.len(), r.warnings.len()); for e in &r.errors { println!("  - error: {e}"); } for w in &r.warnings { println!("  - {w}"); } }
                }
                if !r.errors.is_empty() { failed = true; }
                results.extend(app.workflows);
            }
            for (d, r) in results {
                match r {
                    Ok(c) => { if json { println!("{}", serde_json::to_string_pretty(&c)?); } else { print!("{}", cori_compiler::render(&c)); } }
                    Err(errs) => { failed = true; eprintln!("{}: {} error(s)", d.display(), errs.len()); for e in errs { eprintln!("  - {e}"); } }
                }
            }
            if failed { std::process::exit(1); }
        }
        Cmd::Run { dir, params, decide, json, temporal, namespace, no_mock, user, store_database_url, no_store, reset_store, record, mock_llm, conn: Conn { server, token } } => {
            let (registry_rows, registry_map) = registry(&server, &token).await;
            let record_dir = if record { Some(std::env::temp_dir().join(format!("cori-record-{}", std::process::id()))) } else { None };
            let mut store_server = None;
            let app_dir = app_of_workflow(&dir)?;
            // the store: an explicit admin URL, else the embedded Postgres of a running `cori up` (same data as the hosted app), else the fixtures
            let admin_url = match store_database_url {
                Some(u) => Some(u),
                None if !no_store && app_dir.join("stores").is_dir() => match embedded_pg::running_url(&dirs::home_dir().unwrap().join(".cori/up/pg")) { Some(u) => { eprintln!("store: using the embedded Postgres of `cori up` (fixtures ignored; --no-store to use them)"); Some(u) } None => None },
                None => None,
            };
            if reset_store && admin_url.is_none() { eprintln!("store: --reset-store ignored: no store database (start `cori up`, or pass --store-database-url); the fixtures answer store steps"); }
            if let Some(admin) = admin_url {
                let local = cori_server::stores::local_endpoint(&admin, &app_dir, &dirs::home_dir().unwrap().join(".cori/local"), reset_store).await?;
                for n in &local.notes { eprintln!("{n}"); }
                store_server = Some(local.url);
            }
            let mut opts = cori_engine::run::RunOptions { temporal, namespace, no_mocks: no_mock, store_server, registry: registry_map, registry_rows, record_dir: record_dir.as_ref().map(|p| p.display().to_string()), mock_llm, ..Default::default() };
            // the endpoint's roles: the default identity holds the first one; an explicit --user is checked against them
            let allow_roles: Vec<String> = std::fs::read_to_string(dir.join("workflow.yaml")).ok().and_then(|t| cori_compiler::manifest::parse(&t).ok()).and_then(|m| m.endpoint).and_then(|e| e.allow).and_then(|a| a.get("roles").and_then(|r| r.as_array()).map(|r| r.iter().filter_map(|x| x.as_str().map(String::from)).collect())).unwrap_or_default();
            match user {
                Some(u) => {
                    let u = u.strip_prefix("dev:").unwrap_or(&u);
                    let (sub, roles) = u.split_once(':').unwrap_or((u, ""));
                    let roles: Vec<&str> = roles.split(',').filter(|r| !r.is_empty()).collect();
                    if sub.is_empty() || roles.iter().any(|r| r.contains(':')) { anyhow::bail!("--user is <sub>:<role,role> (got `{u}`), e.g. --user jean:{}", allow_roles.first().map(|s| s.as_str()).unwrap_or("app_user")); }
                    if !allow_roles.is_empty() && !roles.iter().any(|r| allow_roles.iter().any(|a| a == r)) { eprintln!("warning: --user {sub}:{} holds none of the endpoint's roles [{}]; the hosted endpoint would answer 403", roles.join(","), allow_roles.join(", ")); }
                    opts.user = serde_json::json!({ "sub": sub, "roles": roles, "email": format!("{sub}@local") });
                }
                None => if let Some(r) = allow_roles.first() { opts.user = serde_json::json!({ "sub": "dev", "roles": [r], "email": "dev@local" }); eprintln!("user: dev:{r} (the endpoint's first allow role; --user <sub>:<roles> to change)"); },
            }
            for p in params { let (k, v) = parse_kv(&p); opts.params[k] = v; }
            for d in decide { let (k, v) = parse_kv(&d); opts.decisions.insert(k, v); }
            let (compiled, out) = cori_engine::run::run_local(&dir, opts).await?;
            if let Some(rd) = &record_dir { for t in merge_recordings(&app_dir, rd)? { eprintln!("recorded {t}"); } std::fs::remove_dir_all(rd).ok(); }
            // a failed run exits 1 with --json too: its document says why, the exit code says it failed
            if json { println!("{}", serde_json::to_string_pretty(&out)?); if out.error.is_some() { std::process::exit(1); } return Ok(()); }
            println!("run {} · {} · {}", out.run_id, compiled.id, out.status);
            for t in &out.trace {
                let mark = match t.status.as_str() { "ok" => "●", "not_taken" => "○", "failed" => "✕", _ => "?" };
                let tries = if t.attempts > 1 { format!("{} attempts · ", t.attempts) } else { String::new() };
                // a builtin says which way it went
                let said = t.error.clone().unwrap_or_else(|| t.notes.join(" · "));
                let said = match &t.path { Some(p) if said.is_empty() => format!("→ {p}"), Some(p) => format!("→ {p} · {said}"), None => said };
                println!("  {mark} {:<24} {:<9} {:>6} ms  {tries}{said}", t.name, t.kind, t.ms);
            }
            if let Some(r) = &out.result { println!("result: {}", r.get("headline").and_then(|h| h.as_str()).unwrap_or("")); for f in r.get("fields").and_then(|f| f.as_array()).cloned().unwrap_or_default() { println!("  {:<12} {}", f["label"].as_str().unwrap_or(""), f["value"]); } }
            if let Some(e) = &out.error { match out.error_detail.as_ref().and_then(|d| d.step.as_deref()).filter(|s| !s.is_empty()) { Some(step) => eprintln!("error in {step}: {e}"), None => eprintln!("error: {e}") } std::process::exit(1); }
        }
        Cmd::Gen { what: GenCmd::Client { dir, app, out, conn: Conn { server, token } } } => {
            let text = if let Some(d) = dir {
                let app_id = app.or_else(|| std::fs::read_to_string(d.join("cori.toml")).ok().and_then(|t| toml::from_str::<toml::Value>(&t).ok()).and_then(|t| t.get("app")?.get("id")?.as_str().map(String::from))).ok_or_else(|| anyhow::anyhow!("--dir must be the app folder (cori.toml with [app] id at its root), or pass --app"))?;
                let wfs = cori_server::bundles::compile_all(&d).map_err(|e| anyhow::anyhow!("check failed:\n  {}", e.join("\n  ")))?;
                cori_server::clientgen::client_ts(&app_id, &wfs)
            } else {
                let app = app.ok_or_else(|| anyhow::anyhow!("--app is required without --dir"))?;
                Api::new(&server, &token).text(&format!("/v1/apps/{app}/client.ts")).await?
            };
            if let Some(parent) = out.parent() { std::fs::create_dir_all(parent)?; }
            std::fs::write(&out, text)?;
            println!("wrote {}", out.display());
        }
        _ => unreachable!("main dispatches only this module's commands"),
    }
    Ok(())
}

/// `k=v` → (k, v parsed as JSON, else the text). No type check here: `run_local` validates against the manifest (`done=maybe` is rejected there).
pub(crate) fn parse_kv(s: &str) -> (String, serde_json::Value) {
    let (k, v) = s.split_once('=').unwrap_or((s, "true"));
    let val = serde_json::from_str(v).unwrap_or(serde_json::Value::String(v.to_string()));
    (k.to_string(), val)
}

/// The app root of a workflow folder (`<app>/workflows/<id>` → `<app>`), checked for cori.toml.
pub fn app_of_workflow(workflow_dir: &std::path::Path) -> anyhow::Result<PathBuf> {
    let w = workflow_dir.canonicalize().map_err(|e| anyhow::anyhow!("{}: {e}", workflow_dir.display()))?;
    if !w.join("workflow.yaml").exists() { anyhow::bail!("{} has no workflow.yaml: pass a workflow folder, <app>/workflows/<id>", workflow_dir.display()); }
    let app = w.parent().and_then(|p| p.parent()).ok_or_else(|| anyhow::anyhow!("{} is not under <app>/workflows/", workflow_dir.display()))?;
    if !app.join("cori.toml").exists() { anyhow::bail!("{} has no cori.toml (a workflow lives in <app>/workflows/<id>/)", app.display()); }
    Ok(app.to_path_buf())
}

/// Best-effort registry fetch (2 s) for typing and resolution; empty when no server is reachable, said when the server
/// refuses it (`fetch_registry`).
async fn registry(server: &str, token: &str) -> (Vec<serde_json::Value>, std::collections::BTreeMap<String, cori_engine::config::ConnectorConfig>) {
    let (rows, refused) = fetch_registry(server, token).await;
    if let Some(w) = refused { eprintln!("{w}"); }
    let mut map = std::collections::BTreeMap::new();
    for row in &rows { if let Some(n) = row["name"].as_str() { let cfg = serde_json::json!({ "kind": row["kind"], "audience": row["audience"], "secret": row["secret"], "worker": row["worker"], "url": row["transport"]["url"], "command": row["transport"]["command"], "base_url": row["transport"]["base_url"], "task_queue": row["transport"]["task_queue"] }); if let Ok(c) = serde_json::from_value(cfg) { map.insert(n.to_string(), c); } } }
    (rows, map)
}

/// The registry's rows (`GET /v1/connectors`), and the warning to give when the server answers with an error (a login
/// that expired, a member who builds no app): check and run then go on without its connectors, so the tool inputs are
/// typed nowhere and a connector only the registry declares is not found. No server answering (offline) says nothing.
async fn fetch_registry(server: &str, token: &str) -> (Vec<serde_json::Value>, Option<String>) {
    let api = Api::with_timeout(server, token, std::time::Duration::from_secs(2));
    match api.call(api.request(reqwest::Method::GET, "/v1/connectors")).await {
        Ok((status, v)) if status.is_success() => (v.as_array().cloned().unwrap_or_default(), None),
        Ok((status, body)) => (Vec::new(), Some(format!("warning: {server} refused the connector registry, so the tool inputs of its connectors are not typed and only the connectors cori.toml declares are found: {}", api.error(status, &body)))),
        Err(_) => (Vec::new(), None),
    }
}

/// Merge `<record_dir>/<connector>.jsonl` lines into `<app>/fixtures/<connector>.json` and point cori.toml at it.
fn merge_recordings(app_dir: &std::path::Path, record_dir: &std::path::Path) -> anyhow::Result<Vec<String>> {
    let mut touched = Vec::new();
    let Ok(rd) = std::fs::read_dir(record_dir) else { return Ok(touched) };
    for e in rd.filter_map(|e| e.ok()) {
        let p = e.path(); if p.extension().map(|x| x != "jsonl").unwrap_or(true) { continue; }
        let connector = p.file_stem().unwrap().to_string_lossy().to_string();
        let fx_dir = app_dir.join("fixtures"); std::fs::create_dir_all(&fx_dir)?;
        let fx = fx_dir.join(format!("{connector}.json"));
        let mut doc: serde_json::Value = if fx.exists() { serde_json::from_str(&std::fs::read_to_string(&fx)?).unwrap_or(serde_json::json!({})) } else { serde_json::json!({}) };
        let mut n = 0;
        for line in std::fs::read_to_string(&p)?.lines() { let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { continue }; let section = v["section"].as_str().unwrap_or("tools"); let key = v["key"].as_str().unwrap_or(""); if section == "query" { doc["query"] = v["value"].clone(); } else { if !doc[section].is_object() { doc[section] = serde_json::json!({}); } doc[section][key] = v["value"].clone(); } n += 1; }
        std::fs::write(&fx, serde_json::to_string_pretty(&doc)?)?;
        // point the app at the fixture (an override block is enough when the connector comes from the registry)
        let toml_path = app_dir.join("cori.toml");
        let mut text = std::fs::read_to_string(&toml_path).unwrap_or_default();
        if !text.contains(&format!("[connectors.{connector}.dev]")) { text.push_str(&format!("\n[connectors.{connector}.dev]\nmock = \"fixtures/{connector}.json\"   # recorded by `cori run --record`\n")); std::fs::write(&toml_path, text)?; }
        touched.push(format!("fixtures/{connector}.json ({n} entries)"));
        std::fs::remove_file(&p).ok();
    }
    Ok(touched)
}

#[cfg(test)]
mod tests {
    use super::fetch_registry;

    /// A stand-in server that answers every request with `status` and the JSON `body`.
    fn answering(status: &'static str, body: &'static str) -> String {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || for stream in listener.incoming() {
            let Ok(mut s) = stream else { continue };
            // the request's headers (a GET has no body) before answering
            let (mut req, mut buf) = (Vec::new(), [0u8; 4096]);
            while let Ok(n) = s.read(&mut buf) { if n == 0 { break; } req.extend_from_slice(&buf[..n]); if req.windows(4).any(|w| w == b"\r\n\r\n") { break; } }
            let _ = s.write_all(format!("HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len()).as_bytes());
        });
        base
    }

    #[tokio::test]
    async fn a_refused_registry_is_said_and_an_unreachable_server_is_not() {
        // a member who builds no app is refused it: said, with the server's own reason
        let refused = answering("403 Forbidden", r#"{"error":"your roles open the organization's apps: `GET /v1/connectors` is for those who build them or administer it"}"#);
        let (rows, warning) = fetch_registry(&refused, "dev:alice:support_agent").await;
        let warning = warning.expect("a refusal is said");
        assert!(rows.is_empty());
        assert!(warning.starts_with(&format!("warning: {refused} refused the connector registry")), "{warning}");
        assert!(warning.ends_with(": 403 Forbidden: your roles open the organization's apps: `GET /v1/connectors` is for those who build them or administer it"), "{warning}");
        // a builder gets its rows, and nothing is said
        let open = answering("200 OK", r#"[{"name":"helpdesk","kind":"mcp","transport":{"command":["deno","run","helpdesk.ts"]}}]"#);
        let (rows, warning) = fetch_registry(&open, "dev:bo:builder").await;
        assert_eq!((rows.len(), warning), (1, None));
        // no server answering (offline): nothing to type against, and nothing said
        let gone = { let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap(); format!("http://{}", l.local_addr().unwrap()) };
        assert_eq!(fetch_registry(&gone, "dev:bo:builder").await, (Vec::new(), None));
    }
}
