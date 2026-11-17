// SPDX-License-Identifier: Apache-2.0
//! `cori llm …`: model bindings and captured calls (bind, calls). The evaluation loop (eval, dataset, finetune, and the
//! `shadow` binding mode) is the LLM Ops module's, on the server: these commands call its routes, and a server without the
//! module says so.
use crate::cli::*;
use crate::client::{Api, parse};
use reqwest::{Method, RequestBuilder, StatusCode};
use serde_json::{Value, json};

/// Why an LLM Ops command cannot run against `server`.
fn no_llm_ops(server: &str) -> anyhow::Error {
    anyhow::anyhow!("{server} has no LLM Ops module, which serves shadow bindings, `cori llm eval`, `dataset` and `finetune`")
}

/// The text of an answer from an LLM Ops route: a server without the module has no such route (a 404 without a body).
async fn llm_ops(api: &Api, server: &str, req: RequestBuilder) -> anyhow::Result<String> {
    let (status, text) = api.text_of(req).await?;
    if status == StatusCode::NOT_FOUND && text.trim().is_empty() { return Err(no_llm_ops(server)); }
    if !status.is_success() { return Err(api.error(status, &parse(&text))); }
    Ok(text)
}

pub async fn run(cmd: Cmd) -> anyhow::Result<()> {
    match cmd {
        Cmd::Llm { what } => match what {
            LlmCmd::Bind { app, workflow, step, model, provider, mode, tier, conn: Conn { server, token } } => {
                let api = Api::new(&server, &token);
                let (status, v) = api.call(api.request(Method::POST, &format!("/v1/apps/{app}/llm/bindings")).json(&json!({ "workflow": workflow, "step": step, "model": model, "provider": provider, "mode": mode, "tier": tier }))).await?;
                // a server that binds `primary` only refuses the evaluation modes (400, "this server binds primary")
                if status == StatusCode::BAD_REQUEST && mode != "primary" && v["error"].as_str().is_some_and(|e| e.contains("this server binds primary")) { return Err(no_llm_ops(&server)); }
                if !status.is_success() { return Err(api.error(status, &v)); }
                println!("{v}");
            }
            LlmCmd::Eval { app, conn: Conn { server, token } } => {
                let api = Api::new(&server, &token);
                let v: Value = serde_json::from_str(&llm_ops(&api, &server, api.request(Method::GET, &format!("/v1/apps/{app}/llm/eval"))).await?)?;
                for e in v.as_array().cloned().unwrap_or_default() { println!("{}.{}  primary {} ({} calls, valid {:.0}%)  shadow {} ({} calls, valid {:.0}%)  agreement {}  → {}", e["workflow"].as_str().unwrap_or(""), e["step"].as_str().unwrap_or(""), e["primary"]["model"].as_str().unwrap_or("-"), e["primary"]["calls"], e["primary"]["schema_valid_rate"].as_f64().unwrap_or(0.0) * 100.0, e["shadow"]["model"].as_str().unwrap_or("-"), e["shadow"]["calls"], e["shadow"]["schema_valid_rate"].as_f64().unwrap_or(0.0) * 100.0, e["agreement"], e["recommendation"].as_str().unwrap_or("")); if let Some(f) = e["agreement_by_field"].as_object() { for (k, v) in f { println!("    {:<20} {:.0}%", k, v.as_f64().unwrap_or(0.0) * 100.0); } } }
            }
            LlmCmd::Calls { app, conn: Conn { server, token } } => { let v = Api::new(&server, &token).get(&format!("/v1/apps/{app}/llm/calls")).await?; for x in v.as_array().cloned().unwrap_or_default().iter().take(30) { println!("{} {:<22} {:<16} {:<8} {:<7} valid={} {}ms {}", x["at"].as_str().unwrap_or("").get(..19).unwrap_or(""), x["workflow"].as_str().unwrap_or(""), x["step"].as_str().unwrap_or(""), x["mode"].as_str().unwrap_or(""), x["tier"].as_str().unwrap_or(""), x["schema_valid"], x["latency_ms"], x["model"].as_str().unwrap_or("")); } }
            LlmCmd::Dataset { app, out, conn: Conn { server, token } } => {
                let api = Api::new(&server, &token);
                let t = llm_ops(&api, &server, api.request(Method::GET, &format!("/v1/apps/{app}/llm/dataset.jsonl"))).await?;
                std::fs::write(&out, &t)?;
                println!("wrote {} ({} examples)", out.display(), t.lines().count());
            }
            LlmCmd::Finetune { app, workflow, step, provider, base_model, out, conn: Conn { server, token } } => {
                // the server builds the training file from the step's captured calls and, but for `mock`, submits the job with
                // the org's provider and its key (console, LLM providers)
                let api = Api::new(&server, &token);
                let body = json!({ "workflow": workflow, "step": step, "provider": provider, "base_model": base_model });
                let v: Value = serde_json::from_str(&llm_ops(&api, &server, api.request(Method::POST, &format!("/v1/apps/{app}/llm/finetune")).json(&body)).await?)?;
                if let Some(f) = v["training_file"].as_str() { std::fs::write(&out, f)?; println!("wrote {} ({} examples)", out.display(), v["examples"]); }
                if let Some(job) = v["job"].as_str() { println!("fine-tuning job {job} on {base_model} ({provider})"); }
                if let Some(next) = v["next"].as_str() { println!("{next}"); }
            }
        },
        _ => unreachable!("main dispatches only this module's commands"),
    }
    Ok(())
}
