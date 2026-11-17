// SPDX-License-Identifier: Apache-2.0
//! LLM bindings, captured calls, meters and usage.
use super::*;

/// Meters per app (collected now, billed later): runs, steps, LLM tokens and cost, sandbox minutes, approvals.
pub(crate) async fn meters(InRealm(st): InRealm, Path(app): Path<String>, Caller(who): Caller) -> Res {
    require_builder(&st, &who, &app, who.is_operator() || who.is_billing(), "the meters").await?;
    let u = app_usage(&st.db, &app, None, false).await?;
    Ok(Json(json!({
        "app": app,
        "runs": u.runs,
        "steps": u.steps,
        "llm": { "calls": u.llm_calls, "tokens_in": u.tokens_in, "tokens_out": u.tokens_out, "cost_eur": u.cost_eur, "month": crate::llm::month_usage(&st.db, &app).await? },
        "sandbox_ms": u.sandbox_ms,
        "approvals": u.approvals,
        "storage": crate::usage::app_storage(&st.db, &st.settings.limits, &app).await?,
    })).into_response())
}

/// `PUT /v1/apps/{app}/limits` `{ "storage_bytes": 500000000 | "2GB" | null }`: the storage the app may take, set by the
/// organization's admins, at most the organization's for each app (`Limits::storage_bytes_per_app`); `null` leaves it
/// that alone. Over it, the app's deploys, uploads and store writes are refused (`usage::storage_refusal`). Answers the
/// app's storage as the meters show it.
pub(crate) async fn put_app_limits(InRealm(st): InRealm, Path(app): Path<String>, Caller(who): Caller, Json(b): Json<Value>) -> Res {
    require_human(&who, "setting an app's limits")?;
    require_power(&st, &who, &[Power::Admin], "setting an app's limits")?;
    require_app(&st, &app).await?;
    let Some(v) = b.get("storage_bytes") else { return Err(ApiError(StatusCode::BAD_REQUEST, "storage_bytes: a size (`2GB`, `500MB`) or a number of bytes; null for no limit of its own".into())) };
    let own: Option<u64> = match v {
        Value::Null => None,
        v => Some(cori_compiler::files::parse_size(v).map_err(|e| ApiError(StatusCode::BAD_REQUEST, format!("storage_bytes: {e}; null for no limit of its own")))?),
    };
    let per_app = st.settings.limits.storage_bytes_per_app;
    if let Some(n) = own && per_app > 0 && n > per_app {
        return Err(ApiError(StatusCode::BAD_REQUEST, format!("the organization allows each app at most {}: set `{app}` a limit at or below it, or null", crate::usage::human(per_app as f64))));
    }
    let previous: Option<i64> = sqlx::query_scalar("SELECT storage_bytes FROM app_limits WHERE app = $1").bind(&app).fetch_optional(&st.db).await?.flatten();
    match own {
        None => { sqlx::query("DELETE FROM app_limits WHERE app = $1").bind(&app).execute(&st.db).await?; }
        Some(n) => {
            sqlx::query("INSERT INTO app_limits (app, storage_bytes, updated_at, updated_by) VALUES ($1, $2, $3, $4) ON CONFLICT ON CONSTRAINT app_limits_pkey DO UPDATE SET storage_bytes = $2, updated_at = $3, updated_by = $4")
                .bind(&app).bind(n as i64).bind(chrono::Utc::now().to_rfc3339()).bind(&who.sub).execute(&st.db).await?;
        }
    }
    db::audit(&st.db, &who.sub, "app.limits", &app, json!({ "storage_bytes": own, "previous": previous })).await?;
    Ok(Json(crate::usage::app_storage(&st.db, &st.settings.limits, &app).await?).into_response())
}

/// What one app used, all time or within `[from, to)`: runs and approvals by status, step executions, LLM calls (every
/// call, or the primary ones only: a call in a module's mode evaluates a model, it is not billed), tokens and cost, and the
/// milliseconds its `cli` steps ran on the sandbox db. `meters` and `usage` read the same numbers.
pub(crate) struct AppUsage { runs: BTreeMap<String, i64>, steps: i64, llm_calls: i64, tokens_in: i64, tokens_out: i64, cost_eur: f64, sandbox_ms: i64, approvals: BTreeMap<String, i64> }

pub(crate) async fn app_usage(db: &crate::db::Db, app: &str, window: Option<(&str, &str)>, primary_llm_only: bool) -> Result<AppUsage, ApiError> {
    // every query binds the app, then the window's bounds when there is one; the SQL is built from constants only
    async fn rows(db: &crate::db::Db, sql: String, app: &str, window: Option<(&str, &str)>) -> Result<Vec<sqlx::postgres::PgRow>, ApiError> {
        let mut q = sqlx::query(sqlx::AssertSqlSafe(sql)).bind(app.to_string());
        if let Some((from, to)) = window { q = q.bind(from.to_string()).bind(to.to_string()); }
        Ok(q.fetch_all(db).await?)
    }
    let within = |col: &str| if window.is_some() { format!(" AND {col} >= $2 AND {col} < $3") } else { String::new() };
    let by_status = |rs: Vec<sqlx::postgres::PgRow>| rs.iter().map(|r| (r.get::<String, _>("status"), r.get::<i64, _>("n"))).collect::<BTreeMap<_, _>>();
    let runs = by_status(rows(db, format!("SELECT status, COUNT(*) AS n FROM runs WHERE app = $1{} GROUP BY status", within("started_at")), app, window).await?);
    let steps = rows(db, format!("SELECT COUNT(*) AS n FROM run_events e JOIN runs r ON r.id = e.run_id WHERE r.app = $1 AND e.kind = 'step'{}", within("e.at")), app, window).await?.first().map(|r| r.get::<i64, _>("n")).unwrap_or(0);
    let llm = rows(db, format!("SELECT COUNT(*) AS calls, CAST(COALESCE(SUM(tokens_in),0) AS BIGINT) AS tokens_in, CAST(COALESCE(SUM(tokens_out),0) AS BIGINT) AS tokens_out, COALESCE(SUM(cost_eur),0.0) AS cost_eur FROM llm_calls WHERE app = $1{}{}", within("at"), if primary_llm_only { " AND mode = 'primary'" } else { "" }), app, window).await?;
    let llm = llm.first().ok_or(ApiError(StatusCode::INTERNAL_SERVER_ERROR, "llm usage".into()))?;
    // summed here from the `cli` step events, whose payloads are TEXT
    let cli = rows(db, format!("SELECT e.payload FROM run_events e JOIN runs r ON r.id = e.run_id WHERE r.app = $1 AND e.kind = 'step' AND e.payload LIKE '%\"kind\":\"cli\"%'{}", within("e.at")), app, window).await?;
    let sandbox_ms = cli.iter().filter_map(|r| serde_json::from_str::<Value>(&r.get::<String, _>("payload")).ok()).map(|p| p["ms"].as_i64().unwrap_or(0)).sum();
    let approvals = by_status(rows(db, format!("SELECT status, COUNT(*) AS n FROM approvals WHERE app = $1{} GROUP BY status", within("created_at")), app, window).await?);
    Ok(AppUsage { runs, steps, llm_calls: llm.get("calls"), tokens_in: llm.get("tokens_in"), tokens_out: llm.get("tokens_out"), cost_eur: llm.get("cost_eur"), sandbox_ms, approvals })
}

pub(crate) async fn llm_calls(InRealm(st): InRealm, Path(app): Path<String>, Caller(who): Caller) -> Res {
    require_builder(&st, &who, &app, who.is_operator(), "the LLM calls").await?;
    let rows = sqlx::query("SELECT id, run_id, workflow, step, tier, model, mode, schema_valid, latency_ms, cost_eur, at, label, input, output FROM llm_calls WHERE app = $1 ORDER BY at DESC LIMIT 200").bind(&app).fetch_all(&st.db).await?;
    Ok(Json(rows.iter().map(|r| json!({ "id": r.get::<String, _>("id"), "run_id": r.get::<String, _>("run_id"), "workflow": r.get::<String, _>("workflow"), "step": r.get::<String, _>("step"), "tier": r.get::<String, _>("tier"), "model": r.get::<String, _>("model"), "mode": r.get::<String, _>("mode"), "schema_valid": r.get::<i32, _>("schema_valid") == 1, "latency_ms": r.get::<i64, _>("latency_ms"), "cost_eur": r.get::<f64, _>("cost_eur"), "at": r.get::<String, _>("at"), "label": r.get::<Option<String>, _>("label"), "input": serde_json::from_str::<Value>(&r.get::<String, _>("input")).unwrap_or(Value::Null), "output": r.get::<Option<String>, _>("output").and_then(|o| serde_json::from_str::<Value>(&o).ok()) })).collect::<Vec<_>>()).into_response())
}

pub(crate) async fn list_bindings(InRealm(st): InRealm, Path(app): Path<String>, Caller(who): Caller) -> Res {
    require_builder(&st, &who, &app, who.is_operator(), "the LLM calls").await?;
    let rows = sqlx::query("SELECT workflow, step, tier, provider, model, mode, updated_at, updated_by FROM bindings WHERE app = $1 ORDER BY workflow, step, mode").bind(&app).fetch_all(&st.db).await?;
    Ok(Json(rows.iter().map(|r| json!({ "workflow": r.get::<String, _>("workflow"), "step": r.get::<String, _>("step"), "tier": r.get::<String, _>("tier"), "provider": r.get::<String, _>("provider"), "model": r.get::<String, _>("model"), "mode": r.get::<String, _>("mode"), "updated_at": r.get::<String, _>("updated_at"), "updated_by": r.get::<String, _>("updated_by") })).collect::<Vec<_>>()).into_response())
}

/// Bind a model to (workflow, step) as `primary`, or in a mode a module adds (`Module::binding_modes`) for its
/// `llm` step hook; `off` then removes the step's bindings in those modes. Step `*` binds a tier for the whole app.
pub(crate) async fn set_binding(InRealm(st): InRealm, Path(app): Path<String>, Caller(who): Caller, Json(b): Json<Value>) -> Res {
    require_human(&who, "bindings")?;
    require_power(&st, &who, &[Power::Operator], "binding a model")?;
    let (wf, step, tier, provider, model, mode) = (b["workflow"].as_str().unwrap_or("*"), b["step"].as_str().unwrap_or("*"), b["tier"].as_str().unwrap_or("small"), b["provider"].as_str().unwrap_or("openai"), b["model"].as_str().unwrap_or(""), b["mode"].as_str().unwrap_or("primary"));
    if model.is_empty() { return Err(ApiError(StatusCode::BAD_REQUEST, "model required".into())); }
    // the deployment's starter model is one model (its lease refuses any other)
    if provider == crate::llm::STARTER {
        match &st.cfg.llm_starter {
            Some(s) if s.model == model => {}
            Some(s) => return Err(ApiError(StatusCode::BAD_REQUEST, format!("the starter model is `{}` only", s.model))),
            None => return Err(ApiError(StatusCode::BAD_REQUEST, "this server offers no starter model".into())),
        }
    }
    let extra = st.cfg.modules.binding_modes();
    if !(mode == "primary" || extra.contains(&mode) || (mode == "off" && !extra.is_empty())) {
        let modes: Vec<&str> = std::iter::once("primary").chain(extra.iter().copied()).chain((!extra.is_empty()).then_some("off")).collect();
        return Err(ApiError(StatusCode::BAD_REQUEST, format!("mode `{mode}`: this server binds {}", modes.join(" | "))));
    }
    let id = format!("{app}:{wf}:{step}:{mode}");
    if mode == "off" { for m in &extra { sqlx::query("DELETE FROM bindings WHERE id = $1").bind(format!("{app}:{wf}:{step}:{m}")).execute(&st.db).await?; } }
    else { sqlx::query("INSERT INTO bindings (id, app, workflow, step, tier, provider, model, mode, updated_at, updated_by) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10) ON CONFLICT ON CONSTRAINT bindings_pkey DO UPDATE SET tier = $5, provider = $6, model = $7, updated_at = $9, updated_by = $10").bind(&id).bind(&app).bind(wf).bind(step).bind(tier).bind(provider).bind(model).bind(mode).bind(chrono::Utc::now().to_rfc3339()).bind(&who.sub).execute(&st.db).await?; }
    db::audit(&st.db, &who.sub, "llm.bind", &id, b.clone()).await?;
    crate::schedules::sync_app(&st, &app).await;
    Ok(Json(json!({ "ok": true, "binding": id })).into_response())
}

/// Monthly usage per app (billing groundwork): runs by status, steps, LLM calls/tokens/cost, sandbox ms, approvals.
/// `?month=2025-09` (default: the current month). With the current month, `ledger`: the usage ledger's month
/// (`crate::usage`, what the limits are checked against) and the limits. Billing and admins.
pub(crate) async fn usage(InRealm(st): InRealm, Caller(who): Caller, axum::extract::Query(q): axum::extract::Query<std::collections::HashMap<String, String>>) -> Res {
    require_power(&st, &who, &[Power::Billing, Power::Admin], "the usage")?;
    let month = q.get("month").cloned().unwrap_or_else(|| chrono::Utc::now().format("%Y-%m").to_string());
    let (from, to) = { let y: i32 = month[..4].parse().unwrap_or_else(|_| chrono::Datelike::year(&chrono::Utc::now())); let m: u32 = month[5..].parse().unwrap_or(1); let f = chrono::NaiveDate::from_ymd_opt(y, m, 1).unwrap(); let t = if m == 12 { chrono::NaiveDate::from_ymd_opt(y + 1, 1, 1).unwrap() } else { chrono::NaiveDate::from_ymd_opt(y, m + 1, 1).unwrap() }; (format!("{f}T00:00:00"), format!("{t}T00:00:00")) };
    let apps: Vec<String> = sqlx::query("SELECT DISTINCT app FROM runs WHERE started_at >= $1 AND started_at < $2 ORDER BY app").bind(&from).bind(&to).fetch_all(&st.db).await?.iter().map(|r| r.get::<String, _>("app")).collect();
    let mut out = Vec::new();
    let mut total = json!({ "runs": 0, "steps": 0, "llm_calls": 0, "tokens": 0, "llm_cost_eur": 0.0, "sandbox_ms": 0, "approvals": 0 });
    for app in apps {
        let u = app_usage(&st.db, &app, Some((&from, &to)), true).await?;
        let (runs_n, approvals): (i64, i64) = (u.runs.values().sum(), u.approvals.values().sum());
        total["runs"] = json!(total["runs"].as_i64().unwrap() + runs_n); total["steps"] = json!(total["steps"].as_i64().unwrap() + u.steps); total["llm_calls"] = json!(total["llm_calls"].as_i64().unwrap() + u.llm_calls); total["tokens"] = json!(total["tokens"].as_i64().unwrap() + u.tokens_in + u.tokens_out); total["llm_cost_eur"] = json!(total["llm_cost_eur"].as_f64().unwrap() + u.cost_eur); total["sandbox_ms"] = json!(total["sandbox_ms"].as_i64().unwrap() + u.sandbox_ms); total["approvals"] = json!(total["approvals"].as_i64().unwrap() + approvals);
        out.push(json!({ "app": app, "runs": runs_n, "runs_by_status": u.runs, "steps": u.steps, "llm": { "calls": u.llm_calls, "tokens_in": u.tokens_in, "tokens_out": u.tokens_out, "cost_eur": u.cost_eur }, "sandbox_ms": u.sandbox_ms, "approvals": approvals }));
    }
    let ledger = if month == chrono::Utc::now().format("%Y-%m").to_string() { crate::usage::summary(&st).await? } else { Value::Null };
    Ok(Json(json!({ "month": month, "from": from, "to": to, "apps": out, "total": total, "ledger": ledger })).into_response())
}
