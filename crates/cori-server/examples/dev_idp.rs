// SPDX-License-Identifier: Apache-2.0
//! A tiny OIDC provider for tests and demos (stands in for Keycloak): RS256 keys, discovery, JWKS, and the grants Cori
//! uses against Keycloak:
//! - `password` for users with roles (scripts);
//! - `authorization_code` with PKCE (S256), behind a user picker at `/auth` (the console's and the fronts' login);
//! - `refresh_token`;
//! - RFC 8693 token exchange that re-issues the token for another audience, or, with
//!   `requested_token_type=…:refresh_token`, returns a refresh token of the calling client (Keycloak ≥ 26.2 with
//!   "Allow refresh token in Standard Token Exchange");
//! - `client_credentials` for service accounts.
//!
//! Tokens carry `aud` = the client they were issued to plus `DEV_IDP_AUDIENCE` when set (an audience mapper), and the
//! user's roles as that client's roles (`resource_access.<client>.roles`, what the server reads as roles in an app).
//! `DEV_IDP_APP_ROLES_PREFIX` (e.g. `{client_id}:`) lists those instead in `roles`, after the member roles, each named with
//! the prefix (`support-desk:support_agent`), as Entra ID issues the app roles of one registration: the server then reads
//! them with `CORI_AUTH_APP_ROLES_CLAIM=roles` and the same `CORI_AUTH_APP_ROLES_PREFIX`.
//! `DEV_IDP_LIMITED_SCOPE` (client ids, comma-separated) names clients whose tokens carry their own client's roles only, as
//! Keycloak's "Full scope allowed" off leaves them: none of the apps' clients' roles.
//! `DEV_IDP_TTL` (seconds, default 3600) sets the access token lifetime, to test runs that outlive it.
//! It also answers the client hook `cori serve --auth builtin` requires (`CORI_AUTH_CLIENT_HOOK=http://127.0.0.1:8099/cori/clients`):
//! an app's client id is the app's name, and deliveries must be signed with `CORI_AUTH_CLIENT_HOOK_SECRET` when it is set.
//! With `DEV_IDP_PEOPLE` it also stands in for a receiver of people and access: `manage` answers
//! every people event and question at once, from an in-memory directory of its users whose member roles and app grants
//! the tokens it mints then carry; `requests` answers the events `202` and settles each `DEV_IDP_SETTLE_SECS` later by
//! notifying Cori (`DEV_IDP_NOTIFY_URL`: its `/v1/identity/notify`), as an IAM with its own approvals would; `read` answers
//! the questions only. Its rules: an admin (`it_admin`) manages people, a person must exist, and roles are not empty.
//! At `POST /people` it also stands in for a directory of the organization's own (`CORI_IDENTITY_PEOPLE_URL`): `people.list`
//! alone, two people a page, each named by id, address and the directory's name only (the server fills in the rest from
//! their tokens), signed with `DEV_IDP_DIRECTORY_SECRET` when it is set; `DEV_IDP_DIRECTORY_LEFT` names a file of the users
//! it no longer lists.
//! At `POST /access` it stands in for an access webhook of the organization's own (`CORI_IDENTITY_ACCESS_URL`):
//! `access.granted` and `access.revoked`, checked as the receiver checks them, then applied to the grants its tokens carry,
//! at once, or `DEV_IDP_ACCESS_SYNC_SECS` later as an IAM that syncs every so often (answered `202` with `settles`, then
//! `request.settled` to `DEV_IDP_NOTIFY_URL`); signed with `DEV_IDP_ACCESS_SECRET` when it is set, both ways.
//! `DEV_IDP_USER` signs that user in at `/auth` without the picker: a headless browser's sign-in.
//! Run: cargo run -p cori-server --example dev_idp   (listens on 127.0.0.1:8099, issuer http://127.0.0.1:8099/realms/acme;
//! `DEV_IDP_PORT` for another port)
use axum::{Form, Json, Router, body::Bytes, extract::{Query, State}, http::{HeaderMap, StatusCode}, response::{Html, IntoResponse, Redirect}, routing::{get, post}};
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation, jwk};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

struct Idp {
    issuer: String, kid: String, enc: EncodingKey, dec: DecodingKey, jwks: Value, ttl: i64, extra_aud: Option<String>,
    users: HashMap<&'static str, (&'static str, Vec<&'static str>)>,
    /// service accounts: client id → (secret, roles)
    clients: HashMap<&'static str, (&'static str, Vec<&'static str>)>,
    /// authorization codes: code → (user, client, redirect_uri, code_challenge)
    codes: Mutex<HashMap<String, (String, String, String, String)>>,
    /// the client hook's HMAC secret (`CORI_AUTH_CLIENT_HOOK_SECRET`); unset, unsigned deliveries are accepted
    hook_secret: Option<String>,
    /// tests: every delivery appended to this file as a JSON line (`DEV_IDP_HOOK_LOG`), the first n `app.created` of each app
    /// answered 202 (`DEV_IDP_HOOK_PENDING`: the client is made later), and while `DEV_IDP_HOOK_DOWN` names a file that
    /// exists, every delivery answered 503 (an outage)
    hook_log: Option<String>,
    hook_pending: u32,
    hook_down: Option<String>,
    pending_seen: Mutex<HashMap<String, u32>>,
    /// the people receiver (`DEV_IDP_PEOPLE`): manage, requests, read, or none
    people: People,
    /// the user `/auth` signs in without its picker (`DEV_IDP_USER`)
    auto_user: Option<String>,
    /// every client's roles listed in `roles`, each named with this prefix (`{client_id}` filled in), rather than in
    /// `resource_access` (`DEV_IDP_APP_ROLES_PREFIX`)
    app_roles_prefix: Option<String>,
    /// clients whose tokens carry none of the other clients' roles (`DEV_IDP_LIMITED_SCOPE`)
    limited_scope: Vec<String>,
    /// the directory of people's HMAC secret (`DEV_IDP_DIRECTORY_SECRET`), and the file of the users it no longer lists
    /// (`DEV_IDP_DIRECTORY_LEFT`)
    directory_secret: Option<String>,
    directory_left: Option<String>,
    /// the access webhook's HMAC secret (`DEV_IDP_ACCESS_SECRET`), and how long its IAM takes to apply a change
    /// (`DEV_IDP_ACCESS_SYNC_SECS`, 0: at once)
    access_secret: Option<String>,
    access_sync_secs: u64,
}

/// The directory the people receiver keeps: member roles over the users' own, people removed, app grants by (person,
/// client), invitations, the organization's name, and each request's answer (a request delivered twice is answered alike).
#[derive(Default)]
struct People {
    mode: String, notify: Option<String>, settle_secs: u64,
    roles: Mutex<HashMap<String, Vec<String>>>,
    removed: Mutex<std::collections::HashSet<String>>,
    grants: Mutex<HashMap<(String, String), Vec<String>>>,
    invitations: Mutex<Vec<Value>>,
    org: Mutex<(String, String)>,
    answered: Mutex<HashMap<String, (u16, Value)>>,
}

/// A user's member roles: the directory's, else their own.
fn member_roles(idp: &Idp, user: &str) -> Vec<String> {
    if let Some(r) = idp.people.roles.lock().unwrap().get(user) { return r.clone(); }
    idp.users.get(user).map(|u| u.1.iter().map(|r| r.to_string()).collect()).unwrap_or_default()
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct TokenReq {
    grant_type: String, username: String, password: String, client_id: String, client_secret: String,
    subject_token: String, audience: String, requested_token_type: String, refresh_token: String,
    code: String, redirect_uri: String, code_verifier: String, scope: String,
}

fn aud_claim(idp: &Idp, aud: &str) -> Value {
    match &idp.extra_aud { Some(x) if x != aud => json!([aud, x]), _ => json!(aud) }
}

fn mint(idp: &Idp, sub: &str, roles: &[&str], aud: &str, email: &str) -> String {
    let now = chrono::Utc::now().timestamp();
    // a person of the directory: its member roles, and its grants in every app's client
    let directory = !idp.people.mode.is_empty() && idp.users.contains_key(sub);
    let held: Vec<String> = if directory { member_roles(idp, sub) } else { roles.iter().map(|r| r.to_string()).collect() };
    let mut access = json!({ aud: { "roles": held } });
    // its grants in apps: the people receiver's, or the access webhook's
    let limited = idp.limited_scope.iter().any(|c| c == aud);
    if idp.users.contains_key(sub) { for ((who, client), granted) in idp.people.grants.lock().unwrap().iter() { if who == sub && !granted.is_empty() && (!limited || client == aud) { access[client.as_str()] = json!({ "roles": granted }); } } }
    let mut claims = json!({ "iss": idp.issuer, "sub": sub, "aud": aud_claim(idp, aud), "azp": aud, "exp": now + idp.ttl, "iat": now, "email": email, "name": sub, "roles": held, "realm_access": { "roles": held }, "resource_access": access });
    // one claim for every app's roles, after the member roles: `<prefix with the client><role>`
    if let Some(prefix) = &idp.app_roles_prefix {
        let listed = access.as_object().into_iter().flatten().flat_map(|(client, r)| r["roles"].as_array().into_iter().flatten().filter_map(|x| x.as_str()).map(move |role| format!("{}{role}", prefix.replace("{client_id}", client))));
        claims["roles"] = json!(held.iter().cloned().chain(listed).collect::<Vec<_>>());
        claims.as_object_mut().unwrap().remove("resource_access");
    }
    jsonwebtoken::encode(&Header { alg: Algorithm::RS256, kid: Some(idp.kid.clone()), ..Default::default() }, &claims, &idp.enc).unwrap()
}

/// A refresh token: signed like an access token, `typ: Refresh`, 1 day (offline: 30 days).
fn mint_refresh(idp: &Idp, sub: &str, roles: &[&str], client: &str, email: &str, offline: bool) -> String {
    let now = chrono::Utc::now().timestamp();
    let claims = json!({ "iss": idp.issuer, "sub": sub, "aud": idp.issuer, "azp": client, "typ": if offline { "Offline" } else { "Refresh" }, "exp": now + if offline { 30 * 86400 } else { 86400 }, "iat": now, "email": email, "roles": roles, "jti": uuid::Uuid::new_v4().to_string() });
    jsonwebtoken::encode(&Header { alg: Algorithm::RS256, kid: Some(idp.kid.clone()), ..Default::default() }, &claims, &idp.enc).unwrap()
}

fn verify(idp: &Idp, token: &str) -> Result<Value, String> {
    let mut v = Validation::new(Algorithm::RS256); v.set_issuer(&[idp.issuer.as_str()]); v.validate_aud = false;
    jsonwebtoken::decode::<Value>(token, &idp.dec, &v).map(|d| d.claims).map_err(|e| e.to_string())
}

/// The member roles a token was minted with: `realm_access.roles` (an access token's `roles` may list app roles too), else `roles`.
fn roles_of(c: &Value) -> Vec<&str> { c["realm_access"]["roles"].as_array().or(c["roles"].as_array()).map(|a| a.iter().filter_map(|r| r.as_str()).collect()).unwrap_or_default() }
fn err(code: StatusCode, e: &str, d: String) -> (StatusCode, Json<Value>) { (code, Json(json!({ "error": e, "error_description": d }))) }

async fn token(State(idp): State<Arc<Idp>>, Form(req): Form<TokenReq>) -> (StatusCode, Json<Value>) {
    let client = if req.client_id.is_empty() { "support-desk".to_string() } else { req.client_id.clone() };
    match req.grant_type.as_str() {
        "password" => match idp.users.get(req.username.as_str()) {
            Some((pw, roles)) if *pw == req.password => {
                let email = format!("{}@acme.example", req.username);
                (StatusCode::OK, Json(json!({ "access_token": mint(&idp, &req.username, roles, &client, &email), "refresh_token": mint_refresh(&idp, &req.username, roles, &client, &email, false), "token_type": "Bearer", "expires_in": idp.ttl })))
            }
            _ => err(StatusCode::UNAUTHORIZED, "invalid_grant", "bad credentials".into()),
        },
        "authorization_code" => {
            let Some((user, cid, redirect, challenge)) = idp.codes.lock().unwrap().remove(&req.code) else { return err(StatusCode::BAD_REQUEST, "invalid_grant", "unknown or used code".into()) };
            if cid != client || redirect != req.redirect_uri { return err(StatusCode::BAD_REQUEST, "invalid_grant", "client or redirect_uri mismatch".into()); }
            use sha2::Digest;
            let computed = base64::Engine::encode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, sha2::Sha256::digest(req.code_verifier.as_bytes()));
            if computed != challenge { return err(StatusCode::BAD_REQUEST, "invalid_grant", "PKCE verification failed".into()); }
            let roles = idp.users.get(user.as_str()).map(|u| u.1.clone()).unwrap_or_default();
            let email = format!("{user}@acme.example");
            (StatusCode::OK, Json(json!({ "access_token": mint(&idp, &user, &roles, &client, &email), "refresh_token": mint_refresh(&idp, &user, &roles, &client, &email, false), "id_token": mint(&idp, &user, &roles, &client, &email), "token_type": "Bearer", "expires_in": idp.ttl })))
        }
        "refresh_token" => match verify(&idp, &req.refresh_token) {
            Ok(c) if matches!(c["typ"].as_str(), Some("Refresh") | Some("Offline")) => {
                let (sub, email) = (c["sub"].as_str().unwrap_or(""), c["email"].as_str().unwrap_or(""));
                let aud = c["azp"].as_str().unwrap_or(&client).to_string();
                let offline = c["typ"] == "Offline";
                (StatusCode::OK, Json(json!({ "access_token": mint(&idp, sub, &roles_of(&c), &aud, email), "refresh_token": mint_refresh(&idp, sub, &roles_of(&c), &aud, email, offline), "token_type": "Bearer", "expires_in": idp.ttl })))
            }
            Ok(_) => err(StatusCode::BAD_REQUEST, "invalid_grant", "not a refresh token".into()),
            Err(e) => err(StatusCode::BAD_REQUEST, "invalid_grant", e),
        },
        "urn:ietf:params:oauth:grant-type:token-exchange" => match verify(&idp, &req.subject_token) {
            Ok(c) => {
                let (sub, email) = (c["sub"].as_str().unwrap_or(""), c["email"].as_str().unwrap_or(""));
                if req.requested_token_type == "urn:ietf:params:oauth:token-type:refresh_token" {
                    let offline = req.scope.split_whitespace().any(|s| s == "offline_access");
                    return (StatusCode::OK, Json(json!({ "access_token": mint(&idp, sub, &roles_of(&c), &client, email), "refresh_token": mint_refresh(&idp, sub, &roles_of(&c), &client, email, offline), "issued_token_type": req.requested_token_type, "token_type": "Bearer", "expires_in": idp.ttl })));
                }
                let aud = if req.audience.is_empty() { client.as_str() } else { req.audience.as_str() };
                (StatusCode::OK, Json(json!({ "access_token": mint(&idp, sub, &roles_of(&c), aud, email), "issued_token_type": "urn:ietf:params:oauth:token-type:access_token", "token_type": "Bearer" })))
            }
            Err(e) => err(StatusCode::BAD_REQUEST, "invalid_grant", e),
        },
        "client_credentials" => match idp.clients.get(req.client_id.as_str()) {
            Some((secret, roles)) if *secret == req.client_secret => {
                let sub = format!("service-account-{}", req.client_id);
                (StatusCode::OK, Json(json!({ "access_token": mint(&idp, &sub, roles, &req.client_id, ""), "token_type": "Bearer", "expires_in": idp.ttl })))
            }
            _ => err(StatusCode::UNAUTHORIZED, "invalid_client", "unknown client or bad secret".into()),
        },
        _ => err(StatusCode::BAD_REQUEST, "unsupported_grant_type", req.grant_type),
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct AuthReq { client_id: String, redirect_uri: String, state: String, code_challenge: String, code_challenge_method: String, response_type: String, user: String }

/// `/auth`: a user picker (no password: a dev provider), then a redirect with a code. PKCE S256 is required.
async fn authorize(State(idp): State<Arc<Idp>>, Query(mut q): Query<AuthReq>) -> axum::response::Response {
    if q.response_type != "code" || q.code_challenge_method != "S256" || q.code_challenge.is_empty() { return (StatusCode::BAD_REQUEST, "response_type=code and PKCE S256 are required").into_response(); }
    if q.user.is_empty() { q.user = idp.auto_user.clone().unwrap_or_default(); }
    if q.user.is_empty() {
        let esc = |s: &str| s.replace('&', "&amp;").replace('"', "&quot;").replace('<', "&lt;");
        let mut users: Vec<_> = idp.users.iter().collect(); users.sort_by_key(|u| *u.0);
        let hidden = format!("<input type=hidden name=client_id value=\"{}\"><input type=hidden name=redirect_uri value=\"{}\"><input type=hidden name=state value=\"{}\"><input type=hidden name=code_challenge value=\"{}\"><input type=hidden name=code_challenge_method value=S256><input type=hidden name=response_type value=code>", esc(&q.client_id), esc(&q.redirect_uri), esc(&q.state), esc(&q.code_challenge));
        let buttons: String = users.iter().map(|(u, (_, roles))| format!("<button name=user value=\"{u}\">{u} <small>{}</small></button>", roles.join(", "))).collect();
        return Html(format!("<!doctype html><title>dev IdP sign-in</title><style>body{{font:15px system-ui;max-width:420px;margin:60px auto}}button{{display:block;width:100%;margin:8px 0;padding:10px;text-align:left}}small{{color:#666}}</style><h2>Sign in to {}</h2><p>dev IdP: pick a user</p><form method=get action=\"/realms/acme/protocol/openid-connect/auth\">{hidden}{buttons}</form>", esc(&q.client_id))).into_response();
    }
    if !idp.users.contains_key(q.user.as_str()) { return (StatusCode::BAD_REQUEST, "unknown user").into_response(); }
    let code = uuid::Uuid::new_v4().simple().to_string();
    idp.codes.lock().unwrap().insert(code.clone(), (q.user.clone(), q.client_id.clone(), q.redirect_uri.clone(), q.code_challenge.clone()));
    let sep = if q.redirect_uri.contains('?') { '&' } else { '?' };
    Redirect::to(&format!("{}{sep}code={code}&state={}", q.redirect_uri, q.state)).into_response()
}

/// `POST /cori/clients`: the org's client hook (what `cori_server::client_hook` sends). `app.created`
/// answers the app's name as its client id, the same every time; nothing is registered, since any client id gets tokens
/// with its `resource_access` roles. Roles, a moved front and anything else are answered 200 (nothing to do here). Stale
/// and wrongly signed deliveries are refused.
async fn client_hook(State(idp): State<Arc<Idp>>, headers: HeaderMap, body: Bytes) -> (StatusCode, Json<Value>) {
    let (status, answer) = answer_hook(&idp, &headers, &body);
    log_delivery(&idp, None, &headers, &body, status);
    (status, answer)
}

/// A delivery as a line of `DEV_IDP_HOOK_LOG`, with `via` when it came to another endpoint than the client hook's.
fn log_delivery(idp: &Idp, via: Option<&str>, headers: &HeaderMap, body: &[u8], status: StatusCode) {
    let Some(log) = &idp.hook_log else { return };
    let b: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    let mut line = json!({ "event": b["event"], "id": b["id"], "app": b["app"], "role": b["role"], "client_id": b["client_id"], "description": b["description"], "status": status.as_u16() });
    // people and access, and an app's owner: what they are about, when they say it
    for (k, v) in [("request", b["request"].clone()), ("person", b["person"].clone()), ("by", b["by"].clone()), ("owner", b["owner"].clone()), ("question", json!(headers.get("x-cori-question").and_then(|q| q.to_str().ok()))), ("via", json!(via))] {
        if !v.is_null() { line[k] = v; }
    }
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(log) { let _ = writeln!(f, "{line}"); }
}

/// `POST /access`: the organization's own access webhook (`CORI_IDENTITY_ACCESS_URL`). Stale and wrongly signed deliveries
/// are refused; an event other than `access.granted` and `access.revoked` is answered 200, ignored.
async fn access_webhook(State(idp): State<Arc<Idp>>, headers: HeaderMap, body: Bytes) -> (StatusCode, Json<Value>) {
    let refuse = |code: StatusCode, e: &str| (code, Json(json!({ "error": e, "message": e })));
    let (status, answer) = if let Some(secret) = &idp.access_secret && !signed(secret, &headers, &body) { refuse(StatusCode::UNAUTHORIZED, "bad signature") }
        else { match serde_json::from_slice::<Value>(&body) {
            Err(_) => refuse(StatusCode::BAD_REQUEST, "the body is not JSON"),
            Ok(b) if !fresh(&b) => refuse(StatusCode::BAD_REQUEST, "stale delivery"),
            Ok(b) => match b["event"].as_str() {
                Some(e @ ("access.granted" | "access.revoked")) => answer_people_event(&idp, e, &b, (idp.access_sync_secs > 0).then(|| (idp.access_sync_secs, idp.access_secret.clone()))),
                _ => (StatusCode::OK, Json(json!({}))),
            },
        } };
    log_delivery(&idp, Some("access"), &headers, &body, status);
    (status, answer)
}

/// Whether `body` carries the signature of `secret` (`x-cori-signature: sha256=<hex>`).
fn signed(secret: &str, headers: &HeaderMap, body: &[u8]) -> bool {
    use hmac::{Hmac, KeyInit, Mac};
    let sig = headers.get("x-cori-signature").and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("sha256=")).and_then(|h| hex::decode(h).ok()).unwrap_or_default();
    let mut m = <Hmac<sha2::Sha256>>::new_from_slice(secret.as_bytes()).unwrap();
    m.update(body);
    m.verify_slice(&sig).is_ok()
}

/// Whether a delivery's `at` is within five minutes of now.
fn fresh(b: &Value) -> bool { chrono::DateTime::parse_from_rfc3339(b["at"].as_str().unwrap_or("")).is_ok_and(|at| chrono::Utc::now().signed_duration_since(at).num_seconds().abs() < 300) }

fn answer_hook(idp: &Arc<Idp>, headers: &HeaderMap, body: &[u8]) -> (StatusCode, Json<Value>) {
    let refuse = |code: StatusCode, e: &str| (code, Json(json!({ "error": e })));
    if idp.hook_down.as_deref().is_some_and(|f| std::path::Path::new(f).exists()) { return refuse(StatusCode::SERVICE_UNAVAILABLE, "down for the test"); }
    if let Some(secret) = &idp.hook_secret && !signed(secret, headers, body) { return refuse(StatusCode::UNAUTHORIZED, "bad signature"); }
    let Ok(b) = serde_json::from_slice::<Value>(body) else { return refuse(StatusCode::BAD_REQUEST, "the body is not JSON") };
    if !fresh(&b) { return refuse(StatusCode::BAD_REQUEST, "stale delivery"); }
    let app = b["app"].as_str().unwrap_or("");
    let people = idp.people.mode.as_str();
    if !people.is_empty() {
        if b["event"] == "ping" { return (StatusCode::OK, Json(capabilities(people))); }
        if headers.get("x-cori-question").is_some() { return answer_question(idp, &b); }
        if let Some(e) = b["event"].as_str().filter(|e| PEOPLE_EVENTS.contains(e)) {
            if people == "read" { return (StatusCode::OK, Json(json!({ "ignored": e }))); }
            return answer_people_event(idp, e, &b, (people == "requests").then(|| (idp.people.settle_secs, idp.hook_secret.clone())));
        }
    }
    match b["event"].as_str() {
        Some("app.created") if !app.is_empty() => {
            let seen = { let mut p = idp.pending_seen.lock().unwrap(); let n = p.entry(app.to_string()).or_insert(0); *n += 1; *n };
            if seen <= idp.hook_pending { eprintln!("[dev idp] client hook: {app} → 202 ({seen}/{})", idp.hook_pending); return (StatusCode::ACCEPTED, Json(json!({ "message": "the identity team is creating the client" }))); }
            eprintln!("[dev idp] client hook: {app} → client {app}");
            (StatusCode::OK, Json(json!({ "client_id": app })))
        }
        Some("app.created") => refuse(StatusCode::BAD_REQUEST, "no app"),
        Some(e) => { eprintln!("[dev idp] client hook: {e} {app} {}", b["role"].as_str().unwrap_or("")); (StatusCode::OK, Json(json!({}))) }
        None => refuse(StatusCode::BAD_REQUEST, "no event"),
    }
}

/// The directory of the organization's own (`POST /people`): its users but those `DEV_IDP_DIRECTORY_LEFT` names, two a
/// page, by id, address and the directory's name.
async fn directory(State(idp): State<Arc<Idp>>, headers: HeaderMap, body: Bytes) -> (StatusCode, Json<Value>) {
    let refuse = |code: StatusCode, e: &str| (code, Json(json!({ "error": e, "message": e })));
    if let Some(secret) = &idp.directory_secret && !signed(secret, &headers, &body) { return refuse(StatusCode::UNAUTHORIZED, "bad signature"); }
    let Ok(b) = serde_json::from_slice::<Value>(&body) else { return refuse(StatusCode::BAD_REQUEST, "the body is not JSON") };
    if !fresh(&b) { return refuse(StatusCode::BAD_REQUEST, "stale question"); }
    if headers.get("x-cori-question").and_then(|q| q.to_str().ok()) != Some("people.list") { return (StatusCode::NOT_FOUND, Json(json!({ "ignored": b["event"] }))); }
    let left: Vec<String> = idp.directory_left.as_deref().and_then(|f| std::fs::read_to_string(f).ok()).map(|t| t.split_whitespace().map(String::from).collect()).unwrap_or_default();
    let mut users: Vec<&str> = idp.users.keys().copied().filter(|u| !left.iter().any(|l| l == u)).collect();
    users.sort();
    let from: usize = b["cursor"].as_str().and_then(|c| c.parse().ok()).unwrap_or(0);
    let page: Vec<Value> = users.iter().skip(from).take(2).map(|u| {
        let name: String = u.chars().take(1).flat_map(char::to_uppercase).chain(u.chars().skip(1)).collect();
        json!({ "id": u, "email": format!("{u}@acme.example"), "name": format!("{name} (directory)") })
    }).collect();
    let next = (from + 2 < users.len()).then(|| (from + 2).to_string());
    eprintln!("[dev idp] directory: people.list from {from}: {} of {}", page.len(), users.len());
    (StatusCode::OK, Json(json!({ "people": page, "next": next })))
}

const PEOPLE_EVENTS: &[&str] = &["member.invited", "member.roles", "member.removed", "invitation.cancelled", "invitation.resent", "access.granted", "access.revoked", "organization.updated"];
const QUESTIONS: &[&str] = &["organization.get", "people.list", "invitations.list", "access.list", "person.organizations"];

/// What `ping` answers: every people event and question (`manage`), the events settled later (`requests`), the questions
/// alone (`read`).
fn capabilities(mode: &str) -> Value {
    let events: &[&str] = if mode == "read" { &[] } else { PEOPLE_EVENTS };
    json!({ "capabilities": { "events": events, "questions": QUESTIONS, "settles": mode == "requests" } })
}

fn person_json(idp: &Idp, user: &str) -> Value {
    let grants: serde_json::Map<String, Value> = idp.people.grants.lock().unwrap().iter().filter(|((who, _), r)| who == user && !r.is_empty()).map(|((_, c), r)| (c.clone(), json!(r))).collect();
    let roles = member_roles(idp, user);
    json!({ "id": user, "email": format!("{user}@acme.example"), "name": user, "guest": roles.iter().any(|r| r == "guest"), "roles": roles, "grants": grants })
}

fn answer_question(idp: &Idp, b: &Value) -> (StatusCode, Json<Value>) {
    let ok = |v: Value| (StatusCode::OK, Json(v));
    let removed = idp.people.removed.lock().unwrap().clone();
    let mut users: Vec<&str> = idp.users.keys().copied().filter(|u| !removed.contains(*u)).collect(); users.sort();
    match b["event"].as_str().unwrap_or("") {
        "organization.get" => { let (name, slug) = idp.people.org.lock().unwrap().clone(); ok(json!({ "name": name, "slug": slug, "roles": [{ "name": "it_admin", "description": "Administers the organization" }, { "name": "builder", "description": "Builds apps" }, { "name": "approver", "description": "Approves releases" }, { "name": "member", "description": "Uses the apps given to them" }] })) }
        "people.list" => ok(json!({ "people": users.iter().map(|u| person_json(idp, u)).collect::<Vec<_>>(), "next": null })),
        "invitations.list" => ok(json!({ "invitations": idp.people.invitations.lock().unwrap().clone() })),
        "access.list" => {
            let client = b["client_id"].as_str().unwrap_or("");
            let grants: Vec<Value> = idp.people.grants.lock().unwrap().iter().filter(|((who, c), r)| c == client && !r.is_empty() && !removed.contains(who)).map(|((who, _), r)| json!({ "person": { "id": who, "email": format!("{who}@acme.example"), "name": who }, "roles": r })).collect();
            ok(json!({ "grants": grants }))
        }
        "person.organizations" => { let (name, slug) = idp.people.org.lock().unwrap().clone(); ok(json!({ "organizations": [{ "id": "acme", "slug": slug, "name": name }] })) }
        q => ok(json!({ "ignored": q })),
    }
}

/// A people or access event: checked (an admin asks, the person exists), then applied at once, or, with `later` (how long,
/// and the secret its notification is signed with), answered `202` and applied when the IAM's stand-in settles it.
fn answer_people_event(idp: &Arc<Idp>, event: &str, b: &Value, later: Option<(u64, Option<String>)>) -> (StatusCode, Json<Value>) {
    // an access event may name the person by their address (someone shared with from outside): the user it is
    let mut b = b.clone();
    if b["person"].as_str().is_none_or(str::is_empty) && let Some(email) = b["email"].as_str() {
        if let Some(u) = idp.users.keys().find(|u| format!("{u}@acme.example") == email.to_lowercase()) { b["person"] = json!(u); }
    }
    let b = &b;
    let request = b["request"].as_str().unwrap_or("").to_string();
    if let Some((status, v)) = idp.people.answered.lock().unwrap().get(&request).cloned() { return (StatusCode::from_u16(status).unwrap_or(StatusCode::OK), Json(v)); }
    let refuse = |status: StatusCode, message: &str| (status, Json(json!({ "error": "refused", "message": message })));
    let by = b["by"].as_str().unwrap_or("");
    let admin = member_roles(idp, by).iter().any(|r| r == "it_admin");
    let person = b["person"].as_str().unwrap_or("");
    let known = |u: &str| idp.users.contains_key(u) && !idp.people.removed.lock().unwrap().contains(u);
    let roles: Vec<String> = b["roles"].as_array().map(|a| a.iter().filter_map(|r| r.as_str().map(String::from)).collect()).unwrap_or_default();
    let check = match event {
        // a guest's access needs an admin's approval
        "access.granted" if b["guest"] == true && !member_roles(idp, b["approved_by"].as_str().unwrap_or("")).iter().any(|r| r == "it_admin") => Some((StatusCode::FORBIDDEN, "a guest's access needs an admin's approval")),
        "access.granted" | "access.revoked" => if !known(by) { Some((StatusCode::FORBIDDEN, "who asked is not a member")) } else if !known(person) { Some((StatusCode::NOT_FOUND, "no such person in the organization")) } else if roles.is_empty() { Some((StatusCode::UNPROCESSABLE_ENTITY, "no role")) } else { None },
        _ if !admin => Some((StatusCode::FORBIDDEN, "only an admin manages people")),
        "member.roles" | "member.removed" if !known(person) => Some((StatusCode::NOT_FOUND, "no such person in the organization")),
        "member.invited" if idp.users.keys().any(|u| format!("{u}@acme.example") == b["email"].as_str().unwrap_or("")) => Some((StatusCode::CONFLICT, "already a member")),
        _ => None,
    };
    if let Some((status, message)) = check { let a = refuse(status, message); idp.people.answered.lock().unwrap().insert(request.clone(), (status.as_u16(), a.1.0.clone())); return a; }
    if let Some((secs, secret)) = later {
        let (idp2, event, b) = (idp.clone(), event.to_string(), b.clone());
        tokio::spawn(async move {
            let idp = idp2;
            tokio::time::sleep(std::time::Duration::from_secs(secs)).await;
            let result = apply(&idp, &event, &b);
            let Some(url) = idp.people.notify.clone() else { return };
            let now = chrono::Utc::now().to_rfc3339();
            let mut body = json!({ "id": uuid::Uuid::new_v4().to_string(), "event": "request.settled", "at": now, "request": b["request"], "outcome": "done", "message": "approved by the IAM's stand-in", "result": result });
            if let Some(r) = b["realm"].as_str() { body["realm"] = json!(r); }
            let text = body.to_string();
            let mut req = reqwest::Client::new().post(&url).header("content-type", "application/json");
            if let Some(secret) = &secret { use hmac::{Hmac, KeyInit, Mac}; let mut m = <Hmac<sha2::Sha256>>::new_from_slice(secret.as_bytes()).unwrap(); m.update(text.as_bytes()); req = req.header("x-cori-signature", format!("sha256={}", hex::encode(m.finalize().into_bytes()))); }
            match req.body(text).send().await { Ok(r) => eprintln!("[dev idp] request.settled {}: {}", b["request"], r.status()), Err(e) => eprintln!("[dev idp] request.settled {}: {e}", b["request"]) }
        });
        let a = json!({ "message": "waiting for the IAM's approval", "settles": true });
        idp.people.answered.lock().unwrap().insert(request, (202, a.clone()));
        return (StatusCode::ACCEPTED, Json(a));
    }
    let result = apply(idp, event, b);
    idp.people.answered.lock().unwrap().insert(request, (200, result.clone()));
    (StatusCode::OK, Json(result))
}

/// What a people or access event changes in the directory; its answer's body.
fn apply(idp: &Idp, event: &str, b: &Value) -> Value {
    let p = &idp.people;
    let person = b["person"].as_str().unwrap_or("").to_string();
    let roles: Vec<String> = b["roles"].as_array().map(|a| a.iter().filter_map(|r| r.as_str().map(String::from)).collect()).unwrap_or_default();
    let client = b["client_id"].as_str().unwrap_or("").to_string();
    eprintln!("[dev idp] people: {event} {person}{} {}", b["email"].as_str().unwrap_or(""), roles.join(","));
    match event {
        "member.invited" => {
            let id = uuid::Uuid::new_v4().simple().to_string();
            p.invitations.lock().unwrap().push(json!({ "id": id, "email": b["email"], "roles": roles, "grants": b["grants"], "inviter": b["by"], "created_at": chrono::Utc::now().to_rfc3339() }));
            json!({ "invitation": id })
        }
        "invitation.cancelled" => { let inv = b["invitation"].as_str().unwrap_or(""); p.invitations.lock().unwrap().retain(|i| i["id"] != inv); json!({}) }
        "invitation.resent" => json!({}),
        "member.roles" => { p.roles.lock().unwrap().insert(person, roles); json!({}) }
        "member.removed" => { p.grants.lock().unwrap().retain(|(who, _), _| who != &person); p.removed.lock().unwrap().insert(person); json!({}) }
        "access.granted" => { let mut g = p.grants.lock().unwrap(); let e = g.entry((person.clone(), client)).or_default(); for r in roles { if !e.contains(&r) { e.push(r); } } json!({ "person": person }) }
        "access.revoked" => {
            let mut g = p.grants.lock().unwrap();
            if let Some(e) = g.get_mut(&(person.clone(), client)) { e.retain(|r| !roles.contains(r)); }
            // a guest left with no app is no longer of the organization
            if member_roles(idp, &person).iter().any(|r| r == "guest") && !g.iter().any(|((who, _), r)| who == &person && !r.is_empty()) { p.removed.lock().unwrap().insert(person); }
            json!({})
        }
        "organization.updated" => { let mut o = p.org.lock().unwrap(); if let Some(n) = b["name"].as_str() { o.0 = n.into(); } if let Some(s) = b["slug"].as_str() { o.1 = s.into(); } json!({}) }
        _ => json!({}),
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    use rsa::pkcs1::EncodeRsaPrivateKey;
    use rsa::traits::PublicKeyParts;
    let port: u16 = std::env::var("DEV_IDP_PORT").ok().and_then(|v| v.parse().ok()).unwrap_or(8099);
    let issuer = format!("http://127.0.0.1:{port}/realms/acme");
    // its key, kept in `DEV_IDP_KEY_FILE` when given: a test that restarts it keeps the tokens it issued valid
    let priv_key = match std::env::var("DEV_IDP_KEY_FILE").ok().filter(|f| !f.is_empty()) {
        Some(f) if std::path::Path::new(&f).exists() => { use rsa::pkcs1::DecodeRsaPrivateKey; rsa::RsaPrivateKey::from_pkcs1_pem(&std::fs::read_to_string(&f)?)? }
        Some(f) => { let k = rsa::RsaPrivateKey::new(&mut rsa::rand_core::OsRng, 2048)?; std::fs::write(&f, k.to_pkcs1_pem(rsa::pkcs1::LineEnding::LF)?.as_bytes())?; k }
        None => rsa::RsaPrivateKey::new(&mut rsa::rand_core::OsRng, 2048)?,
    };
    let pub_key = priv_key.to_public_key();
    let pem = priv_key.to_pkcs1_pem(rsa::pkcs1::LineEnding::LF)?;
    let enc = EncodingKey::from_rsa_pem(pem.as_bytes())?;
    let b64 = |b: &[u8]| base64::Engine::encode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, b);
    let (n, e) = (b64(&pub_key.n().to_bytes_be()), b64(&pub_key.e().to_bytes_be()));
    let kid = "dev-1".to_string();
    let jwks = json!({ "keys": [{ "kty": "RSA", "use": "sig", "alg": "RS256", "kid": kid, "n": n, "e": e }] });
    let set: jwk::JwkSet = serde_json::from_value(jwks.clone())?;
    let dec = DecodingKey::from_jwk(&set.keys[0])?;
    let users = HashMap::from([("alice", ("alice", vec!["support_agent"])), ("sam", ("sam", vec!["support_lead", "approver"])), ("jean", ("jean", vec!["it_admin"])), ("gina", ("gina", vec!["guest"]))]);
    let clients = HashMap::from([("schedule-support-digest", ("digest-secret", vec!["support_lead"]))]);
    let ttl = std::env::var("DEV_IDP_TTL").ok().and_then(|v| v.parse().ok()).unwrap_or(3600);
    let extra_aud = std::env::var("DEV_IDP_AUDIENCE").ok().filter(|v| !v.is_empty());
    let hook_secret = std::env::var("CORI_AUTH_CLIENT_HOOK_SECRET").ok().filter(|v| !v.is_empty());
    let hook = if hook_secret.is_some() { "signed" } else { "unsigned accepted" };
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    let idp = Arc::new(Idp { issuer: issuer.clone(), kid, enc, dec, jwks, ttl, extra_aud, users, clients, codes: Mutex::new(HashMap::new()), hook_secret,
        hook_log: env("DEV_IDP_HOOK_LOG"), hook_pending: env("DEV_IDP_HOOK_PENDING").and_then(|v| v.parse().ok()).unwrap_or(0), hook_down: env("DEV_IDP_HOOK_DOWN"), pending_seen: Mutex::new(HashMap::new()),
        people: People { mode: env("DEV_IDP_PEOPLE").filter(|m| matches!(m.as_str(), "manage" | "requests" | "read")).unwrap_or_default(), notify: env("DEV_IDP_NOTIFY_URL"), settle_secs: env("DEV_IDP_SETTLE_SECS").and_then(|v| v.parse().ok()).unwrap_or(2), org: Mutex::new(("Acme".into(), "acme".into())), ..Default::default() }, auto_user: env("DEV_IDP_USER"), app_roles_prefix: env("DEV_IDP_APP_ROLES_PREFIX"),
        limited_scope: env("DEV_IDP_LIMITED_SCOPE").map(|v| v.split(',').map(|c| c.trim().to_string()).filter(|c| !c.is_empty()).collect()).unwrap_or_default(),
        directory_secret: env("DEV_IDP_DIRECTORY_SECRET"), directory_left: env("DEV_IDP_DIRECTORY_LEFT"),
        access_secret: env("DEV_IDP_ACCESS_SECRET"), access_sync_secs: env("DEV_IDP_ACCESS_SYNC_SECS").and_then(|v| v.parse().ok()).unwrap_or(0) });
    let app = Router::new()
        .route("/realms/acme/.well-known/openid-configuration", get({ let iss = issuer.clone(); move || async move { Json(json!({
            "issuer": iss, "jwks_uri": format!("{iss}/protocol/openid-connect/certs"), "token_endpoint": format!("{iss}/protocol/openid-connect/token"),
            "authorization_endpoint": format!("{iss}/protocol/openid-connect/auth"), "device_authorization_endpoint": null,
            "grant_types_supported": ["password", "authorization_code", "refresh_token", "client_credentials", "urn:ietf:params:oauth:grant-type:token-exchange"],
            "code_challenge_methods_supported": ["S256"] })) } }))
        .route("/realms/acme/protocol/openid-connect/certs", get(|State(idp): State<Arc<Idp>>| async move { Json(idp.jwks.clone()) }))
        .route("/realms/acme/protocol/openid-connect/token", post(token))
        .route("/realms/acme/protocol/openid-connect/auth", get(authorize))
        .route("/cori/clients", post(client_hook))
        .route("/people", post(directory))
        .route("/access", post(access_webhook))
        .layer(tower_http::cors::CorsLayer::permissive())   // browsers exchange codes at the token endpoint (Keycloak: Web origins)
        .with_state(idp);
    eprintln!("[dev idp] {issuer}  users: alice (support_agent), sam (support_lead, approver), jean (it_admin), gina (guest) · service account schedule-support-digest/digest-secret (support_lead) · access tokens {ttl}s · client hook POST /cori/clients ({hook})");
    axum::serve(tokio::net::TcpListener::bind(("127.0.0.1", port)).await?, app).await?;
    Ok(())
}
