// SPDX-License-Identifier: Apache-2.0
//! `cori store …`: Cori Store from the CLI (new, list, tools, query).
use crate::cli::*;
use crate::client::Api;
use crate::{commands, session};

pub async fn run(cmd: Cmd) -> anyhow::Result<()> {
    match cmd {
        Cmd::Store { what } => {
            match what {
                StoreCmd::New { name, roles, tenant, dir } => { println!("{}", commands::store_new(&dir, &name, &roles, tenant.as_deref())?); }
                StoreCmd::List { app, conn: Conn { server, token } } => {
                    let path = match app.or_else(|| commands::app_id_of(std::path::Path::new("."))) { Some(a) => format!("/v1/apps/{a}/stores"), None => "/v1/stores".to_string() };
                    let v = Api::new(&server, &token).get(&path).await?;
                    for s in v.as_array().cloned().unwrap_or_default() { let t = s["target"].as_str().unwrap_or(""); println!("{:<16} {:<22} {:<14} {:<48} roles {:?}{}", s["app"].as_str().unwrap_or(""), s["name"].as_str().unwrap_or(""), if t == "live" || t == "local" { t.to_string() } else { format!("@{}", t.trim_start_matches("sha256:").get(..12).unwrap_or(t)) }, s["database"].as_str().unwrap_or(""), s["roles"], if s["seeded"].as_bool() == Some(true) { "  seeded" } else { "" }); }
                    if v.as_array().map(|a| a.is_empty()).unwrap_or(true) { println!("no store database yet: `cori deploy .` creates a sandbox per store, `cori promote` the live ones"); }
                }
                StoreCmd::Tools { store, role, dir, online, app, version, conn: Conn { server, token } } => { let offline_root = if online || version.is_some() || app.is_some() { None } else { session::app_root_of(&dir).filter(|r| r.join("stores").is_dir()) }; if let Some(root) = offline_root { println!("{}", cori_compiler::stores::tools_text(&root, &store, role.as_deref()).map_err(|e| anyhow::anyhow!(e))?); return Ok(()); } let r = Api::new(&server, &token).rpc(&commands::store_path(app, version, &store)?, "tools/list", serde_json::json!({})).await?; for t in r["tools"].as_array().cloned().unwrap_or_default() { println!("{:<26} {}", t["name"].as_str().unwrap_or(""), t["description"].as_str().unwrap_or("")); } }
                StoreCmd::RotateLogins { app, conn: Conn { server, token } } => {
                    let v = Api::new(&server, &token).post("/v1/store-logins/rotate", &serde_json::json!({ "app": app })).await?;
                    let rotated = v["rotated"].as_array().cloned().unwrap_or_default();
                    for r in &rotated { println!("{:<24} {:<20} {:<14} {}", r["app"].as_str().unwrap_or(""), r["store"].as_str().unwrap_or(""), r["target"].as_str().unwrap_or("").trim_start_matches("sha256:").chars().take(12).collect::<String>(), r["database"].as_str().unwrap_or("")); }
                    println!("{} store database(s): new login passwords", rotated.len());
                }
                StoreCmd::Query { store, sql, app, version, conn: Conn { server, token } } => { let r = Api::new(&server, &token).rpc(&commands::store_path(app, version, &store)?, "tools/call", serde_json::json!({ "name": "query", "arguments": { "sql": sql } })).await?; if r["isError"].as_bool().unwrap_or(false) { anyhow::bail!("{}", r["content"][0]["text"].as_str().unwrap_or("")); } for row in r["structuredContent"]["rows"].as_array().cloned().unwrap_or_default() { println!("{row}"); } }
            }
        }
        _ => unreachable!("main dispatches only this module's commands"),
    }
    Ok(())
}
