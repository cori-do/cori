// SPDX-License-Identifier: Apache-2.0
//! What the identity provider was told (`crate::client_hook`): every app's client and roles as it knows them (the catalog
//! its IAM reconciles from), the events delivered or waiting, and a given-up event sent again.
use super::*;

/// Every app (or `only` one), its sign-in client and where it stands (`ready`, `pending` while the organization creates it,
/// `none`), and its roles as the identity provider was told of them (pending, confirmed, deleting; since when no kept
/// version declares one).
async fn catalog_of(st: &AppState, only: Option<&str>) -> Result<Vec<Value>, ApiError> {
    let only = only.unwrap_or_default();
    let apps = sqlx::query("SELECT id, name, owner, client_id, client_status, client_note, client_origin FROM apps WHERE ($1 = '' OR id = $1) ORDER BY id").bind(only).fetch_all(&st.db).await?;
    let roles = sqlx::query("SELECT app, role, description, note, state, declared_at, undeclared_since, confirmed_at FROM app_roles WHERE ($1 = '' OR app = $1) ORDER BY app, role").bind(only).fetch_all(&st.db).await?;
    Ok(apps.iter().map(|a| {
        let id: String = a.get("id");
        let status = match (a.get::<Option<String>, _>("client_id"), a.get::<Option<String>, _>("client_status")) { (Some(_), _) => "ready", (None, Some(s)) if s == "pending" => "pending", _ => "none" };
        json!({ "app": id, "name": a.get::<Option<String>, _>("name"), "owner": a.get::<Option<String>, _>("owner"), "client_id": a.get::<Option<String>, _>("client_id"), "client_status": status, "client_note": a.get::<Option<String>, _>("client_note"), "origin": a.get::<Option<String>, _>("client_origin").unwrap_or_else(|| st.front_origin(&id)),
                "roles": roles.iter().filter(|r| r.get::<String, _>("app") == id).map(|r| json!({ "role": r.get::<String, _>("role"), "description": r.get::<Option<String>, _>("description"), "note": r.get::<Option<String>, _>("note"), "state": r.get::<String, _>("state"), "declared_at": r.get::<String, _>("declared_at"), "undeclared_since": r.get::<Option<String>, _>("undeclared_since"), "confirmed_at": r.get::<Option<String>, _>("confirmed_at") })).collect::<Vec<_>>() })
    }).collect())
}

/// What the deployment tells the identity provider, besides apps: whether there is a hook, whether roles go to it, and when.
fn hook_settings(st: &AppState) -> Value {
    json!({ "hook": crate::client_hook::hook_url(st).is_some(), "role_events": crate::client_hook::role_events(st), "roles_at": if st.cfg.identity.roles_at_promote { "promote" } else { "deploy" } })
}

/// `GET /v1/identity/catalog` (`cori identity catalog`): every app's client and roles as the identity provider was told of
/// them, the catalog its IAM reconciles from. Admins, operators and auditors.
pub(crate) async fn identity_catalog(InRealm(st): InRealm, Caller(who): Caller) -> Res {
    require_power(&st, &who, &[Power::Operator, Power::Auditor], "the identity catalog")?;
    let mut out = hook_settings(&st);
    out["apps"] = json!(catalog_of(&st, None).await?);
    Ok(Json(out).into_response())
}

/// `GET /v1/apps/{app}/identity`: the app's entry in the catalog (its client, its roles) and its last identity events, for
/// the console's app page. Its owner and builders, admins, operators and auditors.
pub(crate) async fn app_identity(InRealm(st): InRealm, Path(app): Path<String>, Caller(who): Caller) -> Res {
    let rec = require_app(&st, &app).await?;
    if !(is_owner_or_builder(&who, &rec) || (!who.is_agent() && (who.is_operator() || who.is_auditor()))) {
        return Err(ApiError(StatusCode::FORBIDDEN, format!("`{app}`'s sign-in client and roles are for its owner and builders, operators and auditors")));
    }
    let mut out = hook_settings(&st);
    if let Some(Value::Object(entry)) = catalog_of(&st, Some(&app)).await?.into_iter().next() { for (k, v) in entry { out[k] = v; } }
    out["events"] = json!(events_of(&st, &app, "", 20).await?);
    // where the identity provider grants the app's roles, when it does so outside Cori: none when its owner gives them here
    let given_here = crate::client_hook::handled(&st).await.handles("access.granted");
    out["admin_url"] = json!(if given_here { None } else { admin_url_for(&st, &app, out["client_id"].as_str()) });
    out["may_sync"] = json!(!who.is_agent() && (who.is_admin() || rec.owner.as_deref() == Some(who.sub.as_str())));
    Ok(Json(out).into_response())
}

/// The events of the identity hook (of `app`, of `status`, or all), newest first, with the receiver's last answer.
async fn events_of(st: &AppState, app: &str, status: &str, limit: i64) -> Result<Vec<Value>, ApiError> {
    let rows = sqlx::query("SELECT id, seq, app, event, body, status, attempts, next_at, occurred_at, delivered_at, last_status, last_answer FROM identity_events WHERE ($1 = '' OR app = $1) AND ($2 = '' OR status = $2) ORDER BY seq DESC LIMIT $3")
        .bind(app).bind(status).bind(limit).fetch_all(&st.db).await?;
    Ok(rows.iter().map(|r| json!({ "id": r.get::<String, _>("id"), "seq": r.get::<i64, _>("seq"), "app": r.get::<String, _>("app"), "event": r.get::<String, _>("event"), "body": serde_json::from_str::<Value>(&r.get::<String, _>("body")).unwrap_or(Value::Null),
        "status": r.get::<String, _>("status"), "attempts": r.get::<i64, _>("attempts"), "next_at": r.get::<String, _>("next_at"), "occurred_at": r.get::<String, _>("occurred_at"), "delivered_at": r.get::<Option<String>, _>("delivered_at"),
        "last_status": r.get::<Option<i64>, _>("last_status"), "last_answer": r.get::<Option<String>, _>("last_answer") })).collect())
}

/// `GET /v1/identity/events[?app=&status=&limit=]` (`cori identity events`): the events of the identity hook, newest first,
/// with their attempts and the receiver's last answer. Admins, operators and auditors.
pub(crate) async fn identity_events(InRealm(st): InRealm, Caller(who): Caller, axum::extract::Query(q): axum::extract::Query<BTreeMap<String, String>>) -> Res {
    require_power(&st, &who, &[Power::Operator, Power::Auditor], "the identity events")?;
    let limit: i64 = q.get("limit").and_then(|l| l.parse().ok()).unwrap_or(100).clamp(1, 1000);
    let (app, status) = (q.get("app").cloned().unwrap_or_default(), q.get("status").cloned().unwrap_or_default());
    Ok(Json(events_of(&st, &app, &status, limit).await?).into_response())
}

/// `POST /v1/identity/events/{id}/retry` (`cori identity retry <id>`): a given-up (`dead`) or waiting event delivered again
/// now. Admins.
pub(crate) async fn retry_identity_event(InRealm(st): InRealm, Path(id): Path<String>, Caller(who): Caller) -> Res {
    require_human(&who, "retrying an identity event")?;
    require_power(&st, &who, &[Power::Admin], "retrying an identity event")?;
    let app: Option<String> = sqlx::query_scalar("UPDATE identity_events SET status = 'queued', next_at = $1, occurred_at = CASE WHEN status = 'dead' THEN $1 ELSE occurred_at END WHERE id = $2 AND status IN ('dead', 'queued') RETURNING app")
        .bind(chrono::Utc::now().to_rfc3339()).bind(&id).fetch_optional(&st.db).await?;
    let Some(app) = app else { return Err(ApiError(StatusCode::NOT_FOUND, format!("no event `{id}` given up or waiting"))) };
    db::audit(&st.db, &who.sub, "identity.event.retry", &app, json!({ "id": id })).await?;
    let delivered = crate::client_hook::deliver_app(&st, &app).await.map_err(|e| ApiError(StatusCode::BAD_GATEWAY, format!("{e:#}")))?;
    let status: String = sqlx::query_scalar("SELECT status FROM identity_events WHERE id = $1").bind(&id).fetch_one(&st.db).await?;
    Ok(Json(json!({ "id": id, "app": app, "status": status, "delivered": delivered })).into_response())
}

/// The identity provider's page for `app`'s roles (`CORI_IDENTITY_ADMIN_URL`, its `{app}` and `{client_id}` filled in,
/// encoded); None without one, or when it names the client and the app has none yet.
pub(crate) fn admin_url_for(st: &AppState, app: &str, client_id: Option<&str>) -> Option<String> {
    let template = st.cfg.identity_admin_url.as_deref()?;
    if template.contains("{client_id}") && client_id.is_none() { return None; }
    let enc = |s: &str| s.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect::<String>();
    Some(template.replace("{app}", &enc(app)).replace("{client_id}", &enc(client_id.unwrap_or_default())))
}

/// `POST /v1/identity/notify`: the identity provider's receiver telling Cori what changed on its side, signed as the hook's
/// deliveries are (`x-cori-signature` with `CORI_AUTH_CLIENT_HOOK_SECRET`), fresh (`at` within 5 minutes), each `id` handled
/// once, naming its `realm` on a server of several organizations. Served on the internal listener only when there is one
/// (`CORI_WORKER_LISTEN`).
/// - `request.settled` `{ request, outcome: done | refused, message?, result? }`: a request the receiver answered `202`
///   with `settles: true` for (its IAM's own workflow) is done, or refused;
/// - `person.removed` `{ person }`, `person.deleted` `{ person }`: someone left the organization, or their account is gone.
///
/// The access webhook (`CORI_IDENTITY_ACCESS_URL`) signs with its own secret (`CORI_IDENTITY_ACCESS_SECRET`), which settles
/// the access changes it was sent and nothing else.
pub(crate) async fn identity_notify(InRealm(st): InRealm, via: crate::workers::Via, headers: HeaderMap, body: Bytes) -> Res {
    if st.cfg.worker_listen.is_some() && via.is_none() { return Err(ApiError(StatusCode::NOT_FOUND, "not served here".into())); }
    let access_secret = st.cfg.access_secret.as_deref().filter(|_| st.cfg.access_url.is_some());
    if st.cfg.client_hook_secret.is_none() && access_secret.is_none() { return Err(ApiError(StatusCode::UNAUTHORIZED, "notifications are signed with CORI_AUTH_CLIENT_HOOK_SECRET, which this server does not have".into())); }
    let text = std::str::from_utf8(&body).map_err(|_| ApiError(StatusCode::BAD_REQUEST, "the body is not text".into()))?;
    let signature = headers.get("x-cori-signature").and_then(|v| v.to_str().ok()).unwrap_or("");
    let by_receiver = st.cfg.client_hook_secret.as_deref().is_some_and(|s| crate::client_hook::verify(s, &body, signature));
    if !by_receiver && !access_secret.is_some_and(|s| crate::client_hook::verify(s, &body, signature)) { return Err(ApiError(StatusCode::UNAUTHORIZED, "bad signature".into())); }
    let b: Value = serde_json::from_str(text).map_err(|_| ApiError(StatusCode::BAD_REQUEST, "the body is not JSON".into()))?;
    // the access webhook's secret settles the access changes it was sent, nothing else
    if !by_receiver && b["event"].as_str() != Some("request.settled") { return Err(ApiError(StatusCode::FORBIDDEN, "the access webhook's secret (CORI_IDENTITY_ACCESS_SECRET) settles the access changes it was sent, nothing else".into())); }
    let fresh = chrono::DateTime::parse_from_rfc3339(b["at"].as_str().unwrap_or("")).is_ok_and(|at| chrono::Utc::now().signed_duration_since(at).num_seconds().abs() < 300);
    if !fresh { return Err(ApiError(StatusCode::BAD_REQUEST, "stale notification (`at`)".into())); }
    let (id, event) = (b["id"].as_str().unwrap_or_default().to_string(), b["event"].as_str().unwrap_or_default().to_string());
    if id.is_empty() || event.is_empty() { return Err(ApiError(StatusCode::BAD_REQUEST, "`id` and `event` are required".into())); }
    // the organization it is about, on a server of several
    let st = match (&st.tenancy, b["realm"].as_str()) {
        (Some(_), Some(r)) => st.in_realm_unadmitted(crate::realm::Realm::parse(r).ok_or_else(|| ApiError(StatusCode::BAD_REQUEST, "`realm` names no organization".into()))?),
        (Some(_), None) => return Err(ApiError(StatusCode::BAD_REQUEST, "`realm`: this server hosts several organizations".into())),
        (None, _) => st,
    };
    let now = chrono::Utc::now().to_rfc3339();
    let fresh_id = sqlx::query("INSERT INTO identity_notifications (id, event, at) VALUES ($1, $2, $3) ON CONFLICT ON CONSTRAINT identity_notifications_pkey DO NOTHING").bind(&id).bind(&event).bind(&now).execute(&st.db).await?.rows_affected() > 0;
    if !fresh_id { return Ok(Json(json!({ "duplicate": id })).into_response()); }
    match event.as_str() {
        "request.settled" => {
            let request = b["request"].as_str().unwrap_or_default();
            let outcome = match b["outcome"].as_str() { Some("done") => "done", Some("refused") => "refused", _ => return Err(ApiError(StatusCode::BAD_REQUEST, "`outcome`: done or refused".into())) };
            // signed by the access webhook: an access change only
            let events: Vec<String> = if by_receiver { vec![] } else { crate::client_hook::ACCESS_EVENTS.iter().map(|e| e.to_string()).collect() };
            let n = sqlx::query("UPDATE access_requests SET status = $1, answer = $2, result = COALESCE($3, result), updated_at = $4 WHERE id = $5 AND status IN ('sent', 'pending') AND (cardinality($6::text[]) = 0 OR event = ANY($6))")
                .bind(outcome).bind(b["message"].as_str().unwrap_or("")).bind(b.get("result").map(|r| r.to_string())).bind(&now).bind(request).bind(&events).execute(&st.db).await?.rows_affected();
            if n == 0 { return Err(ApiError(StatusCode::NOT_FOUND, format!("no request `{request}` waiting for its outcome"))); }
            crate::client_hook::forget_answers(&st);
            db::audit(&st.db, "identity", "people.settled", request, json!({ "outcome": outcome, "message": b["message"], "from": if by_receiver { "receiver" } else { "access webhook" } })).await?;
            crate::api::request_settled(&st, request).await.map_err(ApiError::from)?;
            Ok(Json(json!({ "request": request, "status": outcome })).into_response())
        }
        "person.removed" | "person.deleted" => {
            let person = b["person"].as_str().unwrap_or_default();
            if person.is_empty() { return Err(ApiError(StatusCode::BAD_REQUEST, "`person`".into())); }
            crate::client_hook::forget_answers(&st);
            db::audit(&st.db, "identity", &event, person, json!({ "id": id })).await?;
            // Cori's part: nothing moves ownership by itself (their apps show an owner who left, for an admin to reassign)
            crate::api::person_left(&st, person, &std::collections::BTreeMap::new(), "identity", if event == "person.deleted" { "deleted" } else { "left elsewhere" }).await.map_err(ApiError::from)?;
            // a deleted account's name and address go with it; runs and the audit keep its id
            if event == "person.deleted" { sqlx::query("DELETE FROM people_seen WHERE sub = $1").bind(person).execute(&st.db).await?; }
            Ok(Json(json!({ "person": person, "event": event })).into_response())
        }
        _ => Ok(Json(json!({ "ignored": event })).into_response()),
    }
}
