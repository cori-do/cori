// SPDX-License-Identifier: Apache-2.0
//! Server checks (`doctor`), the audit log and webhook deliveries.
use super::*;

/// Server-side health checks for `cori doctor` (admin roles only).
pub(crate) async fn doctor(InRealm(st): InRealm, Caller(who): Caller) -> Res {
    require_power(&st, &who, &[Power::Admin], "the server checks")?;
    let mut checks = Vec::new();
    let db = sqlx::query("SELECT count(*) AS n FROM apps").fetch_one(&st.db).await;
    checks.push(json!({ "name": "database", "ok": db.is_ok(), "detail": match &db { Ok(r) => format!("postgres · {} apps", r.get::<i64, _>("n")), Err(e) => e.to_string() } }));
    let addr = st.cfg.temporal.trim_start_matches("http://").trim_start_matches("https://").trim_end_matches('/').to_string();
    let t = tokio::time::timeout(std::time::Duration::from_secs(3), tokio::net::TcpStream::connect(addr)).await;
    checks.push(json!({ "name": "temporal", "ok": matches!(t, Ok(Ok(_))), "detail": format!("{} · namespace {} · queue {}{}", st.cfg.temporal, st.cfg.namespace, st.cfg.task_queue, if st.cfg.shards > 1 { format!(" × {} shards", st.cfg.shards) } else { String::new() }) }));
    let clusters = st.cfg.store_clusters();
    let store = if clusters.is_empty() { json!({ "name": "store database", "ok": true, "detail": "not configured: store steps use dev mocks" }) } else {
        let r = crate::stores::ping(&st).await;
        let at = clusters.iter().map(|(n, u)| format!("{n}: {}", u.rsplit('@').next().unwrap_or("postgres"))).collect::<Vec<_>>().join(", ");
        json!({ "name": "store database", "ok": r.is_ok(), "detail": r.map(|_| format!("{at} (credentials hidden); new databases on `{}`", st.cfg.store_placement)).unwrap_or_else(|e| e.to_string()) })
    };
    checks.push(store);
    checks.push(json!({ "name": "bundles", "ok": true, "detail": st.bundles.remote().map(|r| format!("{r} (cached in the data dir)")).unwrap_or_else(|| "the data dir (one replica)".into()) }));
    checks.push(json!({ "name": "files", "ok": st.files.unavailable().is_none(), "detail": match (st.files.remote(), st.files.unavailable()) {
        (Some(r), _) => r.to_string(),
        (None, Some(why)) => why.to_string(),
        (None, None) => "the data dir (one replica)".to_string(),
    } }));
    checks.push(json!({ "name": "auth", "ok": true, "detail": format!("{:?}{}", st.cfg.auth, if st.cfg.dev_mocks { " · dev mocks on" } else { "" }).to_lowercase() }));
    checks.extend(identity_checks(&st, &who).await?);
    checks.push(json!({ "name": "secrets", "ok": st.broker.ready(), "detail": if st.broker.ready() { "master key loaded" } else { "no master key (CORI_MASTER_KEY or <data_dir>/master.key)" } }));
    checks.push(json!({ "name": "worker ingest", "ok": true, "detail": if st.cfg.ingest_token.is_empty() { "open (local mode)" } else { "token required" } }));
    let heartbeat = sqlx::query("SELECT count(*) AS n FROM pools").fetch_one(&st.db).await.map(|r| r.get::<i64, _>("n")).unwrap_or(0);
    checks.push(json!({ "name": "pools", "ok": true, "detail": format!("{heartbeat} named pools") }));
    Ok(Json(json!({ "version": env!("CARGO_PKG_VERSION"), "checks": checks })).into_response())
}

// ---------------------------------------------------------------------------
// Connector registry: MCP servers (and other connectors) declared ahead by an admin, with their tool
// declarations snapshotted so agents author against facts and workflows resolve them without a cori.toml block.
// ---------------------------------------------------------------------------

pub(crate) async fn webhook_deliveries(InRealm(st): InRealm, Path(app): Path<String>, Caller(who): Caller) -> Res {
    require_power(&st, &who, &[Power::Admin], "the webhook deliveries")?;
    let rows = sqlx::query("SELECT id, event, url, status, attempts, at, error FROM webhook_deliveries WHERE app = $1 ORDER BY at DESC LIMIT 200").bind(&app).fetch_all(&st.db).await?;
    Ok(Json(rows.iter().map(|r| json!({ "id": r.get::<String, _>("id"), "event": r.get::<String, _>("event"), "url": r.get::<String, _>("url"), "status": r.get::<i64, _>("status"), "attempts": r.get::<i64, _>("attempts"), "at": r.get::<String, _>("at"), "error": r.get::<Option<String>, _>("error") })).collect::<Vec<_>>()).into_response())
}

pub(crate) async fn list_audit(InRealm(st): InRealm, Caller(who): Caller) -> Res {
    require_power(&st, &who, &[Power::Auditor], "the audit log")?;
    let rows = sqlx::query("SELECT actor, action, object, at, details FROM audit ORDER BY at DESC LIMIT 300").fetch_all(&st.db).await?;
    Ok(Json(rows.iter().map(|r| json!({ "actor": r.get::<String, _>("actor"), "action": r.get::<String, _>("action"), "object": r.get::<String, _>("object"), "at": r.get::<String, _>("at"), "details": serde_json::from_str::<Value>(&r.get::<String, _>("details")).unwrap_or(Value::Null) })).collect::<Vec<_>>()).into_response())
}

/// The identity checks of `cori doctor`: discovery, JWKS, the caller's token, and one token exchange per connector audience
/// with the caller's token. Each failure comes with the Keycloak fix (the audience's client, its scope on the platform client).
/// None under dev and off auth, which have no issuer (the `auth` check names the mode).
async fn identity_checks(st: &AppState, who: &Principal) -> Result<Vec<Value>, ApiError> {
    let mut checks: Vec<Value> = Vec::new();
    let (issuer, client_id) = match &st.cfg.auth { crate::auth::AuthMode::Builtin { issuer, .. } => (issuer.clone(), st.cfg.auth_client_id.clone()), _ => return Ok(vec![]) };
    let http = cori_engine::http::builder().timeout(std::time::Duration::from_secs(5)).build().unwrap();
    let disc = http.get(format!("{}/.well-known/openid-configuration", issuer.trim_end_matches('/'))).send().await;
    let cfg: Value = match disc { Ok(r) if r.status().is_success() => r.json().await.unwrap_or(Value::Null), Ok(r) => { checks.push(json!({ "name": "discovery", "ok": false, "detail": format!("{issuer}: http {}", r.status()) })); Value::Null } Err(e) => { checks.push(json!({ "name": "discovery", "ok": false, "detail": format!("{issuer}: {e}") })); Value::Null } };
    if !cfg.is_null() { checks.push(json!({ "name": "discovery", "ok": true, "detail": format!("{issuer} · token_endpoint {}", cfg["token_endpoint"].as_str().unwrap_or("?")) })); }
    if let Some(j) = cfg["jwks_uri"].as_str() { let n = http.get(j).send().await.ok().and_then(|r| if r.status().is_success() { Some(r) } else { None }); match n { Some(r) => { let set: Value = r.json().await.unwrap_or(Value::Null); checks.push(json!({ "name": "jwks", "ok": true, "detail": format!("{} key(s)", set["keys"].as_array().map(|a| a.len()).unwrap_or(0)) })); } None => checks.push(json!({ "name": "jwks", "ok": false, "detail": format!("{j} unreachable") })) } }
    checks.push(json!({ "name": "your token", "ok": true, "detail": format!("{} · roles {:?} · exp {}", who.sub, who.roles, who.claims.get("exp").cloned().unwrap_or(Value::Null)) }));
    let Some(token) = who.token.clone() else { checks.push(json!({ "name": "token exchange", "ok": false, "detail": "no bearer to exchange" })); return Ok(checks); };
    let mut audiences: Vec<(String, String)> = Vec::new();
    for row in registry_rows(&st).await? { if let (Some(n), Some(a)) = (row["name"].as_str(), row["audience"].as_str()) { audiences.push((n.to_string(), a.to_string())); } }
    for r in sqlx::query("SELECT cori_toml FROM versions WHERE status = 'promoted'").fetch_all(&st.db).await? { if let Ok(c) = AppConfig::from_toml(&r.get::<String, _>("cori_toml"), std::path::Path::new(".")) { for (n, cc) in c.connectors { if let Some(a) = cc.audience { if !audiences.iter().any(|(_, x)| x == &a) { audiences.push((n, a)); } } } } }
    if audiences.is_empty() { checks.push(json!({ "name": "token exchange", "ok": true, "detail": "no connector declares an audience" })); }
    for (name, aud) in audiences {
        match st.auth.exchange(&token, &aud, &client_id).await {
            Ok(_) => checks.push(json!({ "name": format!("exchange → {name}"), "ok": true, "detail": format!("audience {aud}") })),
            Err(e) => checks.push(json!({ "name": format!("exchange → {name}"), "ok": false, "detail": format!("audience {aud}: {e}"), "fix": format!("Keycloak (26.2 or later, standard token exchange): client `{aud}` exists in the realm (bearer-only), and a client scope whose audience mapper names `{aud}` is a default client scope of `{client_id}`, which has standard token exchange on. An optional scope does not do: Keycloak offers an exchange the audiences of the default scopes only.") })),
        }
    }
    Ok(checks)
}
