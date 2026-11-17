// SPDX-License-Identifier: Apache-2.0
//! Administering the org: `cori secrets`, `cori pools`.
use crate::cli::*;
use crate::client::Api;

pub async fn run(cmd: Cmd) -> anyhow::Result<()> {
    match cmd {
        Cmd::Secrets { what } => match what {
            SecretsCmd::Set { name, from_env, kind, app, conn: Conn { server, token } } => {
                let value = match from_env { Some(v) => std::env::var(&v).map_err(|_| anyhow::anyhow!("env var {v} is not set"))?, None => { eprint!("value for {name} (hidden): "); let mut s = String::new(); std::io::stdin().read_line(&mut s)?; s.trim_end_matches(['\n', '\r']).to_string() } };
                let v = Api::new(&server, &token).post("/v1/secrets", &serde_json::json!({ "name": name, "kind": kind, "value": value, "app": app })).await?;
                match v["app"].as_str() {
                    Some(a) => println!("secret {} of app {a} stored (version {})", v["name"].as_str().unwrap_or(""), v["version"]),
                    None => println!("secret {} stored (version {})", v["name"].as_str().unwrap_or(""), v["version"]),
                }
            }
            SecretsCmd::List { app, conn: Conn { server, token } } => {
                let path = match &app { Some(a) => format!("/v1/secrets?app={a}"), None => "/v1/secrets".to_string() };
                let v = Api::new(&server, &token).get(&path).await?;
                for x in v.as_array().cloned().unwrap_or_default() { println!("{:<20} {:<36} {:<11} v{:<3} {:<34} {}", x["app"].as_str().unwrap_or("(deployment)"), x["name"].as_str().unwrap_or(""), x["kind"].as_str().unwrap_or(""), x["version"], x["key_by"].as_str().unwrap_or(""), x["updated_at"].as_str().unwrap_or("")); }
            }
        },
        Cmd::Pools { what } => match what {
            PoolsCmd::Create { name, conn: Conn { server, token } } => {
                let v = Api::new(&server, &token).post("/v1/pools", &serde_json::json!({ "name": name })).await?;
                println!("pool {} created · queue {}\nkey (shown once): {}\n{}", v["name"].as_str().unwrap_or(""), v["task_queue"].as_str().unwrap_or(""), v["key"].as_str().unwrap_or(""), v["hint"].as_str().unwrap_or(""));
            }
            PoolsCmd::List { conn: Conn { server, token } } => {
                let v = Api::new(&server, &token).get("/v1/pools").await?;
                for p in v.as_array().cloned().unwrap_or_default() { println!("{:<20} last seen {}  caps {}", p["name"].as_str().unwrap_or(""), p["last_seen"].as_str().unwrap_or("never"), p["capabilities"]); }
            }
        },
        _ => unreachable!("main dispatches only this module's commands"),
    }
    Ok(())
}
