// SPDX-License-Identifier: Apache-2.0
//! The connector registry, leases, worker pools and broker secrets.
use super::*;

/// Org admins create a named pool and receive its key once. `cori work --pool <name> --key <key>` polls `cori.pool.<name>`.
pub(crate) async fn create_pool(InRealm(st): InRealm, Caller(who): Caller, Json(body): Json<Value>) -> Res {
    require_human(&who, "creating pools")?;
    require_power(&st, &who, &[Power::Admin], "creating a worker pool")?;
    let name = body.get("name").and_then(|n| n.as_str()).filter(|n| n.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')).ok_or(ApiError(StatusCode::BAD_REQUEST, "pool name: [a-z0-9-]".into()))?.to_string();
    let key = format!("pk_{}", uuid::Uuid::new_v4().simple());
    sqlx::query("INSERT INTO pools (name, key_hash, created_by, created_at) VALUES ($1, $2, $3, $4) ON CONFLICT ON CONSTRAINT pools_pkey DO UPDATE SET key_hash = $2").bind(&name).bind(sha(&key)).bind(&who.sub).bind(chrono::Utc::now().to_rfc3339()).execute(&st.db).await?;
    db::audit(&st.db, &who.sub, "pool.create", &name, json!({})).await?;
    let queue = crate::names::pool_queue(&st.cfg, st.db.realm(), &name);
    Ok((StatusCode::CREATED, Json(json!({ "name": name, "key": key, "task_queue": queue, "hint": format!("cori work --pool {name} --key <key> --server {}", st.cfg.public_url) }))).into_response())
}

pub(crate) async fn list_pools(InRealm(st): InRealm, Caller(who): Caller) -> Res {
    require_power(&st, &who, &[Power::Operator, Power::Builder], "the worker pools")?;
    let rows = sqlx::query("SELECT name, created_by, created_at, last_seen, capabilities FROM pools ORDER BY name").fetch_all(&st.db).await?;
    Ok(Json(rows.iter().map(|r| json!({ "name": r.get::<String, _>("name"), "created_by": r.get::<String, _>("created_by"), "created_at": r.get::<String, _>("created_at"), "last_seen": r.get::<Option<String>, _>("last_seen"), "capabilities": r.get::<Option<String>, _>("capabilities").and_then(|c| serde_json::from_str::<Value>(&c).ok()) })).collect::<Vec<_>>()).into_response())
}

/// A worker proves its pool key and reports capabilities (binaries, connectors it can serve). Returns the queue to poll.
pub(crate) async fn pool_heartbeat(InRealm(st): InRealm, Path(name): Path<String>, headers: HeaderMap, Json(body): Json<Value>) -> Res {
    let key = headers.get("x-cori-pool-key").and_then(|v| v.to_str().ok()).unwrap_or("");
    let row = sqlx::query("SELECT key_hash FROM pools WHERE name = $1").bind(&name).fetch_optional(&st.db).await?.ok_or(ApiError(StatusCode::NOT_FOUND, "unknown pool".into()))?;
    if row.get::<String, _>("key_hash") != sha(key) { return Err(ApiError(StatusCode::UNAUTHORIZED, "bad pool key".into())); }
    sqlx::query("UPDATE pools SET last_seen = $1, capabilities = $2 WHERE name = $3").bind(chrono::Utc::now().to_rfc3339()).bind(body.get("capabilities").map(|c| c.to_string()).unwrap_or("{}".into())).bind(&name).execute(&st.db).await?;
    // the pool authenticates its own calls with its key (X-Cori-Pool + X-Cori-Pool-Key); the cluster's token stays in the cluster
    Ok(Json(json!({ "task_queue": crate::names::pool_queue(&st.cfg, st.db.realm(), &name) })).into_response())
}

/// Write-only: the value is encrypted at rest and never returned. Rotation = a new version. With `app`, the secret is that
/// app's own (an app's credentials: its connectors', webhooks', schedules'): set by an admin or the app's owner, and its
/// runs find it before the deployment's secret of the same name.
pub(crate) async fn put_secret(InRealm(st): InRealm, Caller(who): Caller, Json(body): Json<Value>) -> Res {
    require_human(&who, "managing secrets")?;
    let name = body.get("name").and_then(|n| n.as_str()).filter(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')).ok_or(ApiError(StatusCode::BAD_REQUEST, "name: [A-Za-z0-9-_.]".into()))?.to_string();
    if crate::secrets::reserved_name(&name) { return Err(ApiError(StatusCode::BAD_REQUEST, format!("`{name}`: names starting with `{}` are Cori's own (a store database's logins)", crate::secrets::STORE_LOGIN_PREFIX))); }
    let app = body.get("app").and_then(|a| a.as_str()).filter(|a| !a.is_empty()).map(String::from);
    match &app {
        Some(a) => {
            let rec = require_app(&st, a).await?;
            if !(who.is_admin() || rec.owner.as_deref() == Some(who.sub.as_str())) { return Err(ApiError(StatusCode::FORBIDDEN, format!("an app's credentials are set by an admin or its owner ({})", rec.owner.as_deref().unwrap_or("nobody")))); }
        }
        None => require_power(&st, &who, &[Power::Admin], "managing secrets")?,
    }
    let value = body.get("value").and_then(|v| v.as_str()).ok_or(ApiError(StatusCode::BAD_REQUEST, "value required".into()))?;
    let kind = body.get("kind").and_then(|k| k.as_str()).unwrap_or("token");
    // a key service refusing (its policy, an outage) is said as it answered: the caller sets credentials for a living
    let v = st.broker.put_in(&st.db, app.as_deref(), &name, kind, value, &who.sub).await.map_err(|e| {
        let msg = format!("{e:#}");
        if st.broker.wrapper().is_some_and(|w| msg.starts_with(w.id())) { ApiError(StatusCode::BAD_GATEWAY, format!("the secret was not stored: {msg}")) } else { ApiError::from(e) }
    })?;
    db::audit(&st.db, &who.sub, "secret.put", &name, json!({ "version": v, "kind": kind, "app": app })).await?;
    Ok((StatusCode::CREATED, Json(json!({ "name": name, "app": app, "version": v }))).into_response())
}

/// The names of the secrets (never their values): the deployment's and every app's, or `?app=<id>`'s alone; with what wraps
/// each one's key (`master`, or the key wrapper's key).
pub(crate) async fn list_secrets(InRealm(st): InRealm, Caller(who): Caller, axum::extract::Query(q): axum::extract::Query<BTreeMap<String, String>>) -> Res {
    require_power(&st, &who, &[Power::Operator, Power::Builder], "the secret names")?;
    let rows = match q.get("app").filter(|a| !a.is_empty()) {
        Some(app) => sqlx::query("SELECT scope, name, kind, version, key_by, created_at FROM secrets WHERE revoked_at IS NULL AND scope = $1 ORDER BY name").bind(app).fetch_all(&st.db).await?,
        None => sqlx::query("SELECT scope, name, kind, version, key_by, created_at FROM secrets WHERE revoked_at IS NULL ORDER BY scope, name").fetch_all(&st.db).await?,
    };
    Ok(Json(rows.iter().map(|r| {
        let scope: String = r.get("scope");
        json!({ "name": r.get::<String, _>("name"), "app": (!scope.is_empty()).then_some(scope), "kind": r.get::<String, _>("kind"), "version": r.get::<i64, _>("version"), "key_by": r.get::<String, _>("key_by"), "updated_at": r.get::<String, _>("created_at") })
    }).collect::<Vec<_>>()).into_response())
}

/// A worker asks for a credential for (run, connector). The lease is recorded; the value goes only to the worker.
/// `POST /v1/leases`: a worker asks, for a running run, either the token of the run's user for a connector's
/// `audience` or the value of a connector's broker `secret`. Only connectors the run's app declares (or the org
/// registry provides) qualify, only with their own secret or audience, and only for a worker that serves them.
pub(crate) async fn lease(InRealm(st): InRealm, via: crate::workers::Via, headers: HeaderMap, Json(body): Json<Value>) -> Res {
    let worker = crate::workers::authenticate(&st, &headers, &via).await?;
    // a model call: the app's monthly LLM budget, and the key of an org provider
    if let Some(provider) = body["llm"].as_str() { return crate::llm::lease(&st, &worker, body["run_id"].as_str().unwrap_or(""), provider, &body).await; }
    // a deployment's connector: the access token of the connection it calls with (`connections`)
    if let Some(provider) = body["connection"].as_str() { return crate::connections::lease(&st, &worker, &body, provider).await; }
    let (run, connector) = (body["run_id"].as_str().unwrap_or("").to_string(), body["connector"].as_str().unwrap_or("").to_string());
    let (kind, asked) = match (body["audience"].as_str(), body["secret"].as_str()) {
        (Some(a), _) => ("audience", a.to_string()),
        (None, Some(s)) => ("secret", s.to_string()),
        _ => return Err(ApiError(StatusCode::BAD_REQUEST, "a lease names an `audience` or a `secret`".into())),
    };
    let refuse = |st: Arc<AppState>, outcome: &'static str, run: String, connector: String, name: String, worker: String| async move {
        sqlx::query("INSERT INTO secret_use (id, name, run_id, connector, worker, at, outcome) VALUES ($1, $2, $3, $4, $5, $6, $7)").bind(uuid::Uuid::new_v4().to_string()).bind(name).bind(run).bind(connector).bind(worker).bind(chrono::Utc::now().to_rfc3339()).bind(outcome).execute(&st.db).await.ok();
        Err::<axum::response::Response, ApiError>(ApiError(StatusCode::FORBIDDEN, format!("lease refused: {outcome}")))
    };
    let logged = if kind == "audience" { format!("audience:{asked}") } else { asked.clone() };
    let Some(r) = crate::workers::run_ref(&st, &run).await? else { return refuse(st.clone(), "unknown_run", run, connector, logged, worker.label()).await };
    if r.status != "running" { return refuse(st.clone(), "run_not_running", run, connector, logged, worker.label()).await; }
    let connectors = crate::workers::app_connectors(&st, &r.app, &r.version).await?;
    let Some(c) = connectors.get(&connector) else { return refuse(st.clone(), "connector_not_in_app", run, connector, logged, worker.label()).await };
    if !crate::workers::serves_connector(&worker, c) { return refuse(st.clone(), "worker_does_not_serve_connector", run, connector, logged, worker.label()).await; }
    if kind == "audience" {
        if c.audience.as_deref() != Some(asked.as_str()) { return refuse(st.clone(), "audience_not_declared", run, connector, logged, worker.label()).await; }
        let row = sqlx::query("SELECT principal_token, user_json FROM runs WHERE id = $1").bind(&run).fetch_one(&st.db).await?;
        let user: Value = row.get::<Option<String>, _>("user_json").and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(Value::Null);
        let token = match &st.cfg.auth {
            crate::auth::AuthMode::Dev => format!("dev:{}:{}", user["sub"].as_str().unwrap_or("run"), user["roles"].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(",")).unwrap_or_default()),
            crate::auth::AuthMode::Builtin { .. } => match crate::identity::audience_token(&st, &run, &r.app, row.get::<Option<String>, _>("principal_token").as_deref(), &user, &asked).await {
                Ok(t) => t,
                Err(e) => { tracing::warn!(run = %run, audience = %asked, error = %e, "lease refused"); return Err(ApiError(StatusCode::FORBIDDEN, format!("lease refused: {e}"))); }
            },
            crate::auth::AuthMode::Off => return refuse(st.clone(), "no_identity_in_off_mode", run, connector, logged, worker.label()).await,
        };
        sqlx::query("INSERT INTO secret_use (id, name, run_id, connector, worker, at, outcome) VALUES ($1, $2, $3, $4, $5, $6, 'exchanged')").bind(uuid::Uuid::new_v4().to_string()).bind(&logged).bind(&run).bind(&connector).bind(worker.label()).bind(chrono::Utc::now().to_rfc3339()).execute(&st.db).await?;
        // a native activity's token travels in its envelope, which Temporal writes into the run's history: sealed for the
        // connector's own worker, which holds its key; the platform's worker never sees it
        if c.kind == "activity" {
            let sealed = st.broker.seal_for_connector(&connector, &run, &token).map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
            return Ok(Json(json!({ "value": sealed, "sealed": true, "ttl_secs": 300, "audience": asked })).into_response());
        }
        return Ok(Json(json!({ "value": token, "ttl_secs": 300, "audience": asked })).into_response());
    }
    if c.secret.as_deref() != Some(asked.as_str()) { return refuse(st.clone(), "secret_not_declared", run, connector, logged, worker.label()).await; }
    // the run's app's own credential, else the deployment's of that name
    match st.broker.resolve(&st.db, &r.app, &asked).await {
        Ok((v, scope)) => {
            sqlx::query("INSERT INTO secret_use (id, name, run_id, connector, worker, at, outcome, scope) VALUES ($1, $2, $3, $4, $5, $6, 'granted', $7)").bind(uuid::Uuid::new_v4().to_string()).bind(&asked).bind(&run).bind(&connector).bind(worker.label()).bind(chrono::Utc::now().to_rfc3339()).bind(scope.unwrap_or_default()).execute(&st.db).await?;
            Ok(Json(json!({ "value": v, "ttl_secs": 300 })).into_response())
        }
        Err(e) => { tracing::warn!(secret = %asked, error = %e, "lease refused"); refuse(st.clone(), "unknown_secret", run, connector, logged, worker.label()).await }
    }
}

pub async fn registry_connector(db: &crate::db::Db, name: &str) -> Result<Option<cori_engine::config::ConnectorConfig>, ApiError> {
    let r = sqlx::query("SELECT config FROM connectors WHERE name = $1").bind(name).fetch_optional(db).await?;
    Ok(r.and_then(|r| serde_json::from_str(&r.get::<String, _>("config")).ok()))
}

/// All registry rows in the `GET /v1/connectors` shape (unfiltered; used by the compiler for typing): the org's, and the
/// deployment's (`connections::Deployment`), whose names no org row takes.
pub async fn registry_rows(st: &AppState) -> Result<Vec<Value>, ApiError> {
    let rows = sqlx::query("SELECT name, kind, config, description, roles, tools, tools_hash, captured_at, declared_by, declared_at FROM connectors ORDER BY name").fetch_all(&st.db).await?;
    let mut out: Vec<Value> = rows.iter().map(|r| connector_row(r, true)).filter(|c| !st.deployment.reserves(c["name"].as_str().unwrap_or(""))).collect();
    out.extend(st.deployment.rows());
    out.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    Ok(out)
}

/// The deployment's connectors are its own: no org declares, refreshes or removes one.
fn not_the_deployments(st: &AppState, name: &str, what: &str) -> Result<(), ApiError> {
    if st.deployment.reserves(name) { return Err(ApiError(StatusCode::CONFLICT, format!("`{name}` is a connector of this server's deployment: {what} is its operators'"))); }
    Ok(())
}

/// A builder asks an admin to declare a server; lands in the inbox (`GET /v1/connector-requests`, console). Its own
/// path, so that no connector name (`requests`) is shadowed under `/v1/connectors/{name}`.
pub(crate) async fn request_connector(InRealm(st): InRealm, Caller(who): Caller, Json(body): Json<Value>) -> Res {
    let url = body.get("url").and_then(|u| u.as_str()).filter(|u| !u.is_empty()).ok_or(ApiError(StatusCode::BAD_REQUEST, "url is required".into()))?;
    Ok((StatusCode::CREATED, Json(queue_connector_request(&st, &who, url, body.get("reason").and_then(|r| r.as_str()).unwrap_or("")).await?)).into_response())
}

/// Files a request for a connector in the admins' inbox, audited: the HTTP route and the MCP `request_connector`.
pub(crate) async fn queue_connector_request(st: &AppState, who: &Principal, url: &str, reason: &str) -> Result<Value, ApiError> {
    let id = uuid::Uuid::new_v4().to_string();
    sqlx::query("INSERT INTO connector_requests (id, url, reason, requested_by, status, at) VALUES ($1, $2, $3, $4, 'open', $5)").bind(&id).bind(url).bind(reason).bind(&who.sub).bind(chrono::Utc::now().to_rfc3339()).execute(&st.db).await?;
    db::audit(&st.db, &who.sub, "connector.request", url, json!({ "id": id })).await?;
    Ok(json!({ "queued": true, "id": id, "hint": format!("an admin declares it with `cori connectors add {url} --name <name>`") }))
}

pub(crate) async fn list_connector_requests(InRealm(st): InRealm, Caller(who): Caller) -> Res {
    require_power(&st, &who, &[Power::Admin], "the connector request inbox")?;
    let rows = sqlx::query("SELECT id, url, reason, requested_by, status, at FROM connector_requests WHERE status = 'open' ORDER BY at").fetch_all(&st.db).await?;
    Ok(Json(rows.iter().map(|r| json!({ "id": r.get::<String, _>("id"), "url": r.get::<String, _>("url"), "reason": r.get::<String, _>("reason"), "requested_by": r.get::<String, _>("requested_by"), "status": r.get::<String, _>("status"), "at": r.get::<String, _>("at") })).collect::<Vec<_>>()).into_response())
}

pub(crate) fn connector_row(r: &sqlx::postgres::PgRow, with_config: bool) -> Value {
    let cfg: Value = serde_json::from_str(&r.get::<String, _>("config")).unwrap_or(Value::Null);
    let mut v = json!({ "name": r.get::<String, _>("name"), "kind": r.get::<String, _>("kind"), "description": r.get::<String, _>("description"), "roles": serde_json::from_str::<Value>(&r.get::<String, _>("roles")).unwrap_or(json!([])), "tools": serde_json::from_str::<Value>(&r.get::<String, _>("tools")).unwrap_or(json!([])), "tools_hash": r.get::<String, _>("tools_hash"), "captured_at": r.get::<Option<String>, _>("captured_at"), "declared_by": r.get::<String, _>("declared_by"), "declared_at": r.get::<String, _>("declared_at") });
    // transport is public (url or command); the secret name is shown, never a value
    v["transport"] = json!({ "url": cfg["url"], "command": cfg["command"], "base_url": cfg["base_url"], "task_queue": cfg["task_queue"] });
    v["audience"] = cfg["audience"].clone(); v["secret"] = cfg["secret"].clone(); v["worker"] = cfg["worker"].clone();
    v["oauth"] = Value::Null; v["provided"] = json!("organization");
    if with_config { v["config"] = cfg; }
    v
}

pub(crate) async fn list_connectors(InRealm(st): InRealm, Caller(who): Caller) -> Res {
    Ok(Json(visible_connectors(&st, &who).await?).into_response())
}

/// The registry rows `who` may see, full tool schemas included and never a config: a connector with a non-empty roles
/// list is visible to those roles (and admins) only; an agent session sees what its human sees. `GET /v1/connectors` and
/// the MCP discovery tools (`list_connectors`, `find_tool`, `scaffold`).
pub(crate) async fn visible_connectors(st: &AppState, who: &Principal) -> Result<Vec<Value>, ApiError> {
    let admin = who.holds_role(&crate::auth::platform_roles().admin);
    Ok(registry_rows(st).await?.into_iter().map(|mut c| { if let Some(o) = c.as_object_mut() { o.remove("config"); } c })
        .filter(|c| { let roles = c["roles"].as_array().cloned().unwrap_or_default(); admin || roles.is_empty() || roles.iter().any(|x| x.as_str().map(|x| who.roles.iter().any(|y| y == x)).unwrap_or(false)) }).collect())
}

/// `tools/list` on a registry MCP server over HTTP, with its broker secret (the server's own environment holds no
/// connector credential), to an address the registry's egress allows. The server never starts a program: the tools of
/// an MCP server over stdio come with its declaration (`cori connectors add --command` lists them where the command runs).
pub(crate) async fn snapshot_tools(st: &AppState, name: &str, cfg: &cori_engine::config::ConnectorConfig) -> Result<Vec<Value>, ApiError> {
    if cfg.kind != "mcp" || cfg.url.is_none() { return Ok(vec![]); }
    let rc = cori_engine::config::ResolvedConnector { name: name.to_string(), kind: cfg.kind.clone(), base_url: None, auth_env: None, task_queue: None, url: cfg.url.clone(), command: None, worker: None, secret: None, audience: None, mock: None, egress: st.cfg.registry_egress(), oauth: None, app_account: false };
    let bearer = match &cfg.secret { Some(n) => st.broker.reveal(&st.db, n).await.ok(), None => None };
    cori_engine::mcp::list_tools(&rc, std::path::Path::new("."), bearer.as_deref(), rc.egress).await.map_err(|e| ApiError(StatusCode::BAD_GATEWAY, format!("tools/list on `{name}` failed: {e}")))
}

/// Whether a connector is an MCP server the server reaches over HTTP (it lists its tools) rather than one started over stdio.
fn mcp_over_http(cfg: &cori_engine::config::ConnectorConfig) -> bool { cfg.kind == "mcp" && cfg.url.is_some() }

pub(crate) fn tools_hash(tools: &[Value]) -> String { use sha2::Digest; format!("sha256:{}", hex::encode(sha2::Sha256::digest(serde_json::to_vec(tools).unwrap_or_default())))[..23].to_string() }

pub(crate) async fn declare_connector(InRealm(st): InRealm, Caller(who): Caller, Json(body): Json<Value>) -> Res {
    require_human(&who, "declaring a connector")?;
    require_power(&st, &who, &[Power::Admin], "declaring a connector")?;
    let name = body.get("name").and_then(|n| n.as_str()).filter(|n| !n.is_empty() && n.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-')).ok_or(ApiError(StatusCode::BAD_REQUEST, "name is required (letters, digits, _ -)".into()))?.to_string();
    not_the_deployments(&st, &name, "declaring it")?;
    let cfg: cori_engine::config::ConnectorConfig = serde_json::from_value(json!({ "kind": body.get("kind").cloned().unwrap_or(json!("mcp")), "ref": body.get("ref"), "audience": body.get("audience"), "worker": body.get("worker"), "base_url": body.get("base_url"), "auth_env": body.get("auth_env"), "secret": body.get("secret"), "task_queue": body.get("task_queue"), "url": body.get("url"), "command": body.get("command") })).map_err(|e| ApiError(StatusCode::BAD_REQUEST, format!("bad connector: {e}")))?;
    if cfg.kind == "mcp" && cfg.url.is_none() && cfg.command.is_none() { return Err(ApiError(StatusCode::BAD_REQUEST, "an mcp connector needs `url` (streamable HTTP) or `command` (stdio argv)".into())); }
    if cfg.kind == "activity" && cfg.task_queue.is_none() { return Err(ApiError(StatusCode::BAD_REQUEST, "an activity connector needs `task_queue`".into())); }
    // MCP servers over HTTP are introspected; other connectors (a stdio MCP server, native activities, HTTP APIs) bring
    // their declarations in `tools`
    let tools = if mcp_over_http(&cfg) { snapshot_tools(&st, &name, &cfg).await? } else { body.get("tools").and_then(|t| t.as_array()).cloned().unwrap_or_default() };
    let roles = body.get("roles").cloned().unwrap_or(json!([]));
    let description = body.get("description").and_then(|d| d.as_str()).unwrap_or("").to_string();
    let now = chrono::Utc::now().to_rfc3339();
    sqlx::query("INSERT INTO connectors (name, kind, config, description, roles, tools, tools_hash, captured_at, declared_by, declared_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10) ON CONFLICT ON CONSTRAINT connectors_pkey DO UPDATE SET kind = $2, config = $3, description = $4, roles = $5, tools = $6, tools_hash = $7, captured_at = $8, declared_by = $9, declared_at = $10")
        .bind(&name).bind(&cfg.kind).bind(serde_json::to_string(&cfg).unwrap()).bind(&description).bind(roles.to_string()).bind(serde_json::to_string(&tools).unwrap()).bind(tools_hash(&tools)).bind(if tools.is_empty() { None } else { Some(now.clone()) }).bind(&who.sub).bind(&now).execute(&st.db).await?;
    db::audit(&st.db, &who.sub, "connector.declare", &name, json!({ "kind": cfg.kind, "tools": tools.len() })).await?;
    if let Some(u) = cfg.url.as_deref() { sqlx::query("UPDATE connector_requests SET status = 'declared' WHERE url = $1 AND status = 'open'").bind(u).execute(&st.db).await.ok(); }
    Ok((StatusCode::CREATED, Json(json!({ "name": name, "kind": cfg.kind, "tools": tools.iter().filter_map(|t| t["name"].as_str()).collect::<Vec<_>>(), "tools_hash": tools_hash(&tools) }))).into_response())
}

/// `POST /v1/connectors/{name}/refresh`: captures the tools again. An MCP server over HTTP is asked; one over stdio sends
/// them in the body (`{ tools }`: `cori connectors refresh` lists them where the command runs), since the server never
/// starts a program.
/// `GET /v1/connectors/{name}/key`: the key of a native-activity connector's own worker (`CORI_CONNECTOR_KEY`), which opens
/// the run's token sealed into each activity's envelope (`secrets::Broker::seal_for_connector`). Admins, in person.
pub(crate) async fn connector_key(InRealm(st): InRealm, Path(name): Path<String>, Caller(who): Caller) -> Res {
    require_human(&who, "a connector's key")?;
    require_power(&st, &who, &[Power::Admin], "a connector's key")?;
    let cfg = registry_connector(&st.db, &name).await?.ok_or(ApiError(StatusCode::NOT_FOUND, format!("unknown connector `{name}`")))?;
    if cfg.kind != "activity" { return Err(ApiError(StatusCode::CONFLICT, format!("`{name}` is a `{}` connector: only a native-activity connector's worker gets a key", cfg.kind))); }
    db::audit(&st.db, &who.sub, "connector.key", &name, json!({})).await?;
    Ok(Json(json!({ "connector": name, "key": hex::encode(st.broker.connector_key(&name)), "env": "CORI_CONNECTOR_KEY" })).into_response())
}

pub(crate) async fn refresh_connector(InRealm(st): InRealm, Path(name): Path<String>, Caller(who): Caller, body: Option<Json<Value>>) -> Res {
    require_human(&who, "refreshing a connector")?;
    require_power(&st, &who, &[Power::Admin], "refreshing a connector")?;
    not_the_deployments(&st, &name, "reading its tools again (at each start)")?;
    let cfg = registry_connector(&st.db, &name).await?.ok_or(ApiError(StatusCode::NOT_FOUND, format!("unknown connector `{name}`")))?;
    let old: String = sqlx::query_scalar("SELECT tools_hash FROM connectors WHERE name = $1").bind(&name).fetch_one(&st.db).await?;
    let sent = body.as_ref().and_then(|b| b.get("tools")).and_then(|t| t.as_array()).cloned();
    let tools = if mcp_over_http(&cfg) { snapshot_tools(&st, &name, &cfg).await? }
        else if let Some(t) = sent { t }
        else if cfg.kind == "mcp" { return Err(ApiError(StatusCode::CONFLICT, format!("`{name}` is an MCP server over stdio and the server does not start programs: `cori connectors refresh {name}` lists its tools where its command runs"))); }
        else { serde_json::from_str::<Vec<Value>>(&sqlx::query_scalar::<_, String>("SELECT tools FROM connectors WHERE name = $1").bind(&name).fetch_one(&st.db).await?).unwrap_or_default() };
    let h = tools_hash(&tools);
    sqlx::query("UPDATE connectors SET tools = $1, tools_hash = $2, captured_at = $3 WHERE name = $4").bind(serde_json::to_string(&tools).unwrap()).bind(&h).bind(chrono::Utc::now().to_rfc3339()).bind(&name).execute(&st.db).await?;
    if h != old { db::audit(&st.db, &who.sub, "connector.schema_changed", &name, json!({ "from": old, "to": h })).await?; }
    Ok(Json(json!({ "name": name, "changed": h != old, "tools": tools.iter().filter_map(|t| t["name"].as_str()).collect::<Vec<_>>(), "tools_hash": h })).into_response())
}

pub(crate) async fn delete_connector(InRealm(st): InRealm, Path(name): Path<String>, Caller(who): Caller) -> Res {
    require_human(&who, "removing a connector")?;
    require_power(&st, &who, &[Power::Admin], "removing a connector")?;
    not_the_deployments(&st, &name, "removing it")?;
    let n = sqlx::query("DELETE FROM connectors WHERE name = $1").bind(&name).execute(&st.db).await?.rows_affected();
    if n == 0 { return Err(ApiError(StatusCode::NOT_FOUND, format!("unknown connector `{name}`"))); }
    db::audit(&st.db, &who.sub, "connector.remove", &name, json!({})).await?;
    Ok(Json(json!({ "removed": name })).into_response())
}

/// Stable JSON for hashing (sorted keys; serde_json maps are already ordered).
pub(crate) fn canonical(v: &Value) -> String { v.to_string() }
