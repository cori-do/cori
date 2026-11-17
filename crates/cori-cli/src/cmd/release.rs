// SPDX-License-Identifier: Apache-2.0
//! Releases of an app: `cori deploy`, `releases`, `promote`, `rollback`, `undeploy`, `reseed`.
use crate::cli::*;
use crate::client::Api;
use crate::commands;
use std::path::PathBuf;

pub async fn run(cmd: Cmd) -> anyhow::Result<()> {
    match cmd {
        Cmd::Releases { app, json, conn: Conn { server, token } } => {
            let api = Api::new(&server, &token);
            let v = api.get(&format!("/v1/apps/{app}/versions")).await?;
            if json { println!("{}", serde_json::to_string_pretty(&v)?); return Ok(()); }
            let all = v.as_array().cloned().unwrap_or_default();
            if all.is_empty() { println!("`{app}` has no release yet: `cori deploy` in its folder"); return Ok(()); }
            let strs = |x: &serde_json::Value| x.as_array().map(|a| a.iter().filter_map(|s| s.as_str()).collect::<Vec<_>>().join(", ")).unwrap_or_default();
            for (section, title) in [("live", "live"), ("newer", "newer than live"), ("older", "older")] {
                let rows: Vec<&serde_json::Value> = all.iter().filter(|x| x["section"] == section).collect();
                if rows.is_empty() { continue; }
                println!("{title}");
                for x in rows {
                    let g = &x["gate"];
                    let gate = match g["state"].as_str().unwrap_or("") {
                        "live" => "live".to_string(),
                        "ready" => format!("ready{}", if x["was_live"].as_bool() == Some(true) { " (was live before)" } else { "" }),
                        "approved" => format!("approved by {}", g["by"].as_str().unwrap_or("")),
                        "requested" => format!("requested by {}, waits for an approver", g["by"].as_str().unwrap_or("")),
                        _ => format!("needs approval: {}", g["reason"].as_str().unwrap_or("")),
                    };
                    let msg = x["message"].as_str().map(|m| m.chars().take(48).collect::<String>()).unwrap_or_default();
                    println!("  {:<5} {:<44} {:<10} {}  {}", x["release"].as_str().unwrap_or(""), gate, x["created_by"].as_str().unwrap_or(""), x["created_at"].as_str().unwrap_or("").get(..16).unwrap_or("").replace('T', " "), msg);
                    let mut notes = vec![format!("version {}", x["short"].as_str().unwrap_or(""))];
                    if let Some(c) = x["git_commit"].as_str() { notes.push(format!("{}@{}{}", x["git_branch"].as_str().unwrap_or("?"), c.get(..7).unwrap_or(c), if x["git_dirty"].as_bool() == Some(true) { " (dirty)" } else { "" })); }
                    let pm = strs(&x["pending_migrations"]); if !pm.is_empty() { notes.push(format!("runs on live data: {pm}")); }
                    let ahead = strs(&x["schema_ahead"]); if !ahead.is_empty() && section != "newer" { notes.push(format!("live data is ahead: {ahead}")); }
                    if section != "live" { notes.push(format!("preview {}", x["preview"]["url"].as_str().unwrap_or(""))); }
                    println!("        {}", notes.join(" · "));
                }
            }
            let label = |h: &str| all.iter().find(|x| x["hash"] == h).and_then(|x| x["release"].as_str().map(String::from)).unwrap_or_else(|| format!("{} (deleted)", h.trim_start_matches("sha256:").get(..12).unwrap_or(h)));
            let d = api.get(&format!("/v1/apps/{app}/promotions")).await?;
            let hist = d.as_array().cloned().unwrap_or_default();
            if !hist.is_empty() { println!("went live"); }
            for x in hist.iter().take(5) { let migs = x["migrations"].as_array().map(|a| a.len()).unwrap_or(0); println!("  {}  {:<5} by {}{}{}", x["at"].as_str().unwrap_or("").get(..16).unwrap_or("").replace('T', " "), label(x["hash"].as_str().unwrap_or("")), x["by"].as_str().unwrap_or(""), if x["kind"] == "rollback" { " · back (rollback)" } else { "" }, if migs > 0 { format!(" · {migs} migration(s) on the live stores") } else { String::new() }); }
        }
        Cmd::Rollback { app, conn: Conn { server, token } } => {
            let v = Api::new(&server, &token).post(&format!("/v1/apps/{app}/rollback"), &serde_json::json!({})).await?;
            println!("app {app} · rolled back to {} (from {}) · the live stores keep their schema", v["release"].as_str().unwrap_or(""), v["previous_release"].as_str().unwrap_or("-"));
            let ahead: Vec<&str> = v["schema_ahead"].as_array().map(|a| a.iter().filter_map(|m| m.as_str()).collect()).unwrap_or_default();
            if !ahead.is_empty() { println!("  warning: the live stores ran migrations {} does not have ({})", v["release"].as_str().unwrap_or(""), ahead.join(", ")); }
        }
        Cmd::Deploy { dir, require_clean, message, json, conn: Conn { server, token } } => {
            let dir = dir.unwrap_or_else(|| PathBuf::from("."));
            if let Some(w) = commands::front_build_warning(&dir) { eprintln!("warning: {w}"); }
            let mut body = commands::publish_bundle(&dir, &server, &token, require_clean, message.as_deref()).await?;
            let app = body["app_id"].as_str().unwrap_or("").to_string();
            body["console_url"] = serde_json::json!(format!("{}/#/versions/{app}", server.trim_end_matches('/')));
            if json { println!("{}", serde_json::to_string_pretty(&body)?); return Ok(()); }
            let short = body["short"].as_str().unwrap_or("?").to_string();
            let status = body["status"].as_str().unwrap_or("?");
            let release = body["release"].as_str().map(String::from).unwrap_or_else(|| short.clone());
            let what = match (status, body["redeployed"].as_bool() == Some(true)) { ("promoted", true) => "is live (the same content was deployed again)".to_string(), (s, true) => format!("{s} again (same content as before)"), (s, false) => s.to_string() };
            println!("app {app} · {release} {what} · version {short}{}", body["git"]["commit"].as_str().map(|c| format!(" · commit {}{}", c.get(..12).unwrap_or(c), if body["git"]["dirty"].as_bool() == Some(true) { " (dirty)" } else { "" })).unwrap_or_default());
            // the sandbox: one database per store, migrated from scratch and seeded; nothing touched the live databases
            let sandbox = body["sandbox"].as_array().cloned().unwrap_or_default();
            if let Some(w) = sandbox.iter().find_map(|s| s["warning"].as_str()) { println!("  sandbox  {w}"); }
            for w in body["warnings"].as_array().cloned().unwrap_or_default() { println!("  warning  {}", w.as_str().unwrap_or_default()); }
            for s in sandbox.iter().filter(|s| s["store"].is_string()) {
                let migs = s["migrations_applied"].as_array().map(|a| a.len()).unwrap_or(0); let seeds = s["seeds_applied"].as_array().map(|a| a.len()).unwrap_or(0);
                println!("  sandbox  {:<20} {}{}", s["store"].as_str().unwrap_or(""), if s["created"].as_bool() == Some(true) { format!("created · {migs} migration(s) · {seeds} seed file(s)") } else { "already there (same version)".into() }, if s["seeds"].as_u64() == Some(0) { " · no seed/*.sql: empty tables" } else { "" });
            }
            if let Some(p) = body["preview_url"].as_str() { println!("  preview  {p}  (its workflows run on the sandbox; nothing reaches the live stores)"); }
            println!("  console  {}  (versions, change review, approve, promote)", body["console_url"].as_str().unwrap_or(""));
            for w in body["workflows"].as_array().cloned().unwrap_or_default() { println!("  workflow {:<26} endpoint {}  schedule {}", w["id"].as_str().unwrap_or(""), w["endpoint"].get("mode").and_then(|m| m.as_str()).unwrap_or("-"), w["schedule"].as_str().unwrap_or("-")); }
            let d = &body["review"]["delta"];
            let adds_writes = body["review"]["needs_approval"].as_bool().unwrap_or(false);
            let protected = body["protected"].as_bool().unwrap_or(false);
            let approvers = body["approver_roles"].as_array().map(|a| a.iter().filter_map(|r| r.as_str()).collect::<Vec<_>>().join(", ")).filter(|a| !a.is_empty()).map(|a| format!(" ({a})")).unwrap_or_default();
            print!("  change review: new/changed writes {} · added {} · removed {}", d["new_or_changed_writes"].as_array().map(|a| a.len()).unwrap_or(0), d["added_workflows"].as_array().map(|a| a.len()).unwrap_or(0), d["removed_workflows"].as_array().map(|a| a.len()).unwrap_or(0));
            match (adds_writes, protected, body["approved_by"].as_str()) {
                (false, false, _) => println!(" · no new writes, no approval needed"),
                (_, _, Some(by)) => println!(" · approved by {by}"),
                (true, _, None) => println!(" · new writes: an approver{approvers} approves before it is promoted"),
                (false, true, None) => println!(" · protected app: an approver{approvers} approves before it is promoted"),
            }
            for w in d["new_or_changed_writes"].as_array().cloned().unwrap_or_default().iter().take(12) { println!("    write  {}", w.as_str().map(String::from).unwrap_or_else(|| w.to_string())); }
            if status != "promoted" {
                println!("  next     {}", body["promote"].as_str().unwrap_or(""));
                println!("           (a human; going live applies this release's pending migrations to the live stores; preview data stays behind)");
                if body["needs_approval"].as_bool().unwrap_or(false) && body["approved_by"].is_null() { println!("           without an approver role the request waits in the console inbox; an approver accepts it there, or runs `cori promote {app} {release}`"); }
            }
        }
        Cmd::Promote { app, hash, conn: Conn { server, token } } => {
            let v = Api::new(&server, &token).post(&format!("/v1/apps/{app}/versions/{hash}/promote"), &serde_json::json!({})).await?;
            let short = v["release"].as_str().or(v["short"].as_str()).unwrap_or("");
            match v["status"].as_str() {
                Some("promoted") => {
                    let migs = v["migrations"].as_array().cloned().unwrap_or_default();
                    println!("app {app} · {short} promoted{} · live at {}{}", if v["went_back"].as_bool() == Some(true) { " (back to a release that was live before: no approval, no migration)" } else { "" }, v["url"].as_str().unwrap_or(""), v["previous_release"].as_str().map(|p| format!(" (was {p})")).unwrap_or_default());
                    let ahead: Vec<&str> = v["schema_ahead"].as_array().map(|a| a.iter().filter_map(|m| m.as_str()).collect()).unwrap_or_default();
                    if !ahead.is_empty() { println!("  warning: the live stores ran migrations {short} does not have ({}): its code runs against that newer schema", ahead.join(", ")); }
                    let skipped: Vec<String> = v["skipped"].as_array().cloned().unwrap_or_default().iter().filter_map(|h| h.as_str().map(|h| h.trim_start_matches("sha256:").get(..12).unwrap_or(h).to_string())).collect();
                    if !skipped.is_empty() { println!("  passed over: {} (older, never live; they can still go live)", skipped.join(", ")); }
                    println!("  live stores: {}", if migs.is_empty() { "no pending migration".to_string() } else { format!("{} migration(s) applied: {}", migs.len(), migs.iter().filter_map(|m| m.as_str()).collect::<Vec<_>>().join(", ")) });
                }
                _ => println!("app {app} · {short} waits for an approver ({}): {}\n  next: {}", v["reason"].as_str().unwrap_or(""), v["approval_id"].as_str().unwrap_or(""), v["next"].as_str().unwrap_or("")),
            }
        }
        Cmd::Undeploy { app, hash, force, conn: Conn { server, token } } => {
            let v = Api::new(&server, &token).delete(&format!("/v1/apps/{app}/versions/{hash}{}", if force { "?force=true" } else { "" })).await?;
            let dropped = v["sandbox_dropped"].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(", ")).unwrap_or_default();
            let terminated = v["terminated_runs"].as_array().map(|a| a.len()).unwrap_or(0);
            println!("app {app} · {} deleted · preview data {}{}", v["short"].as_str().unwrap_or(""), if dropped.is_empty() { "-".to_string() } else { format!("{dropped} dropped") }, if terminated > 0 { format!(" · {terminated} run(s) terminated") } else { String::new() });
        }
        Cmd::Reseed { app, hash, conn: Conn { server, token } } => {
            let v = Api::new(&server, &token).post(&format!("/v1/apps/{app}/versions/{hash}/reseed"), &serde_json::json!({})).await?;
            let short = v["version"].as_str().map(|h| h.trim_start_matches("sha256:").get(..12).unwrap_or(h).to_string()).unwrap_or_default();
            for s in v["sandbox"].as_array().cloned().unwrap_or_default() { println!("app {app} · version {short} · sandbox {:<20} recreated · {} migration(s) · {} seed file(s)", s["store"].as_str().unwrap_or(""), s["migrations_applied"].as_array().map(|a| a.len()).unwrap_or(0), s["seeds_applied"].as_array().map(|a| a.len()).unwrap_or(0)); }
            if v["sandbox"].as_array().map(|a| a.is_empty()).unwrap_or(true) { println!("app {app} · version {short}: no store in this version"); }
        }
        _ => unreachable!("main dispatches only this module's commands"),
    }
    Ok(())
}
