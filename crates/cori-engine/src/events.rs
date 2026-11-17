// SPDX-License-Identifier: Apache-2.0
//! Best-effort worker → server events (step outcomes, approval requests). Local runs have no server and skip.
use serde_json::{Value, json};

pub fn emit(server: Option<&str>, realm: Option<&str>, run_id: &str, kind: &str, payload: Value) {
    let Some(server) = server else { return };
    let url = format!("{}/v1/ingest/events", server.trim_end_matches('/'));
    let body = json!({ "run_id": run_id, "kind": kind, "at": chrono::Utc::now().to_rfc3339(), "payload": payload });
    let req = worker_auth(crate::http::client().post(url), realm);
    tokio::spawn(async move { let _ = req.json(&body).timeout(std::time::Duration::from_secs(5)).send().await; });
}

/// `emit`, awaited and retried (three attempts within a few seconds): for what must reach the server, such as an LLM call's
/// capture, which reports what the call spent.
pub async fn emit_now(server: Option<&str>, realm: Option<&str>, run_id: &str, kind: &str, payload: Value) {
    let Some(server) = server else { return };
    let url = format!("{}/v1/ingest/events", server.trim_end_matches('/'));
    let body = json!({ "run_id": run_id, "kind": kind, "at": chrono::Utc::now().to_rfc3339(), "payload": payload });
    for attempt in 0..3u32 {
        match worker_auth(crate::http::client().post(&url), realm).json(&body).timeout(std::time::Duration::from_secs(5)).send().await {
            Ok(r) if r.status().is_success() => return,
            // refused: another try is refused too
            Ok(r) if r.status().is_client_error() => { tracing::warn!(run = run_id, kind, status = %r.status(), "event refused by the server"); return; }
            _ => tokio::time::sleep(std::time::Duration::from_millis(500 << attempt)).await,
        }
    }
    tracing::warn!(run = run_id, kind, "event not delivered: the server did not answer");
}

/// The worker's credentials on a call to the server: a pool's name and key (`CORI_WORKER_POOL` + `CORI_POOL_KEY`, set
/// by `cori work --pool`), else the cluster's shared token (`CORI_WORKER_TOKEN`); and the realm of the run the call is for
/// (`BundleRef::realm`), on a server keeping organizations apart.
pub fn worker_auth(req: reqwest::RequestBuilder, realm: Option<&str>) -> reqwest::RequestBuilder {
    let var = |n: &str| std::env::var(n).ok().filter(|v| !v.is_empty());
    let req = match realm { Some(r) => req.header("x-cori-realm", r), None => req };
    match (var("CORI_WORKER_POOL"), var("CORI_POOL_KEY")) {
        (Some(pool), Some(key)) => req.header("x-cori-pool", pool).header("x-cori-pool-key", key),
        _ => match var("CORI_WORKER_TOKEN") { Some(t) => req.header("x-cori-worker", t), None => req },
    }
}

/// Removes Cori's own variables (worker token, pool key, and under `cori up` the server's master key and database URLs)
/// from the environment of a process a step starts: `cli` binaries and stdio MCP servers get the machine's environment,
/// not the worker's credentials.
pub fn scrub_env(cmd: &mut tokio::process::Command) -> &mut tokio::process::Command {
    for k in cori_compiler::runtime::cori_env_vars() { cmd.env_remove(k); }
    cmd
}
