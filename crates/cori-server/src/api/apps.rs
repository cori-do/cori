// SPDX-License-Identifier: Apache-2.0
//! Apps: their records, creation and deletion, owners and builders, their OIDC client, and the caller's own apps.
use super::*;

#[derive(Debug, Clone)]
pub struct AppRecord { pub id: String, pub owner: Option<String>, pub builders: Vec<String>, pub name: String, pub roles: Vec<String>, pub template: Option<String>, /// The promoted version: what `/apps/{app}/` serves and the live stores belong to
    pub promoted: Option<String>, /// Every promotion needs an approver (console toggle, `cori apps protect`)
    pub protected: bool, pub created_at: String, /// its own OIDC client (client hook); fronts sign in with `cori-web` without one
    pub client_id: Option<String> }

pub(crate) async fn app_record(st: &AppState, id: &str) -> Result<Option<AppRecord>, ApiError> {
    let r = sqlx::query("SELECT id, promoted_hash, created_at, owner, builders, name, roles, template, protected, client_id FROM apps WHERE id = $1").bind(id).fetch_optional(&st.db).await?;
    Ok(r.as_ref().map(app_record_of))
}

fn app_record_of(r: &sqlx::postgres::PgRow) -> AppRecord {
    let id: String = r.get("id");
    AppRecord { promoted: r.get("promoted_hash"), created_at: r.get("created_at"), owner: r.get("owner"), builders: r.get::<Option<String>, _>("builders").and_then(|b| serde_json::from_str(&b).ok()).unwrap_or_default(), name: r.get::<Option<String>, _>("name").unwrap_or_else(|| id.clone()), roles: r.get::<Option<String>, _>("roles").and_then(|b| serde_json::from_str(&b).ok()).unwrap_or_default(), template: r.get("template"), protected: r.get::<Option<i64>, _>("protected").unwrap_or(0) != 0, client_id: r.get("client_id"), id }
}

/// The app or a 404.
pub(crate) async fn require_app(st: &AppState, app: &str) -> Result<AppRecord, ApiError> {
    app_record(st, app).await?.ok_or(ApiError(StatusCode::NOT_FOUND, format!("unknown app `{app}`")))
}

/// The record every `list_apps` / `get_app` / MCP `get_app` answer carries.
pub(crate) async fn app_summary(st: &AppState, rec: &AppRecord) -> Result<Value, ApiError> {
    let since = (chrono::Utc::now() - chrono::Duration::hours(24)).to_rfc3339();
    let workflows: i64 = match &rec.promoted { Some(h) => sqlx::query("SELECT workflows FROM versions WHERE hash = $1").bind(h).fetch_optional(&st.db).await?.and_then(|v| serde_json::from_str::<Vec<Value>>(&v.get::<String, _>("workflows")).ok()).map(|w| w.len() as i64).unwrap_or(0), None => 0 };
    let runs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM runs WHERE app = $1 AND started_at > $2").bind(&rec.id).bind(&since).fetch_one(&st.db).await?;
    let pending: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM approvals WHERE app = $1 AND status = 'pending'").bind(&rec.id).fetch_one(&st.db).await?;
    let last = sqlx::query("SELECT hash, status, created_at, created_by FROM versions WHERE app = $1 ORDER BY created_at DESC LIMIT 1").bind(&rec.id).fetch_optional(&st.db).await?;
    let versions: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM versions WHERE app = $1").bind(&rec.id).fetch_one(&st.db).await?;
    let promoted_release = match &rec.promoted { Some(h) => Some(release_label_of(st, h).await), None => None };
    Ok(json!({ "id": rec.id, "name": rec.name, "promoted_release": promoted_release, "owner": rec.owner, "builders": rec.builders, "roles": rec.roles, "template": rec.template, "created_at": rec.created_at,
        "promoted": rec.promoted, "protected": rec.protected, "client_id": rec.client_id, "workflows": workflows, "runs_24h": runs, "pending_approvals": pending, "versions": versions,
        "last_version": last.map(|r| json!({ "hash": r.get::<String, _>("hash"), "status": r.get::<String, _>("status"), "created_at": r.get::<String, _>("created_at"), "created_by": r.get::<String, _>("created_by") })),
        "front_url": rec.promoted.as_ref().map(|_| st.front_url(&rec.id)), "console_url": crate::names::console_app_url(&st.cfg.public_url, &rec.id),
        "pull": format!("cori pull {} ./{}", rec.id, rec.id) }))
}

pub(crate) async fn create_app_record(st: &Arc<AppState>, who: &Principal, id: &str, name: Option<&str>, roles: &[String], builders: &[String], template: Option<&str>) -> Result<AppRecord, ApiError> {
    // who may not create an app is told so first: nothing of the ids already taken
    require_human(who, "creating an app")?;
    if !may_create_apps(who) { return Err(ApiError(StatusCode::FORBIDDEN, cannot_create_app(st, who).await)); }
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') { return Err(ApiError(StatusCode::BAD_REQUEST, "app id: letters, digits, - and _ only".into())); }
    if let Some(why) = st.tenancy.as_ref().and_then(|t| t.app_id_problem(id)) { return Err(ApiError(StatusCode::BAD_REQUEST, format!("app id `{id}`: {why}"))); }
    if st.cfg.fronts_url.is_some() { if let Some(why) = crate::names::front_app_id_problem(id) { return Err(ApiError(StatusCode::BAD_REQUEST, format!("app id `{id}`: {why}"))); } }
    if app_record(st, id).await?.is_some() { return Err(ApiError(StatusCode::CONFLICT, format!("app `{id}` already exists"))); }
    crate::usage::check_apps(st).await?;
    // an earlier app of this id whose deletion the identity provider has not heard of yet: its `app.deleted` would follow
    // this one's `app.created`
    if crate::client_hook::undelivered(st, id).await? { return Err(ApiError(StatusCode::CONFLICT, format!("app `{id}`: the identity provider has not yet heard that an earlier app of this id was deleted; try again once it has (`cori identity events --app {id}`)"))); }
    sqlx::query("INSERT INTO apps (id, promoted_hash, created_at, owner, builders, name, roles, template, protected) VALUES ($1, NULL, $2, $3, $4, $5, $6, $7, 0)").bind(id).bind(chrono::Utc::now().to_rfc3339()).bind(&who.sub).bind(serde_json::to_string(builders).unwrap()).bind(name.unwrap_or(id)).bind(serde_json::to_string(roles).unwrap()).bind(template).execute(&st.db).await?;
    db::audit(&st.db, &who.sub, "app.create", id, json!({ "builders": builders, "roles": roles, "template": template })).await?;
    // every app signs its users in with its own client: no client, no app (a client the organization is still creating, a
    // pending one, will do)
    if let Err(e) = sync_client(st, id).await {
        sqlx::query("DELETE FROM apps WHERE id = $1").bind(id).execute(&st.db).await?;
        return Err(ApiError(StatusCode::BAD_GATEWAY, format!("app `{id}` not created: {e}")));
    }
    // the roles it starts with, one by one, when the hook hears of roles
    let ignore = crate::client_hook::deployment_roles(st);
    let start: BTreeMap<String, crate::client_hook::RoleDecl> = roles.iter().filter(|r| !ignore.contains(*r)).map(|r| (r.clone(), Default::default())).collect();
    if let Err(e) = crate::client_hook::sync_roles(st, id, &start).await { tracing::warn!(app = id, error = %e, "the app's first roles were not queued for the identity provider"); }
    Ok(app_record(st, id).await?.unwrap())
}

/// Registers (or updates) the app's OIDC client through the client hook (`client_hook`) with its owner and the roles of the
/// versions it keeps, auditing the outcome.
pub(crate) async fn sync_client(st: &Arc<AppState>, app: &str) -> Result<crate::client_hook::Client, String> {
    let roles: Vec<String> = crate::client_hook::kept_roles(st, app).await.map_err(|e| e.to_string())?.into_keys().collect();
    sync_client_with(st, app, &roles).await
}

/// The roles a version declares: endpoint and front allow lists and the keys of `[auth] roles`. Roles only named by a
/// human step's routing are not listed; the role page takes any role name.
pub(crate) fn declared_roles(wfs: &[bundles::WorkflowEntry], cori_toml: &str) -> Vec<String> {
    let names = |v: Option<&toml::Value>| v.and_then(|a| a.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect::<Vec<String>>()).unwrap_or_default();
    let t: toml::Value = toml::from_str(cori_toml).unwrap_or(toml::Value::Table(Default::default()));
    let mut roles = names(t.get("front").and_then(|f| f.get("allow")).and_then(|a| a.get("roles")));
    roles.extend(t.get("auth").and_then(|a| a.get("roles")).and_then(|r| r.as_table()).map(|r| r.keys().cloned().collect::<Vec<_>>()).unwrap_or_default());
    for w in wfs { if let Some(a) = w.compiled.manifest.endpoint.as_ref().and_then(|e| e.allow.as_ref()).and_then(|a| a.get("roles")).and_then(|r| r.as_array()) { roles.extend(a.iter().filter_map(|x| x.as_str().map(String::from))); } }
    roles.sort(); roles.dedup();
    roles
}

pub(crate) async fn sync_client_with(st: &Arc<AppState>, app: &str, extra_roles: &[String]) -> Result<crate::client_hook::Client, String> {
    use crate::client_hook::Client;
    let rec = app_record(st, app).await.map_err(|e| e.1)?.ok_or_else(|| format!("unknown app `{app}`"))?;
    let owner = rec.owner.as_deref();
    let mut roles = rec.roles.clone(); roles.extend(extra_roles.iter().cloned()); roles.sort(); roles.dedup();
    match crate::client_hook::provision(st, app, owner, &roles).await {
        Ok(Client::Bound(id)) => { let _ = db::audit(&st.db, owner.unwrap_or("cori"), "app.client", app, json!({ "client_id": id })).await; Ok(Client::Bound(id)) }
        Ok(Client::Pending(note)) => { let _ = db::audit(&st.db, owner.unwrap_or("cori"), "app.client.pending", app, json!({ "note": note })).await; Ok(Client::Pending(note)) }
        Ok(Client::None) => Ok(Client::None),
        Err(e) => {
            tracing::warn!(app, "client hook: {e}");
            let _ = db::audit(&st.db, owner.unwrap_or("cori"), "app.client.failed", app, json!({ "error": e })).await;
            Err(e)
        }
    }
}

/// `POST /v1/apps/{app}/oidc-client` (`cori apps sync-client`): calls the client hook again (idempotent) and stores its answer.
pub(crate) async fn post_app_client(InRealm(st): InRealm, Path(app): Path<String>, Caller(who): Caller) -> Res {
    require_human(&who, "syncing an app's sign-in client")?;
    let rec = app_record(&st, &app).await?.ok_or(ApiError(StatusCode::NOT_FOUND, format!("unknown app `{app}`")))?;
    require_owner_or_admin(&who, &rec, "syncing the app's sign-in client")?;
    if crate::client_hook::hook_url(&st).is_none() { return Err(ApiError(StatusCode::CONFLICT, "no client hook: set CORI_AUTH_CLIENT_HOOK on the server (builtin auth)".into())); }
    let client = sync_client(&st, &app).await.map_err(|e| ApiError(StatusCode::BAD_GATEWAY, e))?;
    let (client_id, status, note) = match client { crate::client_hook::Client::Bound(id) => (Some(id), "ready", None), crate::client_hook::Client::Pending(n) => (rec.client_id.clone(), "pending", Some(n)), crate::client_hook::Client::None => (None, "none", None) };
    Ok(Json(json!({ "app": app, "client_id": client_id, "client_status": status, "note": note, "redirect_uri": crate::client_hook::callback_url(&st, &app) })).into_response())
}

pub(crate) async fn create_app(InRealm(st): InRealm, Caller(who): Caller, Json(body): Json<Value>) -> Res {
    let id = body["id"].as_str().unwrap_or("").to_string();
    let strings = |k: &str| body[k].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect::<Vec<String>>()).unwrap_or_default();
    let rec = create_app_record(&st, &who, &id, body["name"].as_str(), &strings("roles"), &strings("builders"), body["template"].as_str()).await?;
    Ok((StatusCode::CREATED, Json(app_summary(&st, &rec).await?)).into_response())
}

/// Unauthenticated: what `cori login` needs to start an OIDC flow, and where the MCP lives.
pub(crate) async fn auth_config(InRealm(st): InRealm) -> Res {
    let base = st.cfg.public_url.trim_end_matches('/');
    let mut v = match &st.cfg.auth {
        crate::auth::AuthMode::Builtin { issuer, .. } => {
            let mut v = json!({ "mode": "builtin", "issuer": issuer, "client_id": st.cfg.auth_client_id, "cli_client_id": st.cfg.cli_client_id, "web_client_id": st.cfg.web_client_id, "mcp": format!("{base}/mcp") });
            // the resource a sign-in names (RFC 8707), and the scopes it asks for besides its own (`openid`, `offline_access`…):
            // an issuer chooses the token's audience by one or the other
            if let Some(r) = st.cfg.sign_in_resource() { v["resource"] = json!(r); }
            if !st.cfg.auth_scopes.is_empty() { v["scopes"] = json!(st.cfg.auth_scopes); }
            // the client MCP hosts sign in with, when the issuer registers none for them
            if let Some(id) = &st.cfg.mcp_client_id { v["mcp_client_id"] = json!(id); }
            if let Some(port) = st.cfg.mcp_callback_port { v["mcp_callback_port"] = json!(port); }
            v
        }
        crate::auth::AuthMode::Dev => json!({ "mode": "dev", "hint": "Authorization: Bearer dev:<sub>:<role,role>", "mcp": format!("{base}/mcp") }),
        crate::auth::AuthMode::Off => json!({ "mode": "off", "mcp": format!("{base}/mcp") }),
    };
    // the deployment's plugin for coding agents, which the console's Connect your agent page offers first
    if let Some(plugin) = &st.cfg.agent_plugin { v["agent_plugin"] = json!(plugin); }
    // where people manage their own account at the identity provider
    if let Some(account) = &st.cfg.account_url { v["account_url"] = json!(account); }
    // the hosts of fronts and releases (`https://*.<domain>`), and the organization's apps portal: a tool that frames
    // releases checks the callbacks it sends a sign-in back to, and lets its frames load them
    if let Some(f) = &st.cfg.fronts_url { v["fronts_url"] = json!(f.pattern()); }
    if let Some(p) = st.portal_url() { v["portal_url"] = json!(p); }
    Ok(Json(v).into_response())
}

/// RFC 9728 protected resource metadata: an MCP host finds the org's issuer here and runs OAuth against it, asking for a
/// token of this resource (accepted by `/mcp` only), a refresh token (`offline_access`), so it stays signed in, and the
/// deployment's own scopes (`CORI_AUTH_SCOPES`).
pub(crate) async fn oauth_protected_resource(InRealm(st): InRealm) -> Res {
    let base = st.cfg.public_url.trim_end_matches('/');
    let servers = match &st.cfg.auth { crate::auth::AuthMode::Builtin { issuer, .. } => vec![issuer.clone()], _ => vec![] };
    let scopes = crate::sign_in_scopes(&MCP_SCOPES, &st.cfg.auth_scopes);
    Ok(Json(json!({ "resource": format!("{base}/mcp"), "authorization_servers": servers, "bearer_methods_supported": ["header"], "scopes_supported": scopes, "resource_name": st.brand.name })).into_response())
}

/// The scopes an MCP host asks for, before the deployment's own.
pub(crate) const MCP_SCOPES: [&str; 4] = ["openid", "profile", "email", "offline_access"];

pub(crate) async fn list_agent_sessions(InRealm(st): InRealm, Caller(who): Caller) -> Res {
    require_human(&who, "listing agent sessions")?;
    let rows = if who.is_admin() { sqlx::query("SELECT jti, sub, app, mcp_session, issued_at, expires_at, revoked_at FROM agent_sessions ORDER BY issued_at DESC LIMIT 200").fetch_all(&st.db).await? } else { sqlx::query("SELECT jti, sub, app, mcp_session, issued_at, expires_at, revoked_at FROM agent_sessions WHERE sub = $1 ORDER BY issued_at DESC LIMIT 200").bind(&who.sub).fetch_all(&st.db).await? };
    Ok(Json(rows.iter().map(|r| json!({ "jti": r.get::<String, _>("jti"), "sub": r.get::<String, _>("sub"), "app": r.get::<String, _>("app"), "mcp_session": r.get::<Option<String>, _>("mcp_session"), "issued_at": r.get::<String, _>("issued_at"), "expires_at": r.get::<String, _>("expires_at"), "revoked_at": r.get::<Option<String>, _>("revoked_at") })).collect::<Vec<_>>()).into_response())
}

pub(crate) async fn revoke_agent_session(InRealm(st): InRealm, Path(jti): Path<String>, Caller(who): Caller) -> Res {
    require_human(&who, "revoking an agent session")?;
    let n = if who.is_admin() { sqlx::query("UPDATE agent_sessions SET revoked_at = $1 WHERE jti = $2 AND revoked_at IS NULL").bind(chrono::Utc::now().to_rfc3339()).bind(&jti).execute(&st.db).await?.rows_affected() } else { sqlx::query("UPDATE agent_sessions SET revoked_at = $1 WHERE jti = $2 AND sub = $3 AND revoked_at IS NULL").bind(chrono::Utc::now().to_rfc3339()).bind(&jti).bind(&who.sub).execute(&st.db).await?.rows_affected() };
    if n == 0 { return Err(ApiError(StatusCode::NOT_FOUND, "no such active agent session".into())); }
    db::audit(&st.db, &who.sub, "agent_session.revoke", &jti, Value::Null).await?;
    Ok(Json(json!({ "revoked": jti })).into_response())
}

/// `POST /v1/apps/{app}/protect { "protected": bool }`: every promotion of the app needs an approver (owner or admin, human).
pub(crate) async fn protect(InRealm(st): InRealm, Path(app): Path<String>, Caller(who): Caller, Json(body): Json<Value>) -> Res {
    require_human(&who, "protecting an app")?;
    let rec = app_record(&st, &app).await?.ok_or(ApiError(StatusCode::NOT_FOUND, format!("unknown app `{app}`")))?;
    require_owner_or_admin(&who, &rec, "protecting the app")?;
    let on = body.get("protected").and_then(|b| b.as_bool()).unwrap_or(true);
    sqlx::query("UPDATE apps SET protected = $1 WHERE id = $2").bind(if on { 1i64 } else { 0 }).bind(&app).execute(&st.db).await?;
    db::audit(&st.db, &who.sub, "app.protect", &app, json!({ "protected": on })).await?;
    let rec = app_record(&st, &app).await?.unwrap();
    Ok(Json(app_summary(&st, &rec).await?).into_response())
}

pub(crate) fn delete_approval_id(app: &str) -> String { format!("delete:{app}") }

/// `DELETE /v1/apps/{app}?confirm=<app>` (`cori apps delete`): the app and everything it holds. Gated three ways:
/// only its owner or an admin asks, typing the app id back; a protected app is refused until it is unprotected; and an
/// approver decides. An approver asking deletes at once; anyone else's request is parked as `delete:<app>` in the approvals
/// inbox and the deletion runs when an approver approves it.
pub(crate) async fn delete_app(InRealm(st): InRealm, Path(app): Path<String>, Caller(who): Caller, axum::extract::Query(q): axum::extract::Query<BTreeMap<String, String>>) -> Res {
    require_human(&who, "deleting an app")?;
    let rec = app_record(&st, &app).await?.ok_or(ApiError(StatusCode::NOT_FOUND, format!("unknown app `{app}`")))?;
    require_owner_or_admin(&who, &rec, "deleting the app")?;
    if q.get("confirm").map(String::as_str) != Some(app.as_str()) { return Err(ApiError(StatusCode::BAD_REQUEST, format!("deleting an app cannot be undone: add `?confirm={app}` (`cori apps delete {app} --confirm {app}`)"))); }
    if rec.protected { return Err(ApiError(StatusCode::CONFLICT, format!("`{app}` is protected: lift the protection first (`cori apps protect {app} --off`)"))); }
    if is_approver(&st, &who) {
        let out = do_delete_app(&st, &app, &who, &who.sub).await?;
        return Ok(Json(out).into_response());
    }
    let aid = delete_approval_id(&app);
    let versions: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM versions WHERE app = $1").bind(&app).fetch_one(&st.db).await?;
    let stores: Vec<String> = sqlx::query("SELECT DISTINCT name FROM store_instances WHERE app = $1 AND target = 'live' ORDER BY name").bind(&app).fetch_all(&st.db).await?.iter().map(|r| r.get::<String, _>("name")).collect();
    let payload = json!({ "kind": "app_deletion", "app": app, "requested_by": who.sub, "reason": "delete the app", "promoted": rec.promoted, "versions": versions, "live_stores": stores });
    sqlx::query("INSERT INTO approvals (id, run_id, app, step, to_role, payload, status, created_at) VALUES ($1, $2, $3, 'delete', 'approver', $4, 'pending', $5) ON CONFLICT ON CONSTRAINT approvals_pkey DO UPDATE SET payload = excluded.payload, status = 'pending', decided_by = NULL, decided_at = NULL, created_at = excluded.created_at")
        .bind(&aid).bind(&aid).bind(&app).bind(payload.to_string()).bind(chrono::Utc::now().to_rfc3339()).execute(&st.db).await?;
    db::audit(&st.db, &who.sub, "app.delete.requested", &app, json!({ "versions": versions, "live_stores": stores })).await?;
    Ok((StatusCode::ACCEPTED, Json(json!({ "app": app, "status": "pending_approval", "approval_id": aid, "approver_roles": st.settings.approver_roles, "next": format!("an approver approves `{aid}` in the console inbox; the deletion follows") }))).into_response())
}

/// Deletes an app for good: its running runs are terminated, every store database (live and sandboxes) dropped, its schedules
/// removed, every version's bundle deleted, and its registry rows (versions, runs and their events, approvals, promotions,
/// LLM captures and bindings, webhook deliveries, idempotency keys) with them; its agent sessions are revoked. The audit
/// keeps its history. `requested_by` is who asked (the approver deciding a parked request is `who`).
pub(crate) async fn do_delete_app(st: &Arc<AppState>, app: &str, who: &Principal, requested_by: &str) -> Result<Value, ApiError> {
    require_human(who, "deleting an app")?;
    require_power(st, who, &[Power::Approver], "deciding an app deletion")?;
    let rec = app_record(st, app).await?.ok_or(ApiError(StatusCode::NOT_FOUND, format!("unknown app `{app}`")))?;
    if rec.protected { return Err(ApiError(StatusCode::CONFLICT, format!("`{app}` is protected: lift the protection first (`cori apps protect {app} --off`)"))); }
    let running: Vec<String> = sqlx::query("SELECT id FROM runs WHERE app = $1 AND status = 'running'").bind(app).fetch_all(&st.db).await?.iter().map(|r| r.get::<String, _>("id")).collect();
    let terminated = terminate_runs(st, &running, &format!("app {app} deleted")).await;
    // the databases first: when dropping fails the app stays (unpromoted runs aside) and the deletion can be retried
    let targets: Vec<String> = sqlx::query("SELECT DISTINCT target FROM store_instances WHERE app = $1 AND target <> $2").bind(app).bind(cori_store::LOCAL).fetch_all(&st.db).await?.iter().map(|r| r.get::<String, _>("target")).collect();
    let mut dropped = Vec::new();
    for t in &targets {
        let names = if t == cori_store::LIVE { crate::stores::drop_live(st, app).await } else { crate::stores::drop_sandbox(st, crate::stores::Target::sandbox(app, t)).await }
            .map_err(|e| ApiError(StatusCode::BAD_GATEWAY, format!("store databases of `{app}` ({t}) not dropped, app kept: {e}")))?;
        dropped.extend(names.into_iter().map(|n| format!("{n}@{}", if t == cori_store::LIVE { t.clone() } else { t.trim_start_matches("sha256:").get(..12).unwrap_or(t).to_string() })));
    }
    // nothing promoted any more: the schedule sync removes the app's Temporal schedules
    sqlx::query("UPDATE apps SET promoted_hash = NULL WHERE id = $1").bind(app).execute(&st.db).await?;
    crate::schedules::sync_app(st, app).await;
    let hashes: Vec<String> = sqlx::query("SELECT hash FROM versions WHERE app = $1").bind(app).fetch_all(&st.db).await?.iter().map(|r| r.get::<String, _>("hash")).collect();
    for h in &hashes { remove_version_files(st, h).await; }
    // every file its runs were given, live and releases'
    remove_files_of(st, app, None).await;
    let (p1, p2) = (format!("{app}:"), format!("{app}@"));
    sqlx::query("DELETE FROM run_events WHERE run_id IN (SELECT id FROM runs WHERE app = $1)").bind(app).execute(&st.db).await?;
    for sql in ["DELETE FROM runs WHERE app = $1", "DELETE FROM approvals WHERE app = $1", "DELETE FROM deployments WHERE app = $1", "DELETE FROM versions WHERE app = $1", "DELETE FROM llm_calls WHERE app = $1", "DELETE FROM llm_budgets WHERE app = $1",
                "DELETE FROM bindings WHERE app = $1", "DELETE FROM webhook_deliveries WHERE app = $1", "DELETE FROM schedules WHERE app = $1", "DELETE FROM store_instances WHERE app = $1", "DELETE FROM release_events WHERE app = $1", "DELETE FROM preview_roles WHERE app = $1", "DELETE FROM app_limits WHERE app = $1"] {
        sqlx::query(sql).bind(app).execute(&st.db).await?;
    }
    sqlx::query("DELETE FROM idempotency_keys WHERE substr(idem_key, 1, $1) = $2 OR substr(idem_key, 1, $1) = $3").bind(p1.len() as i32).bind(&p1).bind(&p2).execute(&st.db).await?;
    sqlx::query("DELETE FROM rate_events WHERE substr(key, 1, $1) = $2").bind(p1.len() as i32).bind(&p1).execute(&st.db).await?;
    sqlx::query("UPDATE agent_sessions SET revoked_at = $1 WHERE app = $2 AND revoked_at IS NULL").bind(chrono::Utc::now().to_rfc3339()).bind(app).execute(&st.db).await?;
    // the app's own credentials go with it, and the key wrapper hears that their key's scope is gone; so do the accounts
    // connected for it, whose providers are told
    if let Err(e) = st.broker.forget_app(&st.db, app).await { tracing::warn!(app, "the app's credentials: {e:#}"); }
    if let Err(e) = crate::connections::forget(st, &crate::connections::Owner::App(app.to_string())).await { tracing::warn!(app, "the app's connections: {e:#}"); }
    sqlx::query("DELETE FROM apps WHERE id = $1").bind(app).execute(&st.db).await?;
    // the identity provider hears of it through the outbox (delivered until it is answered)
    if let Err(e) = crate::client_hook::deprovision(st, app, rec.client_id.as_deref()).await { tracing::warn!(app, "client hook: {e:#}"); let _ = db::audit(&st.db, &who.sub, "app.client.delete_failed", app, json!({ "client_id": rec.client_id, "error": format!("{e:#}") })).await; }
    db::audit(&st.db, &who.sub, "app.delete", app, json!({ "requested_by": requested_by, "owner": rec.owner, "versions": hashes, "stores_dropped": dropped, "terminated_runs": terminated })).await?;
    Ok(json!({ "app": app, "status": "deleted", "versions_deleted": hashes.len(), "stores_dropped": dropped, "terminated_runs": terminated }))
}

pub(crate) async fn get_app(InRealm(st): InRealm, Path(app): Path<String>, front: Option<axum::Extension<FrontHost>>, Caller(who): Caller) -> Res {
    let rec = require_app(&st, &app).await?;
    let wf_json = |w: &bundles::WorkflowEntry| json!({ "id": w.id, "endpoint": w.compiled.manifest.endpoint, "schedule": w.compiled.manifest.schedule, "parameters": w.compiled.manifest.parameters, "result": w.compiled.manifest.result });
    // the workflows the request's host runs: a release's host its release's, else the promoted version's
    let release = front.as_ref().and_then(|f| f.0.release);
    let serves = match release { Some(seq) => Some(resolve_version(&st, &app, &format!("v{seq}")).await?), None => promoted_version(&st, &app).await? };
    if is_owner_or_builder(&who, &rec) {
        let mut v = app_summary(&st, &rec).await?;
        v["app"] = json!(app); v["build"] = json!(true); v["app_roles"] = json!(known_app_roles(&st, &rec).await?);
        // without a store database no deploy or promotion creates the app's stores: the console says so
        v["stores_configured"] = json!(st.cfg.store_database_url.is_some());
        if let Some((_, wfs, _)) = &serves { v["promoted_workflows"] = json!(wfs.iter().map(wf_json).collect::<Vec<_>>()); }
        if let Some(seq) = release { v["release"] = json!(format!("v{seq}")); }
        return Ok(Json(v).into_response());
    }
    // one of the app's users: its front and the workflows their roles may start, nothing about how it is built
    let scoped = scope_to_app(&st, who, &app).await?;
    let roles = member_roles(&st, &scoped, &rec).await?;
    if roles.is_empty() { return Err(ApiError(StatusCode::FORBIDDEN, format!("you hold none of `{app}`'s roles; ask its owner ({}) for access", rec.owner.as_deref().unwrap_or("nobody")))); }
    let mut v = json!({ "id": app, "app": app, "name": rec.name, "promoted": rec.promoted, "roles": roles, "build": false, "front_url": rec.promoted.as_ref().map(|_| st.front_url(&app)) });
    if let Some(h) = &rec.promoted { v["promoted_release"] = json!(release_label_of(&st, h).await); }
    if let Some(seq) = release { v["release"] = json!(format!("v{seq}")); }
    if let Some((_, wfs, _)) = &serves {
        let allowed = |w: &&bundles::WorkflowEntry| w.compiled.manifest.endpoint.as_ref().map(|e| {
            let want: Vec<String> = e.allow.as_ref().and_then(|a| a.get("roles")).and_then(|r| r.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
            want.is_empty() || want.iter().any(|r| roles.contains(r))
        }).unwrap_or(false);
        v["promoted_workflows"] = json!(wfs.iter().filter(allowed).map(wf_json).collect::<Vec<_>>());
    }
    Ok(Json(v).into_response())
}

/// `GET /v1/me`: the caller, and what the console shows them (`persona`: `admin` sees the platform, `builder` builds apps,
/// `user` uses the apps their roles open) with the platform powers behind it; a guest (`guest`) none of them, whatever
/// else they hold. `apps_only`: they reach their apps and nothing else, a guest or a person who holds no platform power
/// (`holds_power`) and builds no app (`bearer_guard`). The server checks every call anyway.
pub(crate) async fn me(InRealm(st): InRealm, Caller(who): Caller) -> Res {
    let guest = who.is_guest();
    let builds_any = if guest { false } else if who.is_admin() || who.is_builder() { true } else {
        let mut any = false;
        for r in sqlx::query("SELECT id FROM apps").fetch_all(&st.db).await? { if let Some(rec) = app_record(&st, &r.get::<String, _>("id")).await? { if is_owner_or_builder(&who, &rec) { any = true; break; } } }
        any
    };
    let apps_only = guest || (who.kind == crate::auth::PrincipalKind::Human && !builds_any && !holds_power(&st, &who));
    let persona = if guest { "user" } else if who.is_admin() { "admin" } else if builds_any { "builder" } else { "user" };
    let mut v = json!({ "sub": who.sub, "roles": who.roles, "email": who.email, "kind": who.kind, "scopes": who.scopes, "app": who.app, "persona": persona, "guest": guest, "apps_only": apps_only,
        "can": if guest { json!({}) } else { json!({ "admin": who.is_admin(), "create_apps": may_create_apps(&who), "build": builds_any, "operate": who.is_operator(), "audit": who.is_auditor(), "usage": who.is_billing(), "approve": is_approver(&st, &who), "store_admin": who.is_store_admin() }) } });
    if let Some(mut org) = organization_of(&st) {
        // its name, as the identity provider says it (cached a minute)
        if let Ok(Some(info)) = crate::client_hook::ask_cached(&st, "organization.get", json!({ "by": who.sub }), std::time::Duration::from_secs(60)).await { org["name"] = info["name"].clone(); }
        v["organization"] = org;
    }
    // the organization's people: those who build its apps or hold a power
    v["can"]["people"] = json!(!who.is_agent() && !apps_only);
    // where the organization's people find their apps, and the console's address (from the portal's host)
    if let Some(p) = st.portal_url() { v["portal_url"] = json!(p); }
    v["console_url"] = json!(format!("{}/", st.cfg.public_url.trim_end_matches('/')));
    // the providers a person connects their own accounts to here (`connections`)
    v["providers"] = json!(st.deployment.providers.values().map(|p| json!({ "name": p.name, "label": p.label })).collect::<Vec<_>>());
    // what waits for an admin's decision: apps shared outside the organization
    if who.is_admin() && !guest && !who.is_agent() {
        v["access_waiting"] = json!(sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM access_requests WHERE status = 'awaiting_approval'").fetch_one(&st.db).await?);
    }
    Ok(Json(v).into_response())
}

/// `GET /v1/me/organizations`: the organizations the caller belongs to on this server (the identity provider's answer to
/// `person.organizations`), the one they act in marked `current`; none for a server of a single organization. People only.
pub(crate) async fn my_organizations(InRealm(st): InRealm, Caller(who): Caller) -> Res {
    require_human(&who, "your organizations")?;
    let Some(current) = organization_of(&st) else { return Ok(Json(json!({ "organizations": [] })).into_response()) };
    let listed = person_organizations(&st, &who).await;
    let mut out: Vec<Value> = listed.as_ref().and_then(|l| l["organizations"].as_array().cloned()).unwrap_or_default();
    if !out.iter().any(|o| o["id"] == current["id"]) { out.insert(0, current.clone()); }
    for o in out.iter_mut() { o["current"] = json!(o["id"] == current["id"]); }
    let mut v = json!({ "organizations": out });
    // whether they may start an organization of their own, where, or why not: the console offers it to those who may not
    // create apps here
    if let Some(start) = listed.as_ref().and_then(|l| start_of(&l["start"])) { v["start"] = start; }
    Ok(Json(v).into_response())
}

/// The identity provider's answer to `person.organizations` for `who` (cached a minute), None when it gives none.
async fn person_organizations(st: &AppState, who: &Principal) -> Option<Value> {
    crate::client_hook::ask_cached(st, "person.organizations", json!({ "person": who.sub }), std::time::Duration::from_secs(60)).await.unwrap_or_else(|e| { tracing::warn!(error = %e, "person.organizations"); None })
}

/// Where `who` may start an organization of their own, under a tenancy whose identity provider lets them
/// (`person.organizations`' `start`): the way to build apps for someone who may not create any where they are.
pub(crate) async fn own_organization_url(st: &AppState, who: &Principal) -> Option<String> {
    organization_of(st)?;
    start_of(&person_organizations(st, who).await?["start"])?["url"].as_str().map(String::from)
}

/// Why `who` may not create an app here, with the ways on: the builder role, which an admin of the organization gives, or
/// an organization of their own when they may start one (`own_organization_url`).
pub(crate) async fn cannot_create_app(st: &AppState, who: &Principal) -> String {
    let refused = "creating an app needs the builder role (or an admin one)";
    match own_organization_url(st, who).await {
        Some(url) => format!("{refused}: ask an admin of this organization for it, or start an organization of your own ({url}) and sign in to it"),
        None => refused.into(),
    }
}

/// What `person.organizations` says of starting an organization of one's own: where (`url`: https, or http on this
/// computer), or why not (`reason`, a sentence shown as it is); None for anything else.
pub(crate) fn start_of(v: &Value) -> Option<Value> {
    if let Some(url) = v["url"].as_str() {
        let ok = reqwest::Url::parse(url).is_ok_and(|u| u.scheme() == "https" || (u.scheme() == "http" && matches!(u.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))));
        return ok.then(|| json!({ "url": url }));
    }
    let reason: String = v["reason"].as_str()?.trim().chars().take(300).collect();
    (!reason.is_empty()).then(|| json!({ "reason": reason }))
}

/// The organization a request acts in, under a tenancy (its realm and its slug); None for the core's single realm.
pub(crate) fn organization_of(st: &AppState) -> Option<Value> {
    st.tenancy.as_ref()?;
    let realm = st.realm()?;
    Some(json!({ "id": realm.as_str(), "slug": (!st.settings.slug.is_empty()).then_some(&st.settings.slug) }))
}

/// `GET /v1/me/apps`: the apps the caller can open: those they build (`build`) and those where they hold a role
/// (`roles`, their app roles), with each one's front, what waits for them there and the live stores they open (`stores`).
pub(crate) async fn my_apps(InRealm(st): InRealm, Caller(who): Caller) -> Res {
    // the apps portal's person (`api::sessions`): the apps to open, and what waits for them there (a go-live, an app's
    // deletion are the console's)
    let portal = who.claims.get("kind").and_then(|k| k.as_str()) == Some("portal");
    // one query of each kind, whatever the number of apps
    let recs: Vec<AppRecord> = sqlx::query("SELECT id, promoted_hash, created_at, owner, builders, name, roles, template, protected, client_id FROM apps ORDER BY id").fetch_all(&st.db).await?.iter().map(app_record_of).collect();
    let mut live: BTreeMap<String, (Option<i64>, String, String)> = BTreeMap::new();
    for r in sqlx::query("SELECT a.id, v.seq, v.hash, v.cori_toml FROM apps a JOIN versions v ON v.hash = a.promoted_hash").fetch_all(&st.db).await? { live.insert(r.get("id"), (r.get("seq"), r.get("hash"), r.get("cori_toml"))); }
    let mut pending: BTreeMap<String, Vec<(String, String, String, Option<String>)>> = BTreeMap::new();
    for r in sqlx::query("SELECT a.app, a.id, a.to_role, COALESCE(r.user_sub, '') AS user_sub, r.store_target FROM approvals a LEFT JOIN runs r ON r.id = a.run_id WHERE a.status = 'pending'").fetch_all(&st.db).await? {
        pending.entry(r.get("app")).or_default().push((r.get("id"), r.get("to_role"), r.get("user_sub"), r.get("store_target")));
    }
    let mut my_runs: BTreeMap<String, i64> = BTreeMap::new();
    for r in sqlx::query("SELECT app, COUNT(*) AS n FROM runs WHERE user_sub = $1 GROUP BY app").bind(&who.sub).fetch_all(&st.db).await? { my_runs.insert(r.get("app"), r.get("n")); }
    // each live store, with the roles it defines
    let mut stores: BTreeMap<String, Vec<(String, Vec<String>)>> = BTreeMap::new();
    for r in sqlx::query("SELECT app, name, roles_yaml FROM store_instances WHERE target = 'live' ORDER BY name").fetch_all(&st.db).await? {
        stores.entry(r.get("app")).or_default().push((r.get("name"), store_roles(&r.get::<String, _>("roles_yaml"))));
    }
    let approver = is_approver(&st, &who);
    let mut out = Vec::new();
    for rec in recs {
        if who.is_agent() && who.app.as_deref() != Some(rec.id.as_str()) { continue; }
        let build = is_owner_or_builder(&who, &rec);
        let scoped = who.clone().in_app(&rec.id, rec.client_id.as_deref());
        let roles = member_roles(&st, &scoped, &rec).await?;
        if !build && roles.is_empty() { continue; }
        let promoted = live.get(&rec.id);
        // on the portal, an app with nothing live is its builders' only (they open it in the console)
        if portal && promoted.is_none() && !build { continue; }
        // what waits for this caller: the same routing as the approvals inbox
        let waiting = pending.get(&rec.id).map(|rows| rows.iter().filter(|(id, to, owner, target)| {
            if portal && (is_platform_approval(id) || !RunScope::Live.sees(approval_target(id, target.clone()).as_deref())) { return false; }
            scoped.roles.contains(to) || (to == "user:initiator" && owner == &who.sub) || (to == "approver" && approver) || who.is_admin()
        }).count()).unwrap_or(0);
        // the stores they open: every one for its builders, those where they hold a role for its users
        let opens: Vec<String> = stores.remove(&rec.id).unwrap_or_default().into_iter().filter(|(_, defined)| build || defined.iter().any(|r| roles.contains(r))).map(|(name, _)| name).collect();
        out.push(json!({ "id": rec.id, "name": rec.name, "build": build, "owner": rec.owner, "roles": roles, "promoted": rec.promoted,
            "promoted_release": promoted.map(|(seq, hash, _)| release_label(*seq, hash)), "protected": rec.protected,
            "description": promoted.and_then(|(_, _, toml)| app_description(toml)),
            // served by the portal's host, from the live release (404 without one: the page draws the app's initial)
            "icon": promoted.map(|_| format!("/__cori/icons/{}", rec.id)),
            "front_url": promoted.map(|_| st.front_url(&rec.id)), "stores": opens, "waiting_for_me": waiting, "my_runs": my_runs.get(&rec.id).copied().unwrap_or(0) }));
    }
    Ok(Json(out).into_response())
}

/// `PUT /v1/apps/{app}/builders` `{ "builders": ["sub", …], "owner"?: "sub" }`: who builds the app besides its owner, and
/// who owns it. The owner or an admin, who may hand it to another owner.
pub(crate) async fn put_builders(InRealm(st): InRealm, Path(app): Path<String>, Caller(who): Caller, Json(body): Json<Value>) -> Res {
    require_human(&who, "changing an app's builders")?;
    let rec = require_app(&st, &app).await?;
    require_owner_or_admin(&who, &rec, "setting the builders")?;
    let mut builders: Vec<String> = body["builders"].as_array().ok_or(ApiError(StatusCode::BAD_REQUEST, "`builders`: a list of user ids".into()))?.iter().filter_map(|b| b.as_str().map(|s| s.trim().to_string())).filter(|b| !b.is_empty()).collect();
    builders.sort(); builders.dedup();
    let owner = match body["owner"].as_str().map(str::trim).filter(|o| !o.is_empty()) {
        Some(o) if Some(o) != rec.owner.as_deref() => Some(o.to_string()),
        _ => rec.owner.clone(),
    };
    builders.retain(|b| Some(b.as_str()) != owner.as_deref());
    sqlx::query("UPDATE apps SET builders = $1, owner = $2 WHERE id = $3").bind(serde_json::to_string(&builders).unwrap()).bind(&owner).bind(&app).execute(&st.db).await?;
    db::audit(&st.db, &who.sub, "app.builders", &app, json!({ "builders": builders, "owner": owner, "previous": { "builders": rec.builders, "owner": rec.owner } })).await?;
    // the identity provider keeps the owner: who may grant the app's roles
    if owner != rec.owner { crate::client_hook::tell_updated(&st, &app).await?; }
    Ok(Json(json!({ "app": app, "owner": owner, "builders": builders })).into_response())
}

/// `GET /v1/apps/{app}/me`: the caller inside one app, what a front shows and gates on: `roles` are the app's roles (its
/// client roles), `platform_roles` the organization's (admin, builder, approver).
pub(crate) async fn app_me(InRealm(st): InRealm, Path(app): Path<String>, headers: HeaderMap) -> Res {
    let who = app_principal(&st, &headers, &app).await?;
    Ok(Json(json!({ "sub": who.sub, "roles": who.roles, "platform_roles": who.platform_roles(), "email": who.email, "kind": who.kind, "scopes": who.scopes, "app": app })).into_response())
}

pub(crate) async fn list_apps(InRealm(st): InRealm, Caller(who): Caller) -> Res {
    Ok(Json(built_apps(&st, &who).await?).into_response())
}

/// The summaries of the apps `who` builds (owner, builders, admins; an agent session its own app): `GET /v1/apps` and the
/// MCP `list_apps`.
pub(crate) async fn built_apps(st: &AppState, who: &Principal) -> Result<Vec<Value>, ApiError> {
    let mut out = Vec::new();
    for r in sqlx::query("SELECT id FROM apps ORDER BY id").fetch_all(&st.db).await? {
        let Some(rec) = app_record(st, &r.get::<String, _>("id")).await? else { continue };
        if is_owner_or_builder(who, &rec) { out.push(app_summary(st, &rec).await?); }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starting_an_organization_is_offered_at_a_safe_address_or_with_a_reason() {
        let start = |v: Value| start_of(&v);
        assert_eq!(start(json!({ "url": "https://auth.cori.do/new-organization" })), Some(json!({ "url": "https://auth.cori.do/new-organization" })));
        // http on this computer only (a laptop's identity provider)
        assert!(start(json!({ "url": "http://127.0.0.1:8098/new-organization" })).is_some());
        for bad in ["http://auth.example/new", "javascript:alert(1)", "data:text/html,x", "/new-organization", "not a url"] { assert_eq!(start(json!({ "url": bad })), None, "{bad}"); }
        // a reason, trimmed and bounded
        assert_eq!(start(json!({ "reason": "  Closed beta.  " })), Some(json!({ "reason": "Closed beta." })));
        assert_eq!(start(json!({ "reason": "x".repeat(400) })).and_then(|v| v["reason"].as_str().map(str::len)), Some(300));
        // nothing to offer
        for none in [json!(null), json!({}), json!({ "reason": "  " }), json!("https://auth.cori.do")] { assert_eq!(start(none.clone()), None, "{none}"); }
    }
}
