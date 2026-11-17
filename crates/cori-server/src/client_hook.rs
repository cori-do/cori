// SPDX-License-Identifier: Apache-2.0
//! The identity hook (`CORI_AUTH_CLIENT_HOOK`): what the server tells the organization's identity provider, or the IAM in
//! front of it, about apps and their roles. Every app signs its users in with an OIDC client of its own, which the
//! organization's code registers; with `CORI_AUTH_CLIENT_HOOK_ROLES=true` it also hears of each role an app declares, so that
//! its IAM keeps the app's roles in its catalog and grants them to people. `cori identity test-hook <url>` checks a
//! receiver against the contract.
//!
//! Events, each a signed POST (`x-cori-signature: sha256=<hmac of the body>` with `CORI_AUTH_CLIENT_HOOK_SECRET`, the event
//! in `x-cori-event`, its id in `x-cori-delivery` and the body's `id`, the time of the attempt in `at` and of the event in
//! `occurred_at`):
//! - `app.created` (the app's owner, the roles it starts with, the front's `redirect_uris`, `post_logout_redirect_uris` and
//!   `origin`, the key that wraps its credentials when a key wrapper names one): answered `200 { client_id }`, or `202` while
//!   the IAM creates the client (the app is created, its client `pending`, and the event is delivered again until a
//!   `client_id` comes back). Any other answer refuses the app.
//! - `app.updated` (the front moved: a new fronts URL), `app.deleted` (with the client, which may be none yet);
//! - `role.created`, `role.updated` (description or note), `role.deleted` (no version of the app kept declares it, for
//!   `CORI_ROLE_DELETE_GRACE_SECS`): answered `200` (done: a role is then `confirmed`) or `202` (delivered again later).
//! - `ping`, which the preflight, the conformance kit and the server send: its answer lists what the receiver handles
//!   besides apps and roles (`Capabilities`).
//! - People and access (`PEOPLE_EVENTS`, to a receiver that lists them): `member.invited`, `member.roles`, `member.removed`,
//!   `invitation.cancelled`, `invitation.resent`, `access.granted`, `access.revoked`, `organization.updated`. Each carries
//!   the request it delivers (`access_requests`, `api::people`) and who asked (`by`); its answer settles the request: `200`
//!   done, `202` pending (delivered again, or settled by the receiver's `request.settled` when it says `settles`), a
//!   `400`, `403`, `404`, `409`, `410` or `422` refused, an `ignored` answer refused (the receiver does not handle it).
//!   A webhook of the organization's own for apps' access (`CORI_IDENTITY_ACCESS_URL`) takes `access.granted` and
//!   `access.revoked` in the receiver's place, signed with its own secret, answered alike (`access_url`).
//!
//! Questions (`ask`) read what only the provider knows, at once and never queued: `organization.get`, `people.list`,
//! `invitations.list`, `access.list`, `person.organizations`; a receiver that does not answer one leaves the console
//! showing what Cori itself saw. A directory of the organization's own (`CORI_IDENTITY_PEOPLE_URL`) answers `people.list`
//! in its place, asked alike. Notifications (`api::identity_notify`) are the receiver telling Cori what changed.
//!
//! What follows `app.created`'s first answer goes through an outbox (`identity_events`): per app in order, each event
//! retried with backoff (10 s, doubling, at most an hour) until it is answered, for `CORI_IDENTITY_GIVE_UP_SECS` (7 days),
//! then `dead`. The leading replica delivers what is due; an event is also tried at once when it is queued. Receivers must
//! be idempotent: the same `id` may come twice, and `app.created` for a known app answers the same `client_id`. Under a
//! tenancy every event carries the app's `realm`: app ids are unique within a realm only.
use crate::state::AppState;
use serde_json::{Value, json};
use sqlx::Row;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

pub use cori_compiler::roles::RoleDecl;

/// What the hook tells the organization's IAM about roles, and when.
#[derive(Debug, Clone)]
pub struct Settings {
    /// The hook hears of each role an app declares (`CORI_AUTH_CLIENT_HOOK_ROLES`); otherwise only of apps, a deploy sending
    /// `app.created` again with the roles the version knows.
    pub roles: bool,
    /// Roles reach the IAM when a version declaring them goes live (`CORI_ROLES_SYNC_AT=promote`), not when it is deployed:
    /// testers then cannot hold a new role before it is live.
    pub roles_at_promote: bool,
    /// A deploy using a role cori.toml does not declare is refused (`CORI_ROLES_STRICT`), so that the IAM's catalog is complete.
    pub strict_roles: bool,
    /// A release goes live only once the IAM confirmed every role it declares (`CORI_PROMOTE_REQUIRES_CONFIRMED_ROLES`).
    pub promote_needs_confirmed_roles: bool,
    /// How long a role no kept version declares stays before `role.deleted` (`CORI_ROLE_DELETE_GRACE_SECS`, default 7 days).
    pub delete_grace: Duration,
    /// How long an event is retried before it is given up (`CORI_IDENTITY_GIVE_UP_SECS`, default 7 days).
    pub give_up: Duration,
}

impl Settings {
    pub fn from_env() -> Self {
        let secs = |k: &str, default: u64| Duration::from_secs(std::env::var(k).ok().and_then(|v| v.trim().parse().ok()).unwrap_or(default));
        Self {
            roles: crate::env_flag("CORI_AUTH_CLIENT_HOOK_ROLES", false),
            roles_at_promote: std::env::var("CORI_ROLES_SYNC_AT").map(|v| v.trim().eq_ignore_ascii_case("promote")).unwrap_or(false),
            strict_roles: crate::env_flag("CORI_ROLES_STRICT", false),
            promote_needs_confirmed_roles: crate::env_flag("CORI_PROMOTE_REQUIRES_CONFIRMED_ROLES", false),
            delete_grace: secs("CORI_ROLE_DELETE_GRACE_SECS", 7 * 86400),
            give_up: secs("CORI_IDENTITY_GIVE_UP_SECS", 7 * 86400),
        }
    }
}

/// The hook URL under builtin auth (required there: `serve` refuses to start without it); dev and off modes have no
/// issuer to register with.
pub fn hook_url(st: &AppState) -> Option<String> {
    if !matches!(st.cfg.auth, crate::auth::AuthMode::Builtin { .. }) { return None; }
    st.cfg.client_hook.clone()
}

/// Whether the hook hears of roles one by one.
pub fn role_events(st: &AppState) -> bool { st.cfg.identity.roles && hook_url(st).is_some() }

/// The fixed sign-in callback of an app's front. Previews live under `/apps/<app>/@<hash>/`, so the redirect cannot be the
/// page itself (issuers match redirect URIs exactly); the callback sends the browser back to the page it came from.
pub fn callback_url(st: &AppState, app: &str) -> String { crate::names::callback_url(&st.front_origin(app), app) }

/// The deployment's own roles (its platform and approver roles), which no app declares and the hook never hears of.
pub fn deployment_roles(st: &AppState) -> Vec<String> {
    let r = crate::auth::platform_roles();
    let mut out: Vec<String> = [&r.admin, &r.operator, &r.auditor, &r.billing, &r.builder, &r.store_admin].into_iter().flatten().cloned().collect();
    out.extend(st.settings.approver_roles.iter().cloned());
    out.sort(); out.dedup();
    out
}

/// One answer of the hook: its status and its body (null when it is not JSON).
#[derive(Debug, Clone)]
pub struct Answer { pub status: u16, pub body: Value }

impl Answer {
    /// What the answer says in words: its `message`, `error` or `detail`.
    pub fn note(&self) -> String {
        ["message", "error", "detail"].iter().find_map(|k| self.body[*k].as_str()).map(String::from).unwrap_or_else(|| format!("http {}", self.status))
    }
}

/// `sha256=<hex HMAC-SHA256 of body>` under `secret`.
pub fn sign(secret: &str, body: &str) -> String {
    use hmac::{Hmac, KeyInit, Mac};
    let mut m = <Hmac<sha2::Sha256>>::new_from_slice(secret.as_bytes()).expect("hmac takes any key");
    m.update(body.as_bytes());
    format!("sha256={}", hex::encode(m.finalize().into_bytes()))
}

/// Whether `header` (`sha256=<hex>`) is the signature of `body` under `secret`, compared in constant time: what a
/// notification from the receiver must carry.
pub fn verify(secret: &str, body: &[u8], header: &str) -> bool {
    use hmac::{Hmac, KeyInit, Mac};
    let Some(sig) = header.trim().strip_prefix("sha256=").and_then(|h| hex::decode(h).ok()) else { return false };
    let mut m = <Hmac<sha2::Sha256>>::new_from_slice(secret.as_bytes()).expect("hmac takes any key");
    m.update(body);
    m.verify_slice(&sig).is_ok()
}

/// One delivery: `payload` with the event's `id`, `event`, `occurred_at`, the attempt's `at` (for the receiver's freshness
/// check) and the realm under a tenancy, signed with `secret`.
async fn post(st: &AppState, url: &str, secret: Option<&str>, event: &str, id: &str, occurred_at: &str, payload: &Value) -> Result<Answer, String> {
    let mut body = payload.clone();
    body["id"] = json!(id); body["event"] = json!(event); body["occurred_at"] = json!(occurred_at); body["at"] = json!(chrono::Utc::now().to_rfc3339());
    if let Some(r) = st.realm() { body["realm"] = json!(r.as_str()); }
    let text = body.to_string();
    let mut req = cori_engine::http::builder().timeout(Duration::from_secs(10)).build().map_err(|e| e.to_string())?
        .post(url).header("content-type", "application/json").header("x-cori-event", event).header("x-cori-delivery", id).header("user-agent", "cori-client-hook/2");
    if let Some(secret) = secret { req = req.header("x-cori-signature", sign(secret, &text)); }
    let r = req.body(text).send().await.map_err(|e| format!("client hook {url}: {e}"))?;
    let status = r.status().as_u16();
    Ok(Answer { status, body: r.json().await.unwrap_or(Value::Null) })
}

/// A signed `ping` to `url`, answered or not: the preflight, before any state exists.
pub async fn ping_url(url: &str, secret: Option<&str>) -> Result<Answer, String> {
    let now = chrono::Utc::now().to_rfc3339();
    let body = json!({ "id": uuid::Uuid::new_v4().to_string(), "event": "ping", "occurred_at": now, "at": now }).to_string();
    let mut req = cori_engine::http::builder().timeout(Duration::from_secs(10)).build().map_err(|e| e.to_string())?
        .post(url).header("content-type", "application/json").header("x-cori-event", "ping").header("user-agent", "cori-client-hook/2");
    if let Some(s) = secret { req = req.header("x-cori-signature", sign(s, &body)); }
    let r = req.body(body).send().await.map_err(|e| format!("client hook {url}: {e}"))?;
    let status = r.status().as_u16();
    Ok(Answer { status, body: r.json().await.unwrap_or(Value::Null) })
}

/// What `app.created` says of an app.
async fn app_created_body(st: &AppState, app: &str, owner: Option<&str>, roles: &[String]) -> Value {
    let base = st.cfg.public_url.trim_end_matches('/');
    let origin = st.front_origin(app);
    let mut body = json!({ "app": app, "owner": owner, "roles": roles, "redirect_uris": [callback_url(st, app)], "post_logout_redirect_uris": [crate::names::post_logout_url(&origin, app)], "origin": origin, "resource": base });
    // the key that will wrap the app's credentials, for the organization's automation to set its policies up
    match st.broker.prepare_app(&st.db, app).await {
        Ok(Some(key)) => body["secrets_key"] = json!(key),
        Ok(None) => {}
        Err(e) => tracing::warn!(app, error = %format!("{e:#}"), "the app's credentials key is not ready: the first secret's write retries"),
    }
    body
}

/// Where an app's sign-in client stands after `provision`.
#[derive(Debug, Clone, PartialEq)]
pub enum Client {
    /// The client the hook answered with.
    Bound(String),
    /// The organization creates it (the hook answered 202): `app.created` is delivered again until a client comes back.
    Pending(String),
    /// No hook (dev and off auth).
    None,
}

/// Registers (or re-asserts) the app's client: `app.created`, at once. A 202 leaves the client pending and the event in the
/// outbox; any other refusal is the caller's error.
pub async fn provision(st: &Arc<AppState>, app: &str, owner: Option<&str>, roles: &[String]) -> Result<Client, String> {
    let Some(url) = hook_url(st) else { return Ok(Client::None) };
    let body = app_created_body(st, app, owner, roles).await;
    let (id, now) = (uuid::Uuid::new_v4().to_string(), chrono::Utc::now().to_rfc3339());
    let answer = post(st, &url, st.cfg.client_hook_secret.as_deref(), "app.created", &id, &now, &body).await?;
    match answer.status {
        202 => {
            sqlx::query("UPDATE apps SET client_status = 'pending', client_note = $1 WHERE id = $2").bind(answer.note()).bind(app).execute(&st.db).await.map_err(|e| e.to_string())?;
            enqueue_with(st, app, "app.created", &body, Some((&id, 1, answer.status, &answer.note()))).await.map_err(|e| e.to_string())?;
            Ok(Client::Pending(answer.note()))
        }
        s if (200..300).contains(&s) => {
            let cid = answer.body["client_id"].as_str().filter(|c| !c.is_empty()).ok_or_else(|| format!("client hook {url}: the answer has no `client_id`"))?.to_string();
            bind(&st.db, app, &cid, body["origin"].as_str()).await.map_err(|e| e.to_string())?;
            Ok(Client::Bound(cid))
        }
        s => Err(format!("client hook {url}: http {s}: {}", answer.note())),
    }
}

/// The app's client is the one the hook answered with.
async fn bind<'e>(db: impl sqlx::Executor<'e, Database = sqlx::Postgres>, app: &str, client_id: &str, origin: Option<&str>) -> Result<(), sqlx::Error> {
    // a client its tokens cannot name the roles of: nobody would hold one in the app
    if let Some(why) = crate::auth::app_roles_claim().unreadable(client_id) { tracing::warn!(app, "{why}"); }
    sqlx::query("UPDATE apps SET client_id = $1, client_status = NULL, client_note = NULL, client_origin = COALESCE($2, client_origin) WHERE id = $3").bind(client_id).bind(origin).bind(app).execute(db).await.map(|_| ())
}

/// Queues an event for `app` and tries to deliver it at once.
pub async fn enqueue(st: &Arc<AppState>, app: &str, event: &str, body: &Value) -> anyhow::Result<String> {
    enqueue_with(st, app, event, body, None).await
}

/// Queues an event for `app` without trying it now: the leading replica delivers it (within 20 s), or the app's next
/// event. For what a delivery itself leads to (`api::person_left`), which does not start another delivery.
pub async fn enqueue_later(st: &AppState, app: &str, event: &str, body: &Value) -> anyhow::Result<String> {
    let (id, now) = (uuid::Uuid::new_v4().to_string(), chrono::Utc::now().to_rfc3339());
    sqlx::query("INSERT INTO identity_events (id, app, event, body, status, attempts, next_at, occurred_at) VALUES ($1, $2, $3, $4, 'queued', 0, $5, $5)")
        .bind(&id).bind(app).bind(event).bind(body.to_string()).bind(&now).execute(&st.db).await?;
    Ok(id)
}

/// `enqueue`, with what a first attempt already got (its id, attempts, status and answer): delivered again later.
async fn enqueue_with(st: &Arc<AppState>, app: &str, event: &str, body: &Value, tried: Option<(&str, i64, u16, &str)>) -> anyhow::Result<String> {
    let now = chrono::Utc::now();
    let id = tried.map(|t| t.0.to_string()).unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let attempts = tried.map(|t| t.1).unwrap_or(0);
    let next = if attempts == 0 { now } else { now + chrono::Duration::from_std(backoff(attempts)).unwrap_or_default() };
    sqlx::query("INSERT INTO identity_events (id, app, event, body, status, attempts, next_at, occurred_at, last_status, last_answer) VALUES ($1, $2, $3, $4, 'queued', $5, $6, $7, $8, $9)")
        .bind(&id).bind(app).bind(event).bind(body.to_string()).bind(attempts).bind(next.to_rfc3339()).bind(now.to_rfc3339()).bind(tried.map(|t| t.2 as i64)).bind(tried.map(|t| t.3.to_string())).execute(&st.db).await?;
    if attempts == 0 {
        let (st, app) = (st.clone(), app.to_string());
        tokio::spawn(async move { if let Err(e) = deliver_app(&st, &app).await { tracing::warn!(app, error = %e, "identity events not delivered now: the leading replica retries"); } });
    }
    Ok(id)
}

/// The wait before attempt `attempts + 1`: 10 s, doubling, at most an hour.
pub fn backoff(attempts: i64) -> Duration {
    let n = attempts.clamp(1, 20) as u32 - 1;
    Duration::from_secs(10u64.saturating_mul(1u64 << n.min(16))).min(Duration::from_secs(3600))
}

/// What delivering an event came to.
enum Outcome { Done, Again(String) }

/// Delivers what is due in this realm, app by app (each app's events in order). Returns how many were answered.
pub async fn deliver(st: &Arc<AppState>) -> anyhow::Result<usize> {
    if hook_url(st).is_none() { return Ok(0); }
    let apps: Vec<String> = sqlx::query_scalar("SELECT DISTINCT e.app FROM identity_events e WHERE e.status = 'queued' AND e.next_at <= $1 AND NOT EXISTS (SELECT 1 FROM identity_events o WHERE o.app = e.app AND o.status = 'queued' AND o.seq < e.seq)")
        .bind(chrono::Utc::now().to_rfc3339()).fetch_all(&st.db).await?;
    let mut n = 0;
    for app in apps { n += deliver_app(st, &app).await?; }
    Ok(n)
}

/// Delivers `app`'s due events in order, one replica at a time (a transaction-level lock per app): stops at the first that
/// is not answered for good (refused, failed, or answered 202), which waits for its next attempt and holds back the rest.
/// An access change goes to the access webhook when there is one (`target`), still in the app's order: a grant waits for
/// the role it names.
pub async fn deliver_app(st: &Arc<AppState>, app: &str) -> anyhow::Result<usize> {
    if hook_url(st).is_none() { return Ok(0); }
    let mut tx = st.db.begin().await?;
    let realm = st.realm().map(|r| r.as_str()).unwrap_or("");
    let got: bool = sqlx::query_scalar("SELECT pg_try_advisory_xact_lock(hashtext($1))").bind(format!("cori_identity:{realm}/{app}")).fetch_one(&mut *tx).await?;
    if !got { return Ok(0); }
    let mut done = 0;
    // the requests a delivery made done: Cori's own part follows (`api::request_settled`)
    let mut settled = Vec::new();
    for _ in 0..50 {
        let Some(r) = sqlx::query("SELECT id, event, body, attempts, next_at, occurred_at FROM identity_events WHERE app = $1 AND status = 'queued' ORDER BY seq LIMIT 1").bind(app).fetch_optional(&mut *tx).await? else { break };
        let (id, event, attempts, next_at, occurred_at): (String, String, i64, String, String) = (r.get("id"), r.get("event"), r.get("attempts"), r.get("next_at"), r.get("occurred_at"));
        let now = chrono::Utc::now();
        if next_at > now.to_rfc3339() { break; }
        let mut payload: Value = serde_json::from_str(&r.get::<String, _>("body")).unwrap_or(json!({}));
        // a role event names the app's client as it is when delivered: it may have been pending when the event was queued
        if event.starts_with("role.") {
            payload["client_id"] = json!(sqlx::query_scalar::<_, Option<String>>("SELECT client_id FROM apps WHERE id = $1").bind(app).fetch_optional(&mut *tx).await?.flatten());
        }
        let Some((url, secret)) = target(st, &event) else { break };
        let (outcome, status, note) = match post(st, &url, secret.as_deref(), &event, &id, &occurred_at, &payload).await {
            Ok(a) => { let o = settle(&mut tx, app, &event, &payload, &a).await?; (o, Some(a.status), a.note()) }
            Err(e) => (Outcome::Again(e.clone()), None, e),
        };
        match outcome {
            Outcome::Done => {
                sqlx::query("UPDATE identity_events SET status = 'done', attempts = attempts + 1, delivered_at = $1, last_status = $2, last_answer = $3 WHERE id = $4")
                    .bind(now.to_rfc3339()).bind(status.map(|s| s as i64)).bind(&note).bind(&id).execute(&mut *tx).await?;
                if let Some(request) = payload["request"].as_str() { settled.push(request.to_string()); }
                done += 1;
            }
            Outcome::Again(why) => {
                let first = chrono::DateTime::parse_from_rfc3339(&occurred_at).map(|d| d.with_timezone(&chrono::Utc)).unwrap_or(now);
                let dead = now - first > chrono::Duration::from_std(st.cfg.identity.give_up).unwrap_or_default();
                let next = now + chrono::Duration::from_std(backoff(attempts + 1)).unwrap_or_default();
                sqlx::query("UPDATE identity_events SET status = $1, attempts = attempts + 1, next_at = $2, last_status = $3, last_answer = $4 WHERE id = $5")
                    .bind(if dead { "dead" } else { "queued" }).bind(next.to_rfc3339()).bind(status.map(|s| s as i64)).bind(&why).bind(&id).execute(&mut *tx).await?;
                if dead {
                    if let Some(request) = payload["request"].as_str() {
                        sqlx::query("UPDATE access_requests SET status = 'given_up', answer = $1, updated_at = $2 WHERE id = $3 AND status IN ('sent', 'pending')").bind(&why).bind(now.to_rfc3339()).bind(request).execute(&mut *tx).await?;
                    }
                    tracing::warn!(app, event = %event, id = %id, "identity event given up: {why}");
                    let _ = crate::db::audit(&st.db, "cori", "identity.event.dead", app, json!({ "event": event, "id": id, "answer": why })).await;
                    continue;   // the next event of the app goes on
                }
                break;
            }
        }
    }
    tx.commit().await?;
    for request in settled {
        if let Err(e) = crate::api::request_settled(st, &request).await { tracing::warn!(request, error = %e, "Cori's part of a settled request failed"); }
    }
    Ok(done)
}

/// What an answer does: `app.created` binds the client (200) or leaves it pending (202); a role is confirmed or gone.
async fn settle(tx: &mut sqlx::Transaction<'static, sqlx::Postgres>, app: &str, event: &str, payload: &Value, a: &Answer) -> anyhow::Result<Outcome> {
    let now = chrono::Utc::now().to_rfc3339();
    let ok = (200..300).contains(&a.status) && a.status != 202;
    match event {
        "app.created" => {
            if a.status == 202 {
                sqlx::query("UPDATE apps SET client_status = 'pending', client_note = $1 WHERE id = $2").bind(a.note()).bind(app).execute(&mut **tx).await?;
                return Ok(Outcome::Again(a.note()));
            }
            if !ok {
                sqlx::query("UPDATE apps SET client_note = $1 WHERE id = $2").bind(format!("http {}: {}", a.status, a.note())).bind(app).execute(&mut **tx).await?;
                return Ok(Outcome::Again(format!("http {}: {}", a.status, a.note())));
            }
            let Some(cid) = a.body["client_id"].as_str().filter(|c| !c.is_empty()) else { return Ok(Outcome::Again("the answer has no `client_id`".into())) };
            bind(&mut **tx, app, cid, payload["origin"].as_str()).await?;
            sqlx::query("INSERT INTO audit (id, actor, action, object, at, details) VALUES ($1, 'cori', 'app.client', $2, $3, $4)").bind(uuid::Uuid::new_v4().to_string()).bind(app).bind(&now).bind(json!({ "client_id": cid, "after": "pending" }).to_string()).execute(&mut **tx).await?;
            Ok(Outcome::Done)
        }
        "role.created" | "role.updated" => {
            if !ok { return Ok(Outcome::Again(if a.status == 202 { a.note() } else { format!("http {}: {}", a.status, a.note()) })); }
            sqlx::query("UPDATE app_roles SET state = 'confirmed', confirmed_at = $1, updated_at = $1 WHERE app = $2 AND role = $3 AND state = 'pending'").bind(&now).bind(app).bind(payload["role"].as_str().unwrap_or("")).execute(&mut **tx).await?;
            Ok(Outcome::Done)
        }
        "role.deleted" => {
            if !ok { return Ok(Outcome::Again(if a.status == 202 { a.note() } else { format!("http {}: {}", a.status, a.note()) })); }
            sqlx::query("DELETE FROM app_roles WHERE app = $1 AND role = $2 AND state = 'deleting'").bind(app).bind(payload["role"].as_str().unwrap_or("")).execute(&mut **tx).await?;
            Ok(Outcome::Done)
        }
        e if PEOPLE_EVENTS.contains(&e) => settle_request(tx, payload, a).await,
        _ => Ok(if ok { Outcome::Done } else { Outcome::Again(if a.status == 202 { a.note() } else { format!("http {}: {}", a.status, a.note()) }) }),
    }
}

/// The people and access events, sent only to a receiver that lists them in its answer to `ping`.
pub const PEOPLE_EVENTS: &[&str] = &["member.invited", "member.roles", "member.removed", "invitation.cancelled", "invitation.resent", "access.granted", "access.revoked", "organization.updated"];

/// The questions the console and the command line may ask the receiver.
pub const QUESTIONS: &[&str] = &["organization.get", "people.list", "invitations.list", "access.list", "person.organizations"];

/// A refusal for good (a request the receiver will not carry out as it is), not a failure to try again.
fn refused_for_good(status: u16) -> bool { matches!(status, 400 | 403 | 404 | 409 | 410 | 422) }

/// What an answer to a people or access event does to the request it delivers (`payload.request`): `done`, `pending`
/// (delivered again later, unless the receiver settles it itself), `refused`; a failure keeps it `sent`, delivered again.
async fn settle_request(tx: &mut sqlx::Transaction<'static, sqlx::Postgres>, payload: &Value, a: &Answer) -> anyhow::Result<Outcome> {
    let now = chrono::Utc::now().to_rfc3339();
    let request = payload["request"].as_str().unwrap_or("");
    let ignored = a.body.get("ignored").is_some();
    let (status, outcome, result) = if ignored {
        ("refused", Outcome::Done, None)
    } else if a.status == 202 {
        if a.body["settles"].as_bool() == Some(true) { ("pending", Outcome::Done, None) } else { ("pending", Outcome::Again(a.note()), None) }
    } else if (200..300).contains(&a.status) {
        ("done", Outcome::Done, Some(a.body.to_string()))
    } else if refused_for_good(a.status) {
        ("refused", Outcome::Done, None)
    } else {
        ("sent", Outcome::Again(format!("http {}: {}", a.status, a.note())), None)
    };
    let said = if ignored { format!("the identity provider does not handle `{}`", payload["event"].as_str().unwrap_or("this")) } else if (200..300).contains(&a.status) && a.status != 202 { a.body["message"].as_str().unwrap_or("").to_string() } else { a.note() };
    sqlx::query("UPDATE access_requests SET status = $1, answer = $2, result = COALESCE($3, result), updated_at = $4 WHERE id = $5 AND status IN ('sent', 'pending')")
        .bind(status).bind(&said).bind(result).bind(&now).bind(request).execute(&mut **tx).await?;
    Ok(outcome)
}

/// What the hook's receiver handles besides apps and roles, as its answer to `ping` lists it
/// (`{ "capabilities": { "events": […], "questions": […], "settles": true } }`). A receiver that lists nothing gets app and
/// role events only, as before.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Capabilities { pub events: BTreeSet<String>, pub questions: BTreeSet<String>, pub settles: bool }

impl Capabilities {
    /// What an answer to `ping` lists, among the events and questions Cori knows.
    pub fn of(answer: &Value) -> Self {
        let c = &answer["capabilities"];
        let names = |k: &str, known: &[&str]| -> BTreeSet<String> { c[k].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).filter(|x| known.contains(x)).map(String::from).collect()).unwrap_or_default() };
        Self { events: names("events", PEOPLE_EVENTS), questions: names("questions", QUESTIONS), settles: c["settles"].as_bool().unwrap_or(false) }
    }
    pub fn handles(&self, event: &str) -> bool { self.events.contains(event) }
    pub fn answers(&self, question: &str) -> bool { self.questions.contains(question) }
    /// These, and the access changes (`ACCESS_EVENTS`), which the access webhook takes.
    pub fn with_access(mut self) -> Self { self.events.extend(ACCESS_EVENTS.iter().map(|e| e.to_string())); self }
    pub fn json(&self) -> Value { json!({ "events": self.events, "questions": self.questions, "settles": self.settles }) }
}

/// How long the receiver's capabilities are reused (an hour; `CORI_IDENTITY_CAPS_SECS`, a test's), or, when its `ping`
/// failed, before it is asked again (a minute at most).
fn caps_fresh() -> Duration { Duration::from_secs(std::env::var("CORI_IDENTITY_CAPS_SECS").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(3600)) }
const CAPS_RETRY: Duration = Duration::from_secs(60);

/// What the receiver handles, from its answer to a signed `ping`, asked again every hour (a minute after a failure).
pub async fn capabilities(st: &AppState) -> Capabilities {
    let Some(url) = hook_url(st) else { return Capabilities::default() };
    if let Some((at, c, ok)) = st.hook_caps.read().await.clone() { if at.elapsed() < if ok { caps_fresh() } else { CAPS_RETRY.min(caps_fresh()) } { return c; } }
    let (c, ok) = match ping_url(&url, st.cfg.client_hook_secret.as_deref()).await {
        Ok(a) if (200..300).contains(&a.status) => (Capabilities::of(&a.body), true),
        Ok(a) => { tracing::warn!(status = a.status, "the identity hook's ping was refused: {}", a.note()); (Capabilities::default(), false) }
        Err(e) => { tracing::warn!(error = %e, "the identity hook did not answer its ping"); (Capabilities::default(), false) }
    };
    *st.hook_caps.write().await = Some((std::time::Instant::now(), c.clone(), ok));
    c
}

/// The organization's own directory of its people (`CORI_IDENTITY_PEOPLE_URL`), under builtin auth: it answers
/// `people.list` in place of the hook's receiver, which keeps the apps, the roles, the changes and the other questions.
pub fn people_url(st: &AppState) -> Option<String> {
    if !matches!(st.cfg.auth, crate::auth::AuthMode::Builtin { .. }) { return None; }
    st.cfg.people_url.clone()
}

/// The changes to an app's access, which a webhook of the organization's own may take in the receiver's place.
pub const ACCESS_EVENTS: &[&str] = &["access.granted", "access.revoked"];

/// The organization's own webhook for apps' access (`CORI_IDENTITY_ACCESS_URL`), under builtin auth: it takes
/// `access.granted` and `access.revoked` (to members, and to guests once an admin approved) in place of the hook's receiver,
/// which keeps the apps, the roles and the rest. No `ping` asks it what it handles: the setting says it.
pub fn access_url(st: &AppState) -> Option<String> {
    if !matches!(st.cfg.auth, crate::auth::AuthMode::Builtin { .. }) { return None; }
    st.cfg.access_url.clone()
}

/// Where `event` is delivered, and the secret it is signed with: an access change to the access webhook when there is one,
/// signed with its own secret; anything else to the hook's receiver. None without a hook.
fn target(st: &AppState, event: &str) -> Option<(String, Option<String>)> {
    if ACCESS_EVENTS.contains(&event) && let Some(url) = access_url(st) { return Some((url, st.cfg.access_secret.clone())); }
    hook_url(st).map(|url| (url, st.cfg.client_hook_secret.clone()))
}

/// What Cori sends and asks of the organization: what the receiver lists (`capabilities`), and the access changes when the
/// access webhook takes them. What the console offers follows it.
pub async fn handled(st: &AppState) -> Capabilities {
    let caps = capabilities(st).await;
    if access_url(st).is_some() { caps.with_access() } else { caps }
}

/// Whether `question` is answered: `people.list` by the directory of people when there is one, any question by the hook's
/// receiver when it lists it.
pub async fn answers(st: &AppState, question: &str) -> bool {
    if question == "people.list" && people_url(st).is_some() { return true; }
    capabilities(st).await.answers(question)
}

/// `question` asked of `url` as the server asks it: a signed POST (`x-cori-question`, the question in `event`; the realm
/// under a tenancy). Its answer, whatever its status.
pub async fn ask_url(url: &str, secret: Option<&str>, question: &str, args: Value, realm: Option<&str>) -> Result<Answer, String> {
    let (id, now) = (uuid::Uuid::new_v4().to_string(), chrono::Utc::now().to_rfc3339());
    let mut body = if args.is_object() { args } else { json!({}) };
    body["id"] = json!(id); body["event"] = json!(question); body["question"] = json!(true); body["occurred_at"] = json!(now); body["at"] = json!(now);
    if let Some(r) = realm { body["realm"] = json!(r); }
    let text = body.to_string();
    let mut req = cori_engine::http::builder().timeout(Duration::from_secs(10)).build().map_err(|e| e.to_string())?
        .post(url).header("content-type", "application/json").header("x-cori-event", question).header("x-cori-question", question).header("x-cori-delivery", &id).header("user-agent", "cori-client-hook/2");
    if let Some(secret) = secret { req = req.header("x-cori-signature", sign(secret, &text)); }
    let r = req.body(text).send().await.map_err(|e| format!("{url}: {e}"))?;
    let status = r.status().as_u16();
    Ok(Answer { status, body: r.json().await.unwrap_or(Value::Null) })
}

/// Asks `question` (`QUESTIONS`), at once, never queued nor retried: `people.list` of the directory of people when there is
/// one, else of the hook's receiver. `Ok(None)` when the receiver does not answer that question: not among its
/// capabilities, a 404 or a 501, an `ignored` answer. The directory answers by its setting: anything but a 2xx is its failure.
pub async fn ask(st: &AppState, question: &str, args: Value) -> Result<Option<Value>, String> {
    let realm = st.realm().map(|r| r.as_str().to_string());
    if question == "people.list" && let Some(url) = people_url(st) {
        let a = ask_url(&url, st.cfg.people_secret.as_deref(), question, args, realm.as_deref()).await.map_err(|e| format!("the directory of people {e}"))?;
        if (200..300).contains(&a.status) { return Ok(Some(a.body)); }
        return Err(format!("the directory of people ({url}) did not answer `{question}`: http {}: {}", a.status, a.note()));
    }
    let Some(url) = hook_url(st) else { return Ok(None) };
    if !capabilities(st).await.answers(question) { return Ok(None); }
    let a = ask_url(&url, st.cfg.client_hook_secret.as_deref(), question, args, realm.as_deref()).await.map_err(|e| format!("identity hook {e}"))?;
    if a.body.get("ignored").is_some() || matches!(a.status, 404 | 501) { return Ok(None); }
    if (200..300).contains(&a.status) { return Ok(Some(a.body)); }
    Err(format!("the identity provider did not answer `{question}`: http {}: {}", a.status, a.note()))
}

/// `ask`, its answer reused for `fresh` within this process and realm; `forget_answers` drops them after a change.
pub async fn ask_cached(st: &AppState, question: &str, args: Value, fresh: Duration) -> Result<Option<Value>, String> {
    let key = format!("{}/{question}/{args}", st.realm().map(|r| r.as_str()).unwrap_or(""));
    if let Some((at, v)) = st.answers.lock().unwrap_or_else(|e| e.into_inner()).get(&key).cloned() { if at.elapsed() < fresh { return Ok(Some(v)); } }
    let v = ask(st, question, args).await?;
    if let Some(v) = &v { st.answers.lock().unwrap_or_else(|e| e.into_inner()).insert(key, (std::time::Instant::now(), v.clone())); }
    Ok(v)
}

/// Requests the receiver said it would settle itself (`202` with `settles`) and has not for `CORI_IDENTITY_SETTLE_DAYS`
/// (30): given up, and shown so. Returns how many.
pub async fn expire_requests(st: &AppState) -> anyhow::Result<u64> {
    let days: i64 = std::env::var("CORI_IDENTITY_SETTLE_DAYS").ok().and_then(|v| v.trim().parse().ok()).filter(|d| *d > 0).unwrap_or(30);
    let now = chrono::Utc::now();
    let before = (now - chrono::Duration::days(days)).to_rfc3339();
    let n = sqlx::query("UPDATE access_requests SET status = 'given_up', answer = $1, updated_at = $2 WHERE status = 'pending' AND updated_at < $3")
        .bind(format!("the identity provider did not settle it within {days} days")).bind(now.to_rfc3339()).bind(&before).execute(&st.db).await?.rows_affected();
    Ok(n)
}

/// Forgets this realm's cached answers (a change was just asked for here).
pub fn forget_answers(st: &AppState) {
    let prefix = format!("{}/", st.realm().map(|r| r.as_str()).unwrap_or(""));
    st.answers.lock().unwrap_or_else(|e| e.into_inner()).retain(|k, _| !k.starts_with(&prefix));
}

/// Whether `app` still has events the hook has not answered for good (an earlier app of the same id being deleted).
pub async fn undelivered(st: &AppState, app: &str) -> anyhow::Result<bool> {
    Ok(sqlx::query("SELECT 1 FROM identity_events WHERE app = $1 AND status = 'queued' LIMIT 1").bind(app).fetch_optional(&st.db).await?.is_some())
}

/// The app is gone: its queued events are dropped, its roles forgotten, and `app.deleted` queued (with its client, if it
/// had one yet: the IAM may be creating it).
pub async fn deprovision(st: &Arc<AppState>, app: &str, client_id: Option<&str>) -> anyhow::Result<()> {
    if hook_url(st).is_none() { return Ok(()); }
    sqlx::query("UPDATE identity_events SET status = 'cancelled' WHERE app = $1 AND status = 'queued'").bind(app).execute(&st.db).await?;
    sqlx::query("DELETE FROM app_roles WHERE app = $1").bind(app).execute(&st.db).await?;
    enqueue(st, app, "app.deleted", &json!({ "app": app, "client_id": client_id })).await?;
    Ok(())
}

/// Tells the IAM of `app`'s roles (role events on): a role it was not told of (`role.created`), one whose description or
/// note changed (`role.updated`), one being deleted that came back (`role.created` again, after the `role.deleted` already
/// queued). A role it knows and that is declared again is no longer on its way out. Returns the roles created.
pub async fn sync_roles(st: &Arc<AppState>, app: &str, roles: &BTreeMap<String, RoleDecl>) -> anyhow::Result<Vec<String>> {
    if !role_events(st) { return Ok(vec![]); }
    let now = chrono::Utc::now().to_rfc3339();
    let mut created = Vec::new();
    for (role, decl) in roles {
        let row = sqlx::query("SELECT state, description, note FROM app_roles WHERE app = $1 AND role = $2").bind(app).bind(role).fetch_optional(&st.db).await?;
        let body = json!({ "app": app, "role": role, "description": decl.description, "note": decl.note });
        match row {
            None => {
                sqlx::query("INSERT INTO app_roles (app, role, description, note, state, declared_at, updated_at) VALUES ($1, $2, $3, $4, 'pending', $5, $5)").bind(app).bind(role).bind(&decl.description).bind(&decl.note).bind(&now).execute(&st.db).await?;
                enqueue(st, app, "role.created", &body).await?;
                created.push(role.clone());
            }
            Some(r) if r.get::<String, _>("state") == "deleting" => {
                sqlx::query("UPDATE app_roles SET state = 'pending', description = $1, note = $2, undeclared_since = NULL, confirmed_at = NULL, updated_at = $3 WHERE app = $4 AND role = $5").bind(&decl.description).bind(&decl.note).bind(&now).bind(app).bind(role).execute(&st.db).await?;
                enqueue(st, app, "role.created", &body).await?;
                created.push(role.clone());
            }
            Some(r) => {
                let changed = r.get::<Option<String>, _>("description") != decl.description || r.get::<Option<String>, _>("note") != decl.note;
                sqlx::query("UPDATE app_roles SET description = $1, note = $2, undeclared_since = NULL, updated_at = $3 WHERE app = $4 AND role = $5").bind(&decl.description).bind(&decl.note).bind(&now).bind(app).bind(role).execute(&st.db).await?;
                if changed { enqueue(st, app, "role.updated", &body).await?; }
            }
        }
    }
    Ok(created)
}

/// The roles every version of `app` it keeps declares (the live one alone with `roles_at_promote`), and those it was
/// created with.
pub async fn kept_roles(st: &AppState, app: &str) -> anyhow::Result<BTreeMap<String, RoleDecl>> {
    let rec = sqlx::query("SELECT roles, promoted_hash FROM apps WHERE id = $1").bind(app).fetch_optional(&st.db).await?;
    let Some(rec) = rec else { return Ok(BTreeMap::new()) };
    let mut out: BTreeMap<String, RoleDecl> = rec.get::<Option<String>, _>("roles").and_then(|r| serde_json::from_str::<Vec<String>>(&r).ok()).unwrap_or_default().into_iter().map(|r| (r, RoleDecl::default())).collect();
    let rows = if st.cfg.identity.roles_at_promote {
        sqlx::query("SELECT roles, workflows, cori_toml FROM versions WHERE app = $1 AND hash = $2").bind(app).bind(rec.get::<Option<String>, _>("promoted_hash").unwrap_or_default()).fetch_all(&st.db).await?
    } else {
        sqlx::query("SELECT roles, workflows, cori_toml FROM versions WHERE app = $1").bind(app).fetch_all(&st.db).await?
    };
    for r in rows { for (role, decl) in version_roles_of(&r) { let e = out.entry(role).or_default(); if decl != RoleDecl::default() { *e = decl; } } }
    let ignore = deployment_roles(st);
    out.retain(|r, _| !ignore.contains(r));
    Ok(out)
}

/// The roles version `hash` records (`version_roles_of`).
pub async fn recorded_roles(st: &AppState, hash: &str) -> anyhow::Result<BTreeMap<String, RoleDecl>> {
    let ignore = deployment_roles(st);
    Ok(match sqlx::query("SELECT roles, workflows, cori_toml FROM versions WHERE hash = $1").bind(hash).fetch_optional(&st.db).await? {
        Some(r) => version_roles_of(&r).into_iter().filter(|(k, _)| !ignore.contains(k)).collect(),
        None => BTreeMap::new(),
    })
}

/// The roles a person may try `app`'s releases with (`api::sessions`): those any of its versions records and those it was
/// created with, the deployment's own aside. A release that never went live counts with `roles_at_promote` too: the
/// identity provider is never told of such a choice.
pub async fn release_roles(st: &AppState, app: &str) -> anyhow::Result<std::collections::BTreeSet<String>> {
    let created = sqlx::query_scalar::<_, Option<String>>("SELECT roles FROM apps WHERE id = $1").bind(app).fetch_optional(&st.db).await?.flatten();
    let mut out: std::collections::BTreeSet<String> = created.and_then(|r| serde_json::from_str::<Vec<String>>(&r).ok()).unwrap_or_default().into_iter().collect();
    for r in sqlx::query("SELECT roles, workflows, cori_toml FROM versions WHERE app = $1").bind(app).fetch_all(&st.db).await? { out.extend(version_roles_of(&r).into_keys()); }
    let ignore = deployment_roles(st);
    out.retain(|r| !ignore.contains(r));
    Ok(out)
}

/// The roles a version row records (`versions.roles`), else those its cori.toml and workflows name (a version deployed before
/// they were recorded).
fn version_roles_of(r: &sqlx::postgres::PgRow) -> BTreeMap<String, RoleDecl> {
    if let Some(m) = r.get::<Option<String>, _>("roles").and_then(|j| serde_json::from_str::<BTreeMap<String, RoleDecl>>(&j).ok()) { return m; }
    let wfs: Vec<crate::bundles::WorkflowEntry> = serde_json::from_str(&r.get::<String, _>("workflows")).unwrap_or_default();
    let compiled: Vec<&cori_compiler::Compiled> = wfs.iter().map(|w| &w.compiled).collect();
    cori_compiler::roles::Roles::of(&r.get::<String, _>("cori_toml"), &compiled, &[]).all(&[])
}

/// The roles of a version just compiled: those its cori.toml declares and those it uses, its stores' role files included.
pub fn version_roles(cori_toml: &str, workflows: &[crate::bundles::WorkflowEntry], bundle_dir: &std::path::Path) -> cori_compiler::roles::Roles {
    let compiled: Vec<&cori_compiler::Compiled> = workflows.iter().map(|w| &w.compiled).collect();
    cori_compiler::roles::Roles::of(cori_toml, &compiled, &cori_compiler::roles::store_roles(bundle_dir))
}

/// Roles no kept version declares any more (role events on), in every app: see `sweep_app`. Returns how many were sent on
/// their way out.
pub async fn sweep_roles(st: &Arc<AppState>) -> anyhow::Result<usize> {
    if !role_events(st) { return Ok(0); }
    let mut out = 0;
    for app in sqlx::query_scalar::<_, String>("SELECT DISTINCT app FROM app_roles WHERE state IN ('pending', 'confirmed')").fetch_all(&st.db).await? { out += sweep_app(st, &app).await?; }
    Ok(out)
}

/// `app`'s roles no kept version declares any more: their `undeclared_since` starts, and once it is older than
/// `CORI_ROLE_DELETE_GRACE_SECS` they go (`role.deleted`); a role declared again stops on its way out. Run every few minutes
/// by the leading replica, and right after a version of the app is deleted.
pub async fn sweep_app(st: &Arc<AppState>, app: &str) -> anyhow::Result<usize> {
    if !role_events(st) { return Ok(0); }
    let now = chrono::Utc::now();
    let grace = chrono::Duration::from_std(st.cfg.identity.delete_grace).unwrap_or_default();
    let kept = kept_roles(st, app).await?;
    let mut out = 0;
    for r in sqlx::query("SELECT role, undeclared_since FROM app_roles WHERE app = $1 AND state IN ('pending', 'confirmed')").bind(app).fetch_all(&st.db).await? {
        let (role, since): (String, Option<String>) = (r.get("role"), r.get("undeclared_since"));
        if kept.contains_key(&role) {
            if since.is_some() { sqlx::query("UPDATE app_roles SET undeclared_since = NULL WHERE app = $1 AND role = $2").bind(app).bind(&role).execute(&st.db).await?; }
            continue;
        }
        let since = match since.and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok()).map(|d| d.with_timezone(&chrono::Utc)) {
            Some(s) => s,
            None => {
                sqlx::query("UPDATE app_roles SET undeclared_since = $1 WHERE app = $2 AND role = $3").bind(now.to_rfc3339()).bind(app).bind(&role).execute(&st.db).await?;
                now
            }
        };
        if now - since < grace { continue; }
        sqlx::query("UPDATE app_roles SET state = 'deleting', updated_at = $1 WHERE app = $2 AND role = $3").bind(now.to_rfc3339()).bind(app).bind(&role).execute(&st.db).await?;
        enqueue(st, app, "role.deleted", &json!({ "app": app, "role": role })).await?;
        out += 1;
    }
    Ok(out)
}

/// Apps whose front moved since their client was told (a new fronts URL): `app.updated` with the new redirect URIs and
/// origin. An app recorded before the origin was (`client_origin` unset) takes the current one silently.
pub async fn sync_origins(st: &Arc<AppState>) -> anyhow::Result<usize> {
    if hook_url(st).is_none() { return Ok(0); }
    let mut n = 0;
    for r in sqlx::query("SELECT id, owner, client_id, client_origin FROM apps WHERE client_id IS NOT NULL").fetch_all(&st.db).await? {
        let (app, owner, client_id, told): (String, Option<String>, String, Option<String>) = (r.get("id"), r.get("owner"), r.get("client_id"), r.get("client_origin"));
        let origin = st.front_origin(&app);
        if told.as_deref() == Some(origin.as_str()) { continue; }
        sqlx::query("UPDATE apps SET client_origin = $1 WHERE id = $2").bind(&origin).bind(&app).execute(&st.db).await?;
        if told.is_none() { continue; }
        enqueue(st, &app, "app.updated", &json!({ "app": app, "client_id": client_id, "owner": owner, "redirect_uris": [callback_url(st, &app)], "post_logout_redirect_uris": [crate::names::post_logout_url(&origin, &app)], "origin": origin })).await?;
        n += 1;
    }
    Ok(n)
}

/// `app.updated` for `app` as it is now, its URIs and its owner: told when it is handed to another owner, who may then
/// grant its roles. Nothing while its client is pending (the next `app.created` names the owner).
pub async fn tell_updated(st: &Arc<AppState>, app: &str) -> anyhow::Result<()> {
    if let Some(body) = updated_body(st, app).await? { enqueue(st, app, "app.updated", &body).await?; }
    Ok(())
}

/// What `app.updated` says of `app` now; None without a hook, or while its client is pending.
pub async fn updated_body(st: &AppState, app: &str) -> anyhow::Result<Option<Value>> {
    if hook_url(st).is_none() { return Ok(None); }
    let Some(r) = sqlx::query("SELECT owner, client_id FROM apps WHERE id = $1").bind(app).fetch_optional(&st.db).await? else { return Ok(None) };
    let (owner, client_id): (Option<String>, Option<String>) = (r.get("owner"), r.get("client_id"));
    let Some(client_id) = client_id else { return Ok(None) };
    let origin = st.front_origin(app);
    Ok(Some(json!({ "app": app, "client_id": client_id, "owner": owner, "redirect_uris": [callback_url(st, app)], "post_logout_redirect_uris": [crate::names::post_logout_url(&origin, app)], "origin": origin })))
}

/// The roles of `app` the IAM has not confirmed among `roles`: what holds back a release when
/// `CORI_PROMOTE_REQUIRES_CONFIRMED_ROLES` is on.
pub async fn unconfirmed(st: &AppState, app: &str, roles: &[String]) -> anyhow::Result<Vec<String>> {
    let confirmed: Vec<String> = sqlx::query_scalar("SELECT role FROM app_roles WHERE app = $1 AND state = 'confirmed'").bind(app).fetch_all(&st.db).await?;
    Ok(roles.iter().filter(|r| !confirmed.contains(r)).cloned().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_up_to_an_hour() {
        assert_eq!(backoff(1), Duration::from_secs(10));
        assert_eq!(backoff(2), Duration::from_secs(20));
        assert_eq!(backoff(5), Duration::from_secs(160));
        assert_eq!(backoff(12), Duration::from_secs(3600));
        assert_eq!(backoff(99), Duration::from_secs(3600));
    }

    #[test]
    fn a_receiver_lists_what_it_handles_among_what_cori_knows() {
        let c = Capabilities::of(&json!({ "capabilities": { "events": ["access.granted", "member.invited", "x.unknown"], "questions": ["people.list", "secrets.dump"], "settles": true } }));
        assert!(c.handles("access.granted") && c.handles("member.invited") && !c.handles("x.unknown") && !c.handles("member.removed"));
        assert!(c.answers("people.list") && !c.answers("secrets.dump"));
        assert!(c.settles);
        // today's receivers answer `ping` with nothing, or `ignored`: apps and roles only
        assert_eq!(Capabilities::of(&json!({})), Capabilities::default());
        assert_eq!(Capabilities::of(&json!({ "ignored": "ping" })), Capabilities::default());
    }

    #[test]
    fn the_access_webhook_adds_the_access_changes_to_what_the_receiver_handles() {
        // a receiver of apps and roles alone: with the access webhook, owners grant their apps' roles, and nothing else opens
        let c = Capabilities::default().with_access();
        assert!(c.handles("access.granted") && c.handles("access.revoked"));
        assert!(!c.handles("member.invited") && !c.handles("member.removed") && c.questions.is_empty() && !c.settles);
        // a receiver of people keeps what it lists
        let c = Capabilities::of(&json!({ "capabilities": { "events": ["member.roles"], "questions": ["people.list"], "settles": true } })).with_access();
        assert!(c.handles("member.roles") && c.handles("access.granted") && c.answers("people.list") && c.settles);
    }

    #[test]
    fn a_notification_is_verified_against_its_body() {
        let body = br#"{"id":"n1","event":"person.removed"}"#;
        let good = sign("s3cret", std::str::from_utf8(body).unwrap());
        assert!(verify("s3cret", body, &good));
        assert!(!verify("other", body, &good), "another secret");
        assert!(!verify("s3cret", br#"{"id":"n2","event":"person.removed"}"#, &good), "another body");
        assert!(!verify("s3cret", body, "") && !verify("s3cret", body, "sha256=zz") && !verify("s3cret", body, &good.replace("sha256=", "")));
    }

    #[test]
    fn some_refusals_are_for_good_others_are_tried_again() {
        for s in [400, 403, 404, 409, 410, 422] { assert!(refused_for_good(s), "{s}"); }
        for s in [401, 408, 429, 500, 502, 503] { assert!(!refused_for_good(s), "{s}"); }
    }

    #[tokio::test]
    async fn an_answer_settles_the_request_it_delivers() {
        let Some(db) = crate::db::test_db().await else { eprintln!("CORI_STORE_DATABASE_URL not set: skipped"); return };
        let request = |id: &str| sqlx::query("INSERT INTO access_requests (id, kind, event, roles, status, asked_by, asked_at, updated_at) VALUES ($1, 'grant', 'access.granted', '[]', 'sent', 'u', 'now', 'now')").bind(id.to_string());
        let status = async |db: &crate::db::Db, id: &str| -> (String, Option<String>) { let r = sqlx::query("SELECT status, answer FROM access_requests WHERE id = $1").bind(id).fetch_one(db).await.unwrap(); (r.get("status"), r.get("answer")) };
        let answer = |status: u16, body: Value| Answer { status, body };
        let cases: [(&str, Answer, &str, bool); 6] = [
            ("done", answer(200, json!({ "invitation": "i1" })), "done", true),
            ("pending", answer(202, json!({ "message": "the IAM is on it" })), "pending", false),
            ("settles", answer(202, json!({ "message": "a ticket", "settles": true })), "pending", true),
            ("refused", answer(409, json!({ "message": "already a member" })), "refused", true),
            ("ignored", answer(200, json!({ "ignored": "access.granted" })), "refused", true),
            ("failed", answer(503, json!({ "message": "down" })), "sent", false),
        ];
        for (id, a, want, finished) in cases {
            request(id).execute(&db).await.unwrap();
            let mut tx = db.begin().await.unwrap();
            let o = settle_request(&mut tx, &json!({ "request": id, "event": "access.granted" }), &a).await.unwrap();
            tx.commit().await.unwrap();
            assert_eq!((status(&db, id).await.0.as_str(), matches!(o, Outcome::Done)), (want, finished), "{id}");
        }
        assert_eq!(status(&db, "refused").await.1.as_deref(), Some("already a member"), "the receiver's reason is kept");
        assert!(status(&db, "ignored").await.1.unwrap().contains("does not handle"));
    }

    #[test]
    fn deliveries_are_signed_over_their_body() {
        // what a receiver recomputes: `printf '{"a":1}' | openssl dgst -sha256 -hmac s3cret`
        assert_eq!(sign("s3cret", "{\"a\":1}"), "sha256=5910e62016ef5034272c926c27071992a465c2335cecf41851bda071577f4f6d");
    }
}
