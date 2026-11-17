// SPDX-License-Identifier: Apache-2.0
//! Store calls over HTTP: the workers' run-scoped path, an app's live and sandbox stores, and store lists.
use super::*;

/// The principal for a store call from a worker: the run's own user (`X-Cori-Run` + worker token), with the run's app and
/// store target (`live`, or the version hash of a preview run's sandbox).
pub(crate) async fn run_store_principal(st: &Arc<AppState>, headers: &HeaderMap, via: &crate::workers::Via) -> Result<Option<(Principal, crate::stores::Target)>, ApiError> {
    let Some(run) = headers.get("x-cori-run").and_then(|v| v.to_str().ok()) else { return Ok(None) };
    let worker = crate::workers::authenticate(st, headers, via).await?;
    if let crate::workers::Worker::Pool(_) = &worker {
        let r = crate::workers::run_ref(st, run).await?.ok_or(ApiError(StatusCode::FORBIDDEN, "unknown or finished run".into()))?;
        if !crate::workers::serves_app(&worker, &crate::workers::app_connectors(st, &r.app, &r.version).await?) { return Err(ApiError(StatusCode::FORBIDDEN, format!("{} does not serve app `{}`", worker.label(), r.app))); }
    }
    let r = sqlx::query("SELECT user_json, app, store_target FROM runs WHERE id = $1 AND status = 'running'").bind(run).fetch_optional(&st.db).await?.ok_or(ApiError(StatusCode::FORBIDDEN, "unknown or finished run".into()))?;
    let u: Value = r.get::<Option<String>, _>("user_json").and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(Value::Null);
    let who = Principal { sub: u["sub"].as_str().unwrap_or("run").to_string(), roles: u["roles"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default(), email: u["email"].as_str().map(String::from), claims: u.clone(), token: None, kind: crate::auth::PrincipalKind::Service, scopes: vec![], app: None, session_id: None, platform: u["platform_roles"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()) };
    let at = crate::stores::Target { app: r.get("app"), target: r.get::<Option<String>, _>("store_target").unwrap_or_else(|| cori_store::LIVE.into()) };
    Ok(Some((who, at)))
}

pub(crate) async fn store_rpc_response(st: &Arc<AppState>, at: crate::stores::Target, name: String, who: Principal, inspect: bool, req: Value) -> Res {
    let v = crate::stores::rpc(st, at, name, who, inspect, req).await.map_err(|e| ApiError(crate::stores::status_of(&e), e.to_string()))?;
    Ok(Json(v).into_response())
}

/// `POST /v1/stores/{name}/mcp`: the workers' path. The run decides the app and whether the call lands on the live
/// database or the version's sandbox; people use the app-scoped paths.
pub(crate) async fn store_mcp(InRealm(st): InRealm, Path(name): Path<String>, via: crate::workers::Via, headers: HeaderMap, Json(req): Json<Value>) -> Res {
    let Some((who, at)) = run_store_principal(&st, &headers, &via).await? else { return Err(ApiError(StatusCode::BAD_REQUEST, format!("store calls are app-scoped: POST /v1/apps/<app>/stores/{name}/mcp (the live database) or /v1/apps/<app>/versions/<hash>/stores/{name}/mcp (a version's sandbox)"))) };
    store_rpc_response(&st, at, name, who, false, req).await
}

/// `POST /v1/apps/{app}/stores/{name}/mcp`: the live database, as the caller's store role; the app's owner, builders and
/// admins without one read as the inspector (audited).
pub(crate) async fn app_store_mcp(InRealm(st): InRealm, Path((app, name)): Path<(String, String)>, headers: HeaderMap, Json(req): Json<Value>) -> Res {
    let who = app_principal(&st, &headers, &app).await?;
    let rec = require_app(&st, &app).await?;
    let at = crate::stores::Target::live(&app);
    let inspect = may_inspect(&st, &who, &rec, &at);
    store_rpc_response(&st, at, name, who, inspect, req).await
}

/// Who may open a version's sandbox besides its builders: approvers, and under dev auth anyone (a preview is tried as the
/// app's own personas, as `start_preview_run` lets them).
pub(crate) fn sandbox_guest(st: &AppState, who: &Principal) -> bool { matches!(st.cfg.auth, crate::auth::AuthMode::Dev) || (!who.is_agent() && is_approver(st, who)) }

/// `POST /v1/apps/{app}/versions/{hash}/stores/{name}/mcp`: a version's sandbox (owner, builders, approvers; agents with
/// `stores:read`; any persona under dev auth), as the caller's store role or the inspector.
pub(crate) async fn version_store_mcp(InRealm(st): InRealm, Path((app, hash, name)): Path<(String, String, String)>, headers: HeaderMap, Json(req): Json<Value>) -> Res {
    let who = app_principal(&st, &headers, &app).await?;
    require_scope(&who, "stores:read", Some(&app))?;
    let rec = require_builder(&st, &who, &app, sandbox_guest(&st, &who), "the preview data").await?;
    let (hash, _, _) = resolve_version(&st, &app, &hash).await?;
    let at = crate::stores::Target::sandbox(&app, &hash);
    let inspect = may_inspect(&st, &who, &rec, &at);
    store_rpc_response(&st, at, name, who, inspect, req).await
}

pub(crate) fn store_row_json(r: &sqlx::postgres::PgRow) -> Value {
    json!({ "app": r.get::<String, _>("app"), "name": r.get::<String, _>("name"), "target": r.get::<String, _>("target"), "database": r.get::<String, _>("db_name"), "cluster": r.get::<String, _>("cluster"), "roles": store_roles(&r.get::<String, _>("roles_yaml")), "seeded": r.get::<i64, _>("seeded") != 0, "updated_at": r.get::<String, _>("updated_at") })
}

/// The roles a store defines, from its recorded role files (`store_instances.roles_yaml`: role → file).
pub(crate) fn store_roles(roles_yaml: &str) -> Vec<String> {
    serde_json::from_str::<BTreeMap<String, String>>(roles_yaml).map(|m| m.into_keys().filter(|r| r != cori_store::INSPECTOR).collect()).unwrap_or_default()
}

/// `GET /v1/apps/{app}/stores`: the app's store databases: live ones and every version's sandbox.
/// Builders and approvers get every target; the app's users the live stores where they hold a role, by name and with
/// their own roles there (where a store lives, and its other roles, are its builders' business).
pub(crate) async fn list_app_stores(InRealm(st): InRealm, Path(app): Path<String>, Caller(who): Caller) -> Res {
    let rec = require_app(&st, &app).await?;
    let all = is_owner_or_builder(&who, &rec) || is_approver(&st, &who) || who.is_operator();
    if all {
        let rows = sqlx::query("SELECT app, name, target, db_name, cluster, roles_yaml, seeded, updated_at FROM store_instances WHERE app = $1 ORDER BY name, target").bind(&app).fetch_all(&st.db).await?;
        return Ok(Json(rows.iter().map(store_row_json).collect::<Vec<_>>()).into_response());
    }
    let held = member_roles(&st, &scope_to_app(&st, who, &app).await?, &rec).await?;
    if held.is_empty() { return Err(ApiError(StatusCode::FORBIDDEN, format!("you hold none of `{app}`'s roles"))); }
    let rows = sqlx::query("SELECT name, roles_yaml, updated_at FROM store_instances WHERE app = $1 AND target = 'live' ORDER BY name").bind(&app).fetch_all(&st.db).await?;
    let theirs: Vec<Value> = rows.iter().filter_map(|r| {
        let roles: Vec<String> = store_roles(&r.get::<String, _>("roles_yaml")).into_iter().filter(|x| held.contains(x)).collect();
        (!roles.is_empty()).then(|| json!({ "app": app, "name": r.get::<String, _>("name"), "target": cori_store::LIVE, "roles": roles, "updated_at": r.get::<String, _>("updated_at") }))
    }).collect();
    Ok(Json(theirs).into_response())
}

/// `GET /v1/stores`: every store database on the server (the console's Operate page).
pub(crate) async fn list_stores(InRealm(st): InRealm, Caller(who): Caller) -> Res {
    require_power(&st, &who, &[Power::Operator], "every store database of the server")?;
    let rows = sqlx::query("SELECT app, name, target, db_name, cluster, roles_yaml, seeded, updated_at FROM store_instances ORDER BY app, name, target").fetch_all(&st.db).await?;
    Ok(Json(rows.iter().map(store_row_json).collect::<Vec<_>>()).into_response())
}

/// `POST /v1/store-logins/rotate` (`cori store rotate-logins`): new passwords for the store databases' own logins, every
/// app's or `{"app": …}`'s (`stores::rotate_logins`). Admins only.
pub(crate) async fn rotate_store_logins(InRealm(st): InRealm, Caller(who): Caller, body: Option<Json<Value>>) -> Res {
    require_human(&who, "rotating store logins")?;
    require_power(&st, &who, &[Power::Admin], "rotating store logins")?;
    let app = body.and_then(|b| b.0["app"].as_str().map(String::from)).filter(|a| !a.is_empty());
    if let Some(a) = &app { require_app(&st, a).await?; }
    let rotated = crate::stores::rotate_logins(&st, app.clone()).await.map_err(|e| ApiError(crate::stores::status_of(&e), format!("{e:#}")))?;
    db::audit(&st.db, &who.sub, "stores.rotate_logins", app.as_deref().unwrap_or("*"), json!({ "databases": rotated.iter().filter_map(|r| r["database"].as_str()).collect::<Vec<_>>() })).await?;
    Ok(Json(json!({ "rotated": rotated })).into_response())
}
