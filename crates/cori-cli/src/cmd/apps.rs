// SPDX-License-Identifier: Apache-2.0
//! `cori apps …`: the apps on a server (list, get, create, sync-client, protect, delete).
use crate::cli::*;
use crate::client::Api;
use crate::commands;

pub async fn run(cmd: Cmd) -> anyhow::Result<()> {
    match cmd {
        Cmd::Apps { what } => match what {
            AppsCmd::List { json, conn: Conn { server, token } } => { let v = commands::apps_list(&server, &token).await?; if json { println!("{}", serde_json::to_string_pretty(&v)?); } else { let apps = v.as_array().cloned().unwrap_or_default(); println!("{:<24} {:<20} {:<23} {}", "APP", "OWNER", "PROMOTED", "WORKFLOWS"); if apps.is_empty() { println!("no apps on {} (create one: cori apps create <id>)", server.trim_end_matches('/')); } for a in apps { println!("{:<24} owner {:<14} promoted {:<14} {} workflow(s){}", a["id"].as_str().unwrap_or(""), a["owner"].as_str().unwrap_or("-"), a["promoted"].as_str().map(|h| h.trim_start_matches("sha256:").get(..12).unwrap_or("").to_string()).unwrap_or_else(|| "-".into()), a["workflows"], if a["protected"].as_bool() == Some(true) { " · protected" } else { "" }); } } }
            AppsCmd::SyncClient { app, conn: Conn { server, token } } => { let v = Api::new(&server, &token).post(&format!("/v1/apps/{app}/oidc-client"), &serde_json::json!({})).await?; println!("app {app} · client {} · redirect {}", v["client_id"].as_str().unwrap_or("?"), v["redirect_uri"].as_str().unwrap_or("?")); }
            AppsCmd::Protect { app, off, conn: Conn { server, token } } => { let v = Api::new(&server, &token).post(&format!("/v1/apps/{app}/protect"), &serde_json::json!({ "protected": !off })).await?; println!("app {app} · {}", if v["protected"].as_bool() == Some(true) { "protected: every promotion needs an approver" } else { "not protected: members promote; only a review that adds writes needs an approver" }); }
            AppsCmd::Delete { app, confirm, conn: Conn { server, token } } => {
                let confirm = match confirm { Some(c) => c, None => {
                    use std::io::IsTerminal;
                    if !std::io::stdin().is_terminal() { anyhow::bail!("deleting an app cannot be undone: pass `--confirm {app}`"); }
                    eprint!("This deletes `{app}` for good: every version, its live and sandbox store databases, schedules, runs and approvals.\nType the app id to confirm: ");
                    let mut s = String::new(); std::io::stdin().read_line(&mut s)?; s.trim().to_string()
                } };
                if confirm != app { anyhow::bail!("confirmation `{confirm}` does not match `{app}`: nothing deleted"); }
                let api = Api::new(&server, &token);
                let v = api.send(api.request(reqwest::Method::DELETE, &format!("/v1/apps/{app}")).query(&[("confirm", &confirm)])).await?;
                if v["status"] == "pending_approval" { println!("app {app} · deletion requested: waits for an approver ({}) · {}", v["approver_roles"].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(", ")).unwrap_or_default(), v["next"].as_str().unwrap_or("")); }
                else { let dropped = v["stores_dropped"].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(", ")).unwrap_or_default(); println!("app {app} deleted · {} version(s) · store databases dropped: {} · {} run(s) terminated", v["versions_deleted"], if dropped.is_empty() { "none".into() } else { dropped }, v["terminated_runs"].as_array().map(|a| a.len()).unwrap_or(0)); }
            }
            AppsCmd::Get { app, conn: Conn { server, token } } => { println!("{}", serde_json::to_string_pretty(&commands::apps_get(&server, &token, &app).await?)?); }
            AppsCmd::Create { app, name, roles, builders, conn: Conn { server, token } } => { let v = commands::apps_create(&server, &token, &app, name.as_deref(), &roles, &builders, None).await?; if v["existed"].as_bool() == Some(true) { println!("app {app} already exists ({})", v["detail"].as_str().unwrap_or("")); } else { println!("app {app} created · owner {} · next: `cori init {app} --app {app}` (or `cori pull {app}`)", v["owner"].as_str().unwrap_or("")); } }
        },
        _ => unreachable!("main dispatches only this module's commands"),
    }
    Ok(())
}
