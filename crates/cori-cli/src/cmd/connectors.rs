// SPDX-License-Identifier: Apache-2.0
//! `cori connectors …`: the org's connector registry (add, list, find, export, requests, refresh, remove).
use crate::cli::*;
use crate::client::Api;
use crate::commands;
use std::path::PathBuf;

pub async fn run(cmd: Cmd) -> anyhow::Result<()> {
    match cmd {
        Cmd::Connectors { what } => {
            match what {
            ConnectorsCmd::Add { url, yaml, name, kind, command, description, audience, secret, worker, roles, base_url, auth_env, from, conn: Conn { server, token } } => {
                let api = Api::new(&server, &token);
                if url.as_deref() == Some("activities") || yaml.is_some() {
                    // activities.yaml → connector kind `activity` (task queue, audience) + one declaration per activity, with the
                    // referenced JSON schemas inlined, so `cori scaffold` writes typed steps and the change review knows the effects
                    let path = yaml.ok_or_else(|| anyhow::anyhow!("`cori connectors add activities --yaml <activities.yaml>`"))?;
                    let doc: serde_json::Value = serde_yaml::from_str(&std::fs::read_to_string(&path)?)?;
                    let dir = path.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| PathBuf::from("."));
                    let resolve = |v: &serde_json::Value| -> serde_json::Value { match v.get("$ref").and_then(|r| r.as_str()) { Some(r) => std::fs::read_to_string(dir.join(r)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(serde_json::json!({ "type": "object" })), None => v.clone() } };
                    let cname = name.clone().or_else(|| doc["name"].as_str().map(String::from)).ok_or_else(|| anyhow::anyhow!("--name or `name:` in the yaml"))?;
                    // the engine reads each activity's timeouts (cori_compiler::timing): refuse one it would not
                    for a in doc["activities"].as_array().cloned().unwrap_or_default() { for (k, v) in a["timeouts"].as_object().cloned().unwrap_or_default() { if let Some(t) = v.as_str() { cori_compiler::timing::duration(t).map_err(|e| anyhow::anyhow!("{}: activity `{}` timeouts.{k} {e}", path.display(), a["name"].as_str().unwrap_or("?")))?; } } }
                    let tools: Vec<serde_json::Value> = doc["activities"].as_array().cloned().unwrap_or_default().iter().map(|a| { let eff = a.get("effects").cloned().unwrap_or(serde_json::json!({})); serde_json::json!({ "name": a["name"], "version": a.get("version").cloned().unwrap_or(serde_json::json!(1)), "description": a.get("description").cloned().unwrap_or(serde_json::json!("")), "inputSchema": a.get("input").map(&resolve).unwrap_or(serde_json::json!({ "type": "object" })), "outputSchema": a.get("output").map(&resolve), "annotations": { "readOnlyHint": eff["read_only"].as_bool().unwrap_or(false), "destructiveHint": eff["destructive"].as_bool().unwrap_or(false), "idempotentHint": eff["idempotent"].as_bool().unwrap_or(false) }, "timeouts": a.get("timeouts"), "retry": a.get("retry") }) }).collect();
                    let body = serde_json::json!({ "name": cname, "kind": "activity", "task_queue": doc["task_queue"], "audience": audience.or_else(|| doc["auth"]["audience"].as_str().map(String::from)), "description": description.unwrap_or_else(|| format!("native activities from {}", path.display())), "roles": roles, "tools": tools, "worker": worker, "secret": secret });
                    let v = api.post("/v1/connectors", &body).await?;
                    println!("declared {} (activity) on {} · activities: {}", v["name"].as_str().unwrap_or(""), doc["task_queue"].as_str().unwrap_or("?"), v["tools"].as_array().map(|a| a.iter().filter_map(|t| t.as_str()).collect::<Vec<_>>().join(", ")).unwrap_or_default());
                    return Ok(());
                }
                let declare = |body: serde_json::Value| { let api = &api; async move { match api.post("/v1/connectors", &body).await {
                    Ok(v) => { println!("declared {} ({}) · tools: {}", v["name"].as_str().unwrap_or(""), v["kind"].as_str().unwrap_or(""), v["tools"].as_array().map(|a| a.iter().filter_map(|t| t.as_str()).collect::<Vec<_>>().join(", ")).unwrap_or_default()); anyhow::Ok(true) }
                    Err(e) => { eprintln!("{}: {e}", body["name"].as_str().unwrap_or("?")); anyhow::Ok(false) }
                } } };
                if let Some(f) = from {
                    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&f)?)?;
                    let servers = v.get("mcpServers").and_then(|m| m.as_object()).cloned().ok_or_else(|| anyhow::anyhow!("{} has no `mcpServers` object", f.display()))?;
                    let mut n = 0;
                    for (name, s) in servers {
                        let mut cmd: Vec<String> = s.get("command").and_then(|c| c.as_str()).map(|c| vec![c.to_string()]).unwrap_or_default();
                        cmd.extend(s.get("args").and_then(|a| a.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect::<Vec<_>>()).unwrap_or_default());
                        let name = name.replace(|c: char| !(c.is_alphanumeric() || c == '_' || c == '-'), "_");
                        let mut body = serde_json::json!({ "name": name, "kind": "mcp", "url": s.get("url"), "command": if cmd.is_empty() { serde_json::Value::Null } else { serde_json::json!(cmd) }, "description": format!("imported from {}", f.display()), "roles": roles });
                        if s.get("url").is_none() && !cmd.is_empty() { if let Some(t) = stdio_tools(&name, &cmd).await { body["tools"] = serde_json::json!(t); } }
                        if declare(body).await? { n += 1; }
                    }
                    println!("imported {n} connector(s) from {}", f.display());
                } else {
                    let name = name.or_else(|| url.as_ref().and_then(|u| reqwest::Url::parse(u).ok()).and_then(|u| u.host_str().map(|h| h.split('.').next().unwrap_or("mcp").to_string()))).ok_or_else(|| anyhow::anyhow!("--name is required"))?;
                    let mut body = serde_json::json!({ "name": name, "kind": kind, "url": url, "command": command, "description": description.unwrap_or_default(), "audience": audience, "secret": secret, "worker": worker, "roles": roles, "base_url": base_url, "auth_env": auth_env });
                    if let (true, None, Some(cmd)) = (kind == "mcp", &url, &command) { if let Some(t) = stdio_tools(&name, cmd).await { body["tools"] = serde_json::json!(t); } }
                    if !declare(body).await? { std::process::exit(1); }
                }
            }
                ConnectorsCmd::Find { query, json, conn: Conn { server, token } } => { let rows = commands::registry_rows(&server, &token, None).await?; let hits = cori_compiler::authoring::find_tools(&rows, &query.join(" ")); if json { println!("{}", serde_json::to_string_pretty(&hits)?); } else if hits.is_empty() { println!("no tool matches `{}` (see `cori connectors list`; ask an admin for a new connector)", query.join(" ")); } else { for h in hits { println!("{:<18} {:<28} {}", h["connector"].as_str().unwrap_or(""), h["tool"].as_str().unwrap_or(""), h["description"].as_str().unwrap_or("").chars().take(90).collect::<String>()); } println!("next: `cori scaffold <connector> <tool> --workflow <id>`"); } }
                ConnectorsCmd::List { json, conn: Conn { server, token } } => { let v = Api::new(&server, &token).get("/v1/connectors").await?; if json { println!("{}", serde_json::to_string_pretty(&v)?); return Ok(()); } for x in v.as_array().cloned().unwrap_or_default() { let tools: Vec<&str> = x["tools"].as_array().map(|a| a.iter().filter_map(|t| t["name"].as_str()).collect()).unwrap_or_default(); println!("{:<20} {:<9} {:<40} {}", x["name"].as_str().unwrap_or(""), x["kind"].as_str().unwrap_or(""), x["transport"]["url"].as_str().map(String::from).or_else(|| x["transport"]["command"].as_array().map(|a| a.iter().filter_map(|s| s.as_str()).collect::<Vec<_>>().join(" "))).unwrap_or_default().chars().take(40).collect::<String>(), if tools.is_empty() { x["description"].as_str().unwrap_or("").to_string() } else { format!("{} tools: {}", tools.len(), tools.join(", ")) }); } }
                ConnectorsCmd::Export { to, conn: Conn { server, token } } => {
                    let v = Api::new(&server, &token).get("/v1/connectors").await?;
                    let mut doc: serde_json::Value = if to.exists() { serde_json::from_str(&std::fs::read_to_string(&to)?).unwrap_or(serde_json::json!({})) } else { serde_json::json!({}) };
                    if !doc["mcpServers"].is_object() { doc["mcpServers"] = serde_json::json!({}); }
                    let mut n = 0; let mut needs = Vec::new();
                    for x in v.as_array().cloned().unwrap_or_default() { if x["kind"].as_str() != Some("mcp") { continue; } let name = x["name"].as_str().unwrap_or("").to_string(); let entry = if let Some(u) = x["transport"]["url"].as_str() { serde_json::json!({ "type": "http", "url": u }) } else if let Some(cmd) = x["transport"]["command"].as_array() { let parts: Vec<&str> = cmd.iter().filter_map(|s| s.as_str()).collect(); serde_json::json!({ "command": parts.first().copied().unwrap_or(""), "args": parts.iter().skip(1).collect::<Vec<_>>() }) } else { continue }; if let Some(sec) = x["secret"].as_str() { needs.push(format!("{name}: credential `{sec}` lives in the Cori broker; set it in your host's env or auth config")); } doc["mcpServers"][&name] = entry; n += 1; }
                    std::fs::write(&to, serde_json::to_string_pretty(&doc)?)?;
                    println!("wrote {n} server(s) to {}", to.display()); for w in needs { println!("  note: {w}"); }
                }
                ConnectorsCmd::Requests { conn: Conn { server, token } } => { let v = Api::new(&server, &token).get("/v1/connector-requests").await?; let a = v.as_array().cloned().unwrap_or_default(); if a.is_empty() { println!("no open requests"); } for r in a { println!("{}  {:<40} by {:<12} {}  → cori connectors add {} --name <name>", r["at"].as_str().unwrap_or("").get(..19).unwrap_or(""), r["url"].as_str().unwrap_or(""), r["requested_by"].as_str().unwrap_or(""), r["reason"].as_str().unwrap_or(""), r["url"].as_str().unwrap_or("")); } }
                ConnectorsCmd::Key { name, conn: Conn { server, token } } => {
                    // the key alone on stdout (`CORI_CONNECTOR_KEY=$(cori connectors key billing)`), the hint on stderr
                    let v = Api::new(&server, &token).get(&format!("/v1/connectors/{name}/key")).await?;
                    println!("{}", v["key"].as_str().unwrap_or(""));
                    eprintln!("set it as CORI_CONNECTOR_KEY on {name}'s worker: the Cori SDK opens each run's token with it");
                }
                ConnectorsCmd::Refresh { name, conn: Conn { server, token } } => {
                    let api = Api::new(&server, &token);
                    // an MCP server over stdio: the server never starts a program, so its tools are listed here
                    let rows = api.get("/v1/connectors").await?;
                    let row = rows.as_array().into_iter().flatten().find(|r| r["name"].as_str() == Some(name.as_str()));
                    let cmd: Option<Vec<String>> = row.filter(|r| r["kind"] == "mcp" && r["transport"]["url"].is_null()).and_then(|r| serde_json::from_value(r["transport"]["command"].clone()).ok());
                    let body = match cmd { Some(cmd) => match stdio_tools(&name, &cmd).await { Some(t) => serde_json::json!({ "tools": t }), None => std::process::exit(1) }, None => serde_json::json!({}) };
                    let v = api.post(&format!("/v1/connectors/{name}/refresh"), &body).await?; println!("{}", v);
                }
                ConnectorsCmd::Remove { name, conn: Conn { server, token } } => { let v = Api::new(&server, &token).delete(&format!("/v1/connectors/{name}")).await?; println!("{}", v); }
            }
        }
        _ => unreachable!("main dispatches only this module's commands"),
    }
    Ok(())
}

/// The tools of an MCP server over stdio, listed by starting it here (`tools/list`): the server never starts a program, so
/// a declaration brings them. None, after saying why, when the command does not run on this machine.
async fn stdio_tools(name: &str, command: &[String]) -> Option<Vec<serde_json::Value>> {
    let rc = cori_engine::config::ResolvedConnector { name: name.into(), kind: "mcp".into(), base_url: None, auth_env: None, task_queue: None, url: None, command: Some(command.to_vec()), worker: None, secret: None, audience: None, mock: None, egress: cori_engine::egress::Egress::Any, oauth: None, app_account: false };
    match cori_engine::mcp::list_tools(&rc, &std::env::current_dir().unwrap_or_default(), None, cori_engine::egress::Egress::Any).await {
        Ok(t) => Some(t),
        Err(e) => { eprintln!("{name}: `{}` did not list its tools here ({e}); declare it where the command runs, or refresh it there later (`cori connectors refresh {name}`)", command.join(" ")); None }
    }
}
