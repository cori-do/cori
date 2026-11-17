// SPDX-License-Identifier: Apache-2.0
//! A run's steps, for its inspection: what each step is, as the run's release compiled it (its author's description, what
//! it works with, the data it uses: never its code), and what it did in the run (what it received and gave back at each
//! attempt, the decision a person made, the model it asked). Whoever may see the run sees them; its builders and operators
//! also get where each step is written, a query's text and the instructions a model was given.
use super::*;

/// `GET /v1/apps/{app}/runs/{run}/steps`: the run's workflow as its release defines it, step by step, and each step's data
/// in the run. A console route: fronts read `runs/{run}` and its `events`.
pub(crate) async fn run_steps(InRealm(st): InRealm, Path((app, run)): Path<(String, String)>, front: Option<axum::Extension<FrontHost>>, Caller(who): Caller) -> Res {
    run_in_scope(&st, &app, &run, front.as_ref().map(|f| &f.0)).await?;
    let r = sqlx::query("SELECT r.workflow, r.user_sub, v.workflows, v.seq FROM runs r LEFT JOIN versions v ON v.hash = r.version_hash AND v.app = r.app WHERE r.id = $1 AND r.app = $2")
        .bind(&run).bind(&app).fetch_optional(&st.db).await?.ok_or(ApiError(StatusCode::NOT_FOUND, "unknown run".into()))?;
    let builds = may_see_all_runs(&st, &who, &app).await?;
    if r.get::<String, _>("user_sub") != who.sub && !builds { return Err(ApiError(StatusCode::FORBIDDEN, "not your run".into())); }
    let wf: String = r.get("workflow");
    // its release's compiled workflow; none when the release was deleted since
    let entry = r.get::<Option<String>, _>("workflows").and_then(|w| serde_json::from_str::<Vec<bundles::WorkflowEntry>>(&w).ok()).and_then(|ws| ws.into_iter().find(|w| w.id == wf));
    let mut out = json!({
        "workflow": { "id": wf, "release": r.get::<Option<i64>, _>("seq").map(|n| format!("v{n}")) },
        "steps": entry.as_ref().map(|e| e.compiled.dag.nodes.iter().map(|n| step_plan(n, builds)).collect::<Vec<_>>()).unwrap_or_default(),
    });
    if let Some(m) = entry.as_ref().map(|e| &e.compiled.manifest) {
        out["workflow"]["name"] = json!(m.name);
        if !m.description.is_empty() { out["workflow"]["description"] = json!(m.description); }
    }

    // what the steps did: each attempt of a step the worker ran (its event), the child runs it started, the model calls
    let events = sqlx::query("SELECT kind, at, payload FROM run_events WHERE run_id = $1 AND kind IN ('step', 'child_started', 'llm_call') ORDER BY at").bind(&run).fetch_all(&st.db).await?;
    let mut attempts: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    let mut children = Vec::new();
    let mut prompts: BTreeMap<String, Value> = BTreeMap::new();
    for e in &events {
        let (kind, at): (String, String) = (e.get("kind"), e.get("at"));
        let p: Value = serde_json::from_str(&e.get::<String, _>("payload")).unwrap_or(Value::Null);
        let Some(step) = p.get(if kind == "step" { "name" } else { "step" }).and_then(|s| s.as_str()).map(String::from) else { continue };
        match kind.as_str() {
            "step" => {
                let mut a = json!({ "at": at });
                for k in ["attempt", "status", "ms", "input", "output", "error", "too_large"] { if let Some(x) = p.get(k).filter(|x| !x.is_null()) { a[k] = x.clone(); } }
                attempts.entry(step).or_default().push(a);
            }
            // what the step gave the child: its parameters
            "child_started" => children.push(json!({ "step": step, "run_id": p["child_run_id"], "workflow": p["workflow"], "at": at, "input": p["params"] })),
            _ => { if let Some(t) = p.get("prompt").filter(|t| !t.is_null()) { prompts.insert(at, t.clone()); } }
        }
    }
    out["attempts"] = json!(attempts);
    out["children"] = json!(children);

    // the people a step asked: who, what they were shown, who decided and when
    let asked = sqlx::query("SELECT step, to_role, status, payload, created_at, decided_by, decided_at FROM approvals WHERE run_id = $1 AND app = $2 ORDER BY created_at").bind(&run).bind(&app).fetch_all(&st.db).await?;
    out["decisions"] = json!(asked.iter().map(|a| {
        let p: Value = serde_json::from_str(&a.get::<String, _>("payload")).unwrap_or(Value::Null);
        json!({ "step": a.get::<String, _>("step"), "to": a.get::<String, _>("to_role"), "status": a.get::<String, _>("status"), "asked_at": a.get::<String, _>("created_at"),
                "decided_by": a.get::<Option<String>, _>("decided_by"), "decided_at": a.get::<Option<String>, _>("decided_at"), "shown": p.get("payload").cloned().unwrap_or(Value::Null) })
    }).collect::<Vec<_>>());

    // the model calls, as the server priced them: for the app's builders and operators (costs, models, prompts)
    if builds {
        let calls = sqlx::query("SELECT step, tier, model, provider, mode, schema_valid, latency_ms, tokens_in, tokens_out, cost_eur, at FROM llm_calls WHERE run_id = $1 ORDER BY at").bind(&run).fetch_all(&st.db).await?;
        out["model_calls"] = json!(calls.iter().map(|c| {
            let at: String = c.get("at");
            json!({ "step": c.get::<String, _>("step"), "tier": c.get::<String, _>("tier"), "model": c.get::<String, _>("model"), "provider": c.get::<Option<String>, _>("provider"), "mode": c.get::<String, _>("mode"),
                    "valid": c.get::<i32, _>("schema_valid") != 0, "ms": c.get::<i64, _>("latency_ms"), "tokens_in": c.get::<i64, _>("tokens_in"), "tokens_out": c.get::<i64, _>("tokens_out"),
                    "cost_eur": c.get::<f64, _>("cost_eur"), "prompt": prompts.get(&at), "at": at })
        }).collect::<Vec<_>>());
    }
    Ok(Json(out).into_response())
}

/// What a step is, as a person reads it: its author's description, what it works with (its literals, never its code: a
/// value its code computes is left out) and the data it uses (its bindings' references); for builders, where it is
/// written and a query's text.
pub(crate) fn step_plan(n: &cori_compiler::dag::Node, builds: bool) -> Value {
    let lit = |k: &str| n.literals.get(k).and_then(|v| v.as_str()).filter(|s| *s != "<expr>").map(String::from);
    let mut v = json!({ "name": n.name, "kind": n.kind, "access": n.effect.access, "external": n.effect.external });
    if !n.description.is_empty() { v["description"] = json!(n.description); }
    if let Some(p) = &n.nested_in { v["nested_in"] = json!(p); v["slot"] = json!(n.slot); }
    if !n.refs.is_empty() { v["uses"] = json!(n.refs); }
    for (k, as_) in [("connector", "connector"), ("tool", "tool"), ("method", "method"), ("id", "workflow"), ("activity", "activity"), ("argv0", "program")] {
        if let Some(s) = lit(k) { v[as_] = json!(s); }
    }
    // a value the author wrote out, or what it falls back to: `{ default: … }` reads `default`
    let choice = |k: &str| n.literals.get(k).and_then(|x| if x.get("default").is_some() { Some("default".to_string()) } else { x.as_str().filter(|s| *s != "<expr>").map(String::from) });
    match n.kind.as_str() {
        "llm" => { v["tier"] = json!(lit("tier").unwrap_or_else(|| "medium".into())); if let Some(f) = choice("fallback") { v["fallback"] = json!(f); } }
        "store" if n.literals.contains_key("query") => {
            v["query"] = json!(true);
            if builds { if let Some(t) = n.literals["query"].get("text").and_then(|t| t.as_str()) { v["query_text"] = json!(t); } }
        }
        "human" => {
            if let Some(k) = lit("kind") { v["ask"] = json!(k); }
            if let Some(to) = n.literals.get("to").filter(|t| t.is_object()) { v["to"] = to.clone(); }
            if let Some(t) = lit("timeout") { v["timeout"] = json!(t); }
            if let Some(o) = choice("on_timeout") { v["on_timeout"] = json!(o); }
        }
        "wait" => { if let Some(ms) = n.literals.get("for").and_then(|f| f.get("timeout_ms")).and_then(|m| m.as_u64()) { v["wait_ms"] = json!(ms); } }
        "loop" => { if let Some(m) = n.literals.get("max_iterations").and_then(|m| m.as_u64()) { v["max_iterations"] = json!(m); } }
        _ => {}
    }
    if !n.goto_targets.is_empty() { v["goto"] = json!(n.goto_targets.iter().map(|(slot, to)| json!({ "slot": slot, "to": to })).collect::<Vec<_>>()); }
    if builds { v["file"] = json!(n.file); v["line"] = json!(n.line); }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nodes(app: &str, wf: &str) -> Vec<cori_compiler::dag::Node> {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../templates").join(app).join("workflows").join(wf);
        cori_compiler::compile(&dir, &cori_compiler::Options { run_deno: false, sdk_path: None, connectors: vec![], deno: None }).unwrap_or_else(|e| panic!("{}", e.join("; "))).dag.nodes
    }

    /// The template's request flow, step by step, as a person reads it: no code, what each step works with, the data it
    /// uses; a builder also gets where each is written.
    #[test]
    fn a_step_reads_as_its_author_described_it() {
        let ns = nodes("team-requests", "submit_request");
        let plan = |name: &str, builds: bool| step_plan(ns.iter().find(|n| n.name == name).unwrap(), builds);
        let triage = plan("triage", false);
        assert_eq!(triage["description"], "Sort and summarise the request");
        assert_eq!((triage["kind"].as_str(), triage["tier"].as_str(), triage["fallback"].as_str(), triage["access"].as_str()), (Some("llm"), Some("small"), Some("default"), Some("prompt")));
        assert!(triage["uses"].as_array().unwrap().contains(&json!("params.title")));
        assert!(triage.get("file").is_none() && triage.get("prompt").is_none(), "{triage}");
        let record = plan("record", false);
        assert_eq!((record["connector"].as_str(), record["tool"].as_str(), record["access"].as_str()), (Some("requests"), Some("createRequest"), Some("may_write")));
        let ask = plan("approve_request", false);
        assert_eq!((ask["ask"].as_str(), ask["to"].clone(), ask["timeout"].as_str(), ask["on_timeout"].as_str()), (Some("approval"), json!({ "role": "manager" }), Some("7d"), Some("default")));
        assert_eq!((ask["nested_in"].as_str(), ask["slot"].as_str()), (Some("approval"), Some("then")));
        assert_eq!(plan("approval", false)["goto"], json!([{ "slot": "else", "to": "decide" }]));
        let b = plan("triage", true);
        assert_eq!((b["file"].as_str(), b["line"].as_u64().is_some()), (Some("steps/triage.ts"), true));
    }

    /// A query's text is for builders: someone who only uses the app reads that the step looks data up.
    #[test]
    fn a_query_is_for_builders() {
        let ns = nodes("team-requests", "list_requests");
        let latest = ns.iter().find(|n| n.name == "latest").unwrap();
        assert_eq!((step_plan(latest, false)["query"].as_bool(), step_plan(latest, false).get("query_text").is_none()), (Some(true), true));
        assert!(step_plan(latest, true)["query_text"].as_str().is_some_and(|t| t.to_lowercase().contains("select")), "{}", step_plan(latest, true));
    }
}
