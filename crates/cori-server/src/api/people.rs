// SPDX-License-Identifier: Apache-2.0
//! The people of the organization and their access, for the console and the command line (people only: agent sessions and
//! apps' tokens never reach these routes).
//! - Who they are: the identity provider's answer to `people.list` (the identity hook's questions, `client_hook::ask`, or a
//!   directory of the organization's own), else those the server has seen (`people_seen`), recorded from their tokens'
//!   claims as they call (`person_seen`), so that the ids it keeps (an app's owner and builders, a run's user, who decided,
//!   the audit log) read as names. What the answer leaves out of a person, the server fills in from what it saw.
//! - What changes: every invitation, member's roles, removal, grant or revocation of an app's roles is a request
//!   (`access_requests`), delivered to the identity provider as a hook event (`client_hook::PEOPLE_EVENTS`) when its
//!   receiver handles it (an app's access: the organization's access webhook when it has one, `client_hook::access_url`),
//!   and settled by its answer. The provider decides: Cori asks, in the name of who asked (`by`).
use super::*;
use crate::client_hook::{self, Capabilities};
use std::time::Duration;

/// How often one process writes that a person called: often enough to say when they were last seen, and no write per
/// request. Roles that changed are written at once.
const SEEN_EVERY: std::time::Duration = std::time::Duration::from_secs(600);

/// Records that `who` called: their address and name as their token gives them. Agent sessions are their person's, who
/// called with a token of their own first; nothing anonymous is recorded. A failure to write is logged, never the caller's.
pub(crate) async fn person_seen(st: &AppState, who: &Principal) {
    // nobody to name under `off` auth; under a tenancy, a person is recorded in the organization they act in only
    if matches!(st.cfg.auth, crate::auth::AuthMode::Off) || (st.tenancy.is_some() && st.realm().is_none()) { return; }
    record_person(&st.db, &st.seen, who).await
}

/// `person_seen`, on the realm's database and with this process's memory of whom it last wrote when.
async fn record_person(db: &db::Db, seen: &std::sync::Mutex<std::collections::HashMap<String, std::time::Instant>>, who: &Principal) {
    if who.is_agent() || who.sub.is_empty() { return; }
    // the roles the token carries: member roles, and the roles in each app's client (`Principal::client_roles`)
    let roles = serde_json::to_string(who.platform_roles()).ok();
    let app_roles = who.client_roles().map(|by_client| json!(by_client).to_string());
    // once a window per person and set of roles: a person whose roles changed is written at once, and so is one whose roles
    // came back to a set written earlier in the window (a role given, taken back, given again)
    let held = { use std::hash::{Hash, Hasher}; let mut h = std::collections::hash_map::DefaultHasher::new(); (&roles, &app_roles).hash(&mut h); h.finish() };
    let person = format!("people/{}/{}", db.realm().map(|r| r.as_str().to_string()).unwrap_or_default(), who.sub);
    let key = format!("{person}/{held:x}");
    {
        let mut seen = seen.lock().unwrap_or_else(|e| e.into_inner());
        let last = seen.get(&person).copied();
        if seen.get(&key).is_some_and(|at| at.elapsed() < SEEN_EVERY && last.is_none_or(|l| l <= *at)) { return; }
        let now = std::time::Instant::now();
        seen.insert(key, now);
        seen.insert(person, now);
    }
    let clean = |s: &str| -> String { s.chars().filter(|c| !c.is_control()).take(200).collect() };
    let email = who.email.as_deref().map(clean).filter(|e| !e.is_empty());
    let name = who.claims.get("name").and_then(|n| n.as_str()).map(clean).filter(|n| !n.trim().is_empty());
    let now = chrono::Utc::now().to_rfc3339();
    let r = sqlx::query("INSERT INTO people_seen (sub, email, name, first_at, last_at, roles, app_roles) VALUES ($1, $2, $3, $4, $4, $5, $6) ON CONFLICT ON CONSTRAINT people_seen_pkey DO UPDATE SET last_at = EXCLUDED.last_at, email = COALESCE(EXCLUDED.email, people_seen.email), name = COALESCE(EXCLUDED.name, people_seen.name), roles = COALESCE(EXCLUDED.roles, people_seen.roles), app_roles = COALESCE(EXCLUDED.app_roles, people_seen.app_roles)")
        .bind(&who.sub).bind(&email).bind(&name).bind(&now).bind(&roles).bind(&app_roles).execute(db).await;
    if let Err(e) = r { tracing::warn!(error = %e, "could not record a person's call"); }
}

/// A person as the server has seen them, with the roles their last token carried (`grants`: by app client).
fn seen_json(r: &sqlx::postgres::PgRow) -> Value {
    let parse = |k: &str| r.get::<Option<String>, _>(k).and_then(|v| serde_json::from_str::<Value>(&v).ok());
    json!({ "id": r.get::<String, _>("sub"), "email": r.get::<Option<String>, _>("email"), "name": r.get::<Option<String>, _>("name"), "first_seen": r.get::<String, _>("first_at"), "last_seen": r.get::<String, _>("last_at"),
        "roles": parse("roles").unwrap_or_else(|| json!([])), "grants": parse("app_roles").unwrap_or_else(|| json!({})) })
}

/// What role `role` gives in Cori, from the server's role lists: the powers of its holders (`admin`, `operator`,
/// `auditor`, `billing`, `builder`, `store_admin`, `approver`); none for a role that only opens apps.
pub(crate) fn powers_of(st: &AppState, role: &str) -> Vec<&'static str> {
    let r = crate::auth::platform_roles();
    let mut out: Vec<&'static str> = [("admin", &r.admin), ("operator", &r.operator), ("auditor", &r.auditor), ("billing", &r.billing), ("builder", &r.builder), ("store_admin", &r.store_admin)]
        .into_iter().filter(|(_, list)| list.iter().any(|x| x == role)).map(|(p, _)| p).collect();
    if st.settings.approver_roles.iter().any(|x| x == role) { out.push("approver"); }
    out
}

/// How long the provider's answers are reused within a process: the people for 30 s, the organization for a minute.
const PEOPLE_FRESH: Duration = Duration::from_secs(30);
const ORG_FRESH: Duration = Duration::from_secs(60);

/// The apps as access needs them: owner, builders, sign-in client.
struct AppAccess { id: String, owner: Option<String>, builders: Vec<String>, client_id: Option<String> }

async fn apps_of(st: &AppState) -> Result<Vec<AppAccess>, ApiError> {
    Ok(sqlx::query("SELECT id, owner, builders, client_id FROM apps ORDER BY id").fetch_all(&st.db).await?.iter().map(|r| AppAccess {
        id: r.get("id"), owner: r.get("owner"), builders: r.get::<Option<String>, _>("builders").and_then(|b| serde_json::from_str(&b).ok()).unwrap_or_default(), client_id: r.get("client_id"),
    }).collect())
}

/// The people the identity provider lists (`people.list`, every page: its receiver's, or the organization's own directory),
/// None when it does not answer that question. Someone it names by no `id` is left out: Cori knows people by the subject
/// of their tokens.
async fn directory_people(st: &AppState, by: &str) -> Result<Option<Vec<Value>>, String> {
    let (mut out, mut unnamed) = (Vec::new(), 0);
    let mut cursor: Option<String> = None;
    for _ in 0..50 {
        let args = json!({ "by": by, "cursor": cursor });
        let Some(page) = client_hook::ask_cached(st, "people.list", args, PEOPLE_FRESH).await? else { return Ok(None) };
        let people = page["people"].as_array().ok_or_else(|| "the answer to `people.list` has no `people`".to_string())?;
        unnamed += people.iter().filter(|p| !named(p)).count();
        out.extend(people.iter().filter(|p| named(p)).cloned());
        match page["next"].as_str().filter(|n| !n.is_empty()) { Some(n) => cursor = Some(n.to_string()), None => break }
    }
    if unnamed > 0 { tracing::warn!(unnamed, "people.list names people without an `id`: left out"); }
    Ok(Some(out))
}

/// Whether the directory names a person by their id, the subject (`sub`) of their tokens.
fn named(p: &Value) -> bool { p["id"].as_str().is_some_and(|id| !id.trim().is_empty()) }

/// What the directory leaves out of a person, from what the server has seen of them: when they were last seen, and their
/// address, name, member roles and apps' roles as their last token carried them. A directory that knows who the people
/// are, and not their roles, still shows them.
fn fill_from_seen(p: &mut Value, seen: &BTreeMap<String, Value>) {
    let Some(s) = p["id"].as_str().and_then(|id| seen.get(id)) else { return };
    p["last_seen"] = s["last_seen"].clone();
    for k in ["email", "name", "roles", "grants"] { if p[k].is_null() { p[k] = s[k].clone(); } }
}

/// The names and addresses of `ids` as the server has seen them (`people_seen`), by id: what a person is shown of the
/// others in their own apps (who asked for an approval) when the organization's people are not theirs to read.
pub(crate) async fn names_seen(st: &AppState, ids: &[String]) -> Result<BTreeMap<String, Value>, ApiError> {
    if ids.is_empty() { return Ok(BTreeMap::new()); }
    let rows = sqlx::query("SELECT sub, name, email FROM people_seen WHERE sub = ANY($1)").bind(ids).fetch_all(&st.db).await?;
    Ok(rows.iter().map(|r| (r.get::<String, _>("sub"), json!({ "name": r.get::<Option<String>, _>("name"), "email": r.get::<Option<String>, _>("email") }))).collect())
}

/// What the server has seen of people, by id.
async fn seen_by_id(st: &AppState) -> Result<BTreeMap<String, Value>, ApiError> {
    Ok(sqlx::query("SELECT sub, email, name, first_at, last_at, roles, app_roles FROM people_seen").fetch_all(&st.db).await?.iter().map(|r| (r.get::<String, _>("sub"), seen_json(r))).collect())
}

/// The requests not settled yet, and those settled in the last week, newest first (`app`: of that app only).
async fn recent_requests(st: &AppState, app: Option<&str>) -> Result<Vec<Value>, ApiError> {
    let week = (chrono::Utc::now() - chrono::Duration::days(7)).to_rfc3339();
    let rows = sqlx::query("SELECT * FROM access_requests WHERE ($1 = '' OR app = $1) AND (status IN ('awaiting_approval', 'sent', 'pending') OR updated_at > $2) ORDER BY asked_at DESC LIMIT 200")
        .bind(app.unwrap_or("")).bind(&week).fetch_all(&st.db).await?;
    Ok(rows.iter().map(request_json).collect())
}

/// A request as the console shows it.
pub(crate) fn request_json(r: &sqlx::postgres::PgRow) -> Value {
    let parse = |k: &str| r.get::<Option<String>, _>(k).and_then(|v| serde_json::from_str::<Value>(&v).ok()).unwrap_or(Value::Null);
    json!({ "id": r.get::<String, _>("id"), "kind": r.get::<String, _>("kind"), "event": r.get::<String, _>("event"), "person": r.get::<Option<String>, _>("person"), "email": r.get::<Option<String>, _>("email"),
        "app": r.get::<Option<String>, _>("app"), "roles": parse("roles"), "details": parse("details"), "guest": r.get::<i64, _>("guest") != 0, "until": r.get::<Option<String>, _>("until"), "note": r.get::<Option<String>, _>("note"),
        "status": r.get::<String, _>("status"), "asked_by": r.get::<String, _>("asked_by"), "asked_at": r.get::<String, _>("asked_at"), "approved_by": r.get::<Option<String>, _>("approved_by"), "decided_at": r.get::<Option<String>, _>("decided_at"),
        "decided_note": r.get::<Option<String>, _>("decided_note"), "answer": r.get::<Option<String>, _>("answer"), "result": parse("result"), "updated_at": r.get::<String, _>("updated_at") })
}

/// What the caller may change here: the receiver handles it, and the caller holds the power it takes (the provider checks
/// again from its own records).
fn can_json(caps: &Capabilities, admin: bool) -> Value {
    json!({ "invite": admin && caps.handles("member.invited"), "set_roles": admin && caps.handles("member.roles"), "remove": admin && caps.handles("member.removed"),
            "cancel_invitations": admin && caps.handles("invitation.cancelled"), "resend_invitations": admin && caps.handles("invitation.resent"), "grant": caps.handles("access.granted"),
            "revoke": caps.handles("access.revoked"), "rename": admin && caps.handles("organization.updated"), "leave": caps.handles("member.removed") })
}

/// `GET /v1/people`: the people of the organization (the identity provider's, else those the server has seen), each with the
/// apps they own, build and use, when they were last seen; the requests about people under way; what the caller may change.
/// Those who build the organization's apps or hold a power read it; the others reach their apps only (`bearer_guard`).
pub(crate) async fn list_people(InRealm(st): InRealm, Caller(who): Caller) -> Res {
    require_human(&who, "the people list")?;
    let seen = seen_by_id(&st).await?;
    let apps = apps_of(&st).await?;
    let app_of_client: BTreeMap<String, String> = apps.iter().filter_map(|a| a.client_id.clone().map(|c| (c, a.id.clone()))).collect();
    let (listed, source, problem) = match directory_people(&st, &who.sub).await {
        Ok(Some(p)) => (p, "directory", None),
        Ok(None) => (seen.values().cloned().collect(), "seen", None),
        Err(e) => { tracing::warn!(error = %e, "people.list"); (seen.values().cloned().collect(), "seen", Some(e)) }
    };
    // the apps shared with guests, and who shared and approved each
    let shares: Vec<Value> = sqlx::query("SELECT * FROM access_requests WHERE kind = 'share' AND status IN ('sent', 'pending', 'done')").fetch_all(&st.db).await?.iter().map(request_json).collect();
    let guest_roles = &crate::auth::platform_roles().guest;
    let people: Vec<Value> = listed.into_iter().map(|mut p| {
        let id = p["id"].as_str().unwrap_or_default().to_string();
        fill_from_seen(&mut p, &seen);
        // a guest, as the directory says, else as their roles do
        if p["guest"].is_null() { p["guest"] = json!(strings(&p["roles"]).iter().any(|r| guest_roles.contains(r))); }
        if p["guest"] == true {
            let email = p["email"].as_str().map(str::to_lowercase);
            p["shares"] = json!(shares.iter().filter(|r| r["person"] == id.as_str() || (email.is_some() && r["email"].as_str() == email.as_deref()))
                .map(|r| json!({ "app": r["app"], "roles": r["roles"], "by": r["asked_by"], "approved_by": r["approved_by"], "until": r["until"], "status": r["status"] })).collect::<Vec<_>>());
        }
        let uses: Vec<Value> = p["grants"].as_object().map(|g| g.iter().filter_map(|(client, roles)| app_of_client.get(client).map(|app| json!({ "app": app, "roles": roles }))).collect()).unwrap_or_default();
        p["apps"] = json!({ "owns": apps.iter().filter(|a| a.owner.as_deref() == Some(id.as_str())).map(|a| &a.id).collect::<Vec<_>>(),
                            "builds": apps.iter().filter(|a| a.builders.iter().any(|b| b == &id)).map(|a| &a.id).collect::<Vec<_>>(), "uses": uses });
        p
    }).collect();
    let caps = client_hook::handled(&st).await;
    // the member roles: those the identity provider offers (`organization.get`), else those seen; each with its powers
    let offered = client_hook::ask_cached(&st, "organization.get", json!({ "by": who.sub }), ORG_FRESH).await.unwrap_or_else(|e| { tracing::warn!(error = %e, "organization.get"); None })
        .and_then(|o| o["roles"].as_array().cloned()).unwrap_or_default();
    let mut member_roles: Vec<Value> = offered.iter().filter_map(|r| r["name"].as_str().map(|n| json!({ "name": n, "description": r["description"], "offered": true }))).collect();
    let seen_roles: std::collections::BTreeSet<String> = people.iter().flat_map(|p| strings(&p["roles"])).collect();
    for r in seen_roles { if !member_roles.iter().any(|m| m["name"] == r.as_str()) { member_roles.push(json!({ "name": r, "description": null, "offered": false })); } }
    for m in &mut member_roles { let n = m["name"].as_str().unwrap_or_default().to_string(); m["powers"] = json!(powers_of(&st, &n)); m["guest"] = json!(guest_roles.contains(&n)); }
    // the requests: an admin's are everyone's; anyone else sees those they asked
    let requests: Vec<Value> = recent_requests(&st, None).await?.into_iter().filter(|r| who.is_admin() || r["asked_by"] == who.sub.as_str()).collect();
    let mut v = json!({ "people": people, "source": source, "can": can_json(&caps, who.is_admin()), "member_roles": member_roles, "requests": requests,
        "admin_url": st.cfg.identity_admin_url.as_deref().map(|u| u.replace("{app}", "").replace("{client_id}", "")) });
    if let Some(e) = problem { v["problem"] = json!(e); }
    Ok(Json(v).into_response())
}

/// The receiver must handle `event`, or the change is made in the identity provider itself: a 409 saying so.
async fn require_handled(st: &AppState, event: &str, what: &str) -> Result<Capabilities, ApiError> {
    let caps = client_hook::handled(st).await;
    if caps.handles(event) { return Ok(caps); }
    let there = st.cfg.identity_admin_url.as_deref().map(|u| format!(" ({})", u.replace("{app}", "").replace("{client_id}", ""))).unwrap_or_default();
    Err(ApiError(StatusCode::CONFLICT, format!("{what} is done in your identity provider{there}: its receiver does not take `{event}` from Cori")))
}

/// Records a request and queues its event (`queue`: the app it is about, or '' for the organization's people), unless it
/// waits for an admin's approval (`status`). Returns its id.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn new_request(st: &Arc<AppState>, who: &Principal, kind: &str, event: &str, queue: &str, status: &str, fields: &Value, body: Value) -> Result<String, ApiError> {
    let (id, now) = (uuid::Uuid::new_v4().to_string(), chrono::Utc::now().to_rfc3339());
    let s = |k: &str| fields[k].as_str().map(String::from);
    sqlx::query("INSERT INTO access_requests (id, kind, event, person, email, app, roles, details, guest, until, note, status, asked_by, asked_at, updated_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $14)")
        .bind(&id).bind(kind).bind(event).bind(s("person")).bind(s("email")).bind(s("app")).bind(fields["roles"].to_string()).bind(fields.get("details").map(|d| d.to_string()))
        .bind(if fields["guest"].as_bool() == Some(true) { 1i64 } else { 0 }).bind(s("until")).bind(s("note")).bind(status).bind(&who.sub).bind(&now).execute(&st.db).await?;
    db::audit(&st.db, &who.sub, &format!("people.{kind}"), s("person").or(s("email")).or(s("app")).as_deref().unwrap_or("organization"), json!({ "request": id, "event": event, "fields": fields, "status": status })).await?;
    if status == "sent" { send_request(st, &id, queue, event, body, who).await?; }
    Ok(id)
}

/// Queues the event of request `id` for the receiver, in the name of who asked (`by`), and forgets the answers it changes.
pub(crate) async fn send_request(st: &Arc<AppState>, id: &str, queue: &str, event: &str, mut body: Value, who: &Principal) -> Result<(), ApiError> {
    body["request"] = json!(id);
    if body.get("by").is_none() { body["by"] = json!(who.sub); body["by_email"] = json!(who.email); }
    client_hook::enqueue(st, queue, event, &body).await?;
    client_hook::forget_answers(st);
    Ok(())
}

/// The answer to a request just made: its id and where it stands now (delivered at once when the receiver answered).
async fn request_answer(st: &AppState, id: &str) -> Res {
    // the first delivery runs at once, in the background: give it a moment, then say where the request is
    for _ in 0..20 {
        let r = sqlx::query("SELECT * FROM access_requests WHERE id = $1").bind(id).fetch_one(&st.db).await?;
        if r.get::<String, _>("status") != "sent" { return Ok((StatusCode::ACCEPTED, Json(request_json(&r))).into_response()); }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let r = sqlx::query("SELECT * FROM access_requests WHERE id = $1").bind(id).fetch_one(&st.db).await?;
    Ok((StatusCode::ACCEPTED, Json(request_json(&r))).into_response())
}

fn strings(v: &Value) -> Vec<String> { v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.trim().to_string())).filter(|s| !s.is_empty()).collect()).unwrap_or_default() }

fn email_problem(e: &str) -> Option<&'static str> {
    let ok = e.len() <= 254 && e.split_once('@').is_some_and(|(a, d)| !a.is_empty() && d.contains('.') && !d.starts_with('.') && !d.ends_with('.')) && !e.chars().any(|c| c.is_whitespace() || c.is_control());
    (!ok).then_some("an email address")
}

/// `POST /v1/invitations` `{ email, roles?, grants?: [{ app, roles }] }`: invites someone into the organization, with member
/// roles (`member` when none) and, optionally, roles in apps, which they hold once they join. Admins.
pub(crate) async fn invite(InRealm(st): InRealm, Caller(who): Caller, Json(b): Json<Value>) -> Res {
    require_human(&who, "inviting people")?;
    require_power(&st, &who, &[Power::Admin], "inviting people")?;
    require_handled(&st, "member.invited", "inviting people").await?;
    let email = b["email"].as_str().map(|e| e.trim().to_lowercase()).unwrap_or_default();
    if let Some(what) = email_problem(&email) { return Err(ApiError(StatusCode::BAD_REQUEST, format!("`email`: {what}"))); }
    let mut roles = strings(&b["roles"]); if roles.is_empty() { roles = vec!["member".into()]; }
    let mut grants = Vec::new();
    for g in b["grants"].as_array().cloned().unwrap_or_default() {
        let app = g["app"].as_str().unwrap_or_default();
        let rec = require_app(&st, app).await?;
        let client_id = rec.client_id.clone().ok_or_else(|| ApiError(StatusCode::CONFLICT, format!("`{app}`'s sign-in client is not ready yet: its roles cannot be granted")))?;
        let declared = client_hook::kept_roles(&st, app).await?;
        let want = strings(&g["roles"]);
        if let Some(r) = want.iter().find(|r| !declared.contains_key(*r)) { return Err(ApiError(StatusCode::BAD_REQUEST, format!("`{r}` is not one of `{app}`'s roles ({})", declared.keys().cloned().collect::<Vec<_>>().join(", ")))); }
        if !want.is_empty() { grants.push(json!({ "app": app, "client_id": client_id, "roles": want })); }
    }
    let fields = json!({ "email": email, "roles": roles, "details": { "grants": grants } });
    let body = json!({ "email": email, "roles": roles, "grants": grants, "guest": false, "until": null });
    let id = new_request(&st, &who, "invite", "member.invited", "", "sent", &fields, body).await?;
    request_answer(&st, &id).await
}

/// `GET /v1/invitations`: the invitations waiting to be accepted (the identity provider's, when it lists them) and the
/// invitation requests under way. Admins.
pub(crate) async fn list_invitations(InRealm(st): InRealm, Caller(who): Caller) -> Res {
    require_human(&who, "the invitations")?;
    require_power(&st, &who, &[Power::Admin], "the invitations")?;
    let listed = client_hook::ask_cached(&st, "invitations.list", json!({ "by": who.sub }), PEOPLE_FRESH).await.map_err(|e| ApiError(StatusCode::BAD_GATEWAY, e))?;
    let requests: Vec<Value> = recent_requests(&st, None).await?.into_iter().filter(|r| r["kind"] == "invite").collect();
    Ok(Json(json!({ "invitations": listed.map(|l| l["invitations"].clone()).unwrap_or(Value::Null), "requests": requests })).into_response())
}

/// `DELETE /v1/invitations/{id}`: cancels an invitation not accepted yet. Admins.
pub(crate) async fn cancel_invitation(InRealm(st): InRealm, Caller(who): Caller, Path(invitation): Path<String>) -> Res {
    require_human(&who, "cancelling an invitation")?;
    require_power(&st, &who, &[Power::Admin], "cancelling an invitation")?;
    require_handled(&st, "invitation.cancelled", "cancelling an invitation").await?;
    let id = new_request(&st, &who, "cancel_invitation", "invitation.cancelled", "", "sent", &json!({ "roles": [], "details": { "invitation": invitation } }), json!({ "invitation": invitation })).await?;
    request_answer(&st, &id).await
}

/// `POST /v1/invitations/{id}/resend`: emails an invitation again. Admins.
pub(crate) async fn resend_invitation(InRealm(st): InRealm, Caller(who): Caller, Path(invitation): Path<String>) -> Res {
    require_human(&who, "sending an invitation again")?;
    require_power(&st, &who, &[Power::Admin], "sending an invitation again")?;
    require_handled(&st, "invitation.resent", "sending an invitation again").await?;
    let id = new_request(&st, &who, "resend_invitation", "invitation.resent", "", "sent", &json!({ "roles": [], "details": { "invitation": invitation } }), json!({ "invitation": invitation })).await?;
    request_answer(&st, &id).await
}

/// `PUT /v1/people/{id}/roles` `{ roles }`: a person's member roles (the organization's: owner, admin, builder…). Admins.
pub(crate) async fn set_person_roles(InRealm(st): InRealm, Caller(who): Caller, Path(person): Path<String>, Json(b): Json<Value>) -> Res {
    require_human(&who, "changing someone's roles")?;
    require_power(&st, &who, &[Power::Admin], "changing someone's roles")?;
    require_handled(&st, "member.roles", "changing someone's roles").await?;
    let roles = strings(&b["roles"]);
    if roles.is_empty() { return Err(ApiError(StatusCode::BAD_REQUEST, "`roles`: at least one".into())); }
    let id = new_request(&st, &who, "roles", "member.roles", "", "sent", &json!({ "person": person, "roles": roles }), json!({ "person": person, "roles": roles })).await?;
    request_answer(&st, &id).await
}

/// `GET /v1/people/{id}/leaving`: what someone leaves behind if removed: the apps they own (each to hand to another
/// owner) and build, their agent sessions, the approvals waiting on them as a run's initiator. Admins.
pub(crate) async fn leaving(InRealm(st): InRealm, Caller(who): Caller, Path(person): Path<String>) -> Res {
    require_human(&who, "what someone leaves")?;
    require_power(&st, &who, &[Power::Admin], "what someone leaves")?;
    let apps = apps_of(&st).await?;
    let sessions: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM agent_sessions WHERE sub = $1 AND revoked_at IS NULL AND expires_at > $2").bind(&person).bind(chrono::Utc::now().to_rfc3339()).fetch_one(&st.db).await?;
    let waiting: Vec<Value> = sqlx::query("SELECT a.app, a.run_id, a.step FROM approvals a JOIN runs r ON r.id = a.run_id WHERE a.status = 'pending' AND a.to_role = 'user:initiator' AND r.user_sub = $1 ORDER BY a.app")
        .bind(&person).fetch_all(&st.db).await?.iter().map(|r| json!({ "app": r.get::<String, _>("app"), "run": r.get::<String, _>("run_id"), "step": r.get::<String, _>("step") })).collect();
    Ok(Json(json!({ "person": person, "owns": apps.iter().filter(|a| a.owner.as_deref() == Some(person.as_str())).map(|a| &a.id).collect::<Vec<_>>(),
        "builds": apps.iter().filter(|a| a.builders.iter().any(|b| b == &person)).map(|a| &a.id).collect::<Vec<_>>(), "agent_sessions": sessions, "waiting": waiting })).into_response())
}

/// `DELETE /v1/people/{id}` `{ owners?: { <app>: <new owner> } }`: removes someone from the organization. Admins; leaving
/// is `POST /v1/organization/leave`. Once the identity provider removed them, Cori does its part (`person_left`): the
/// apps they own go to the owners named, the remover by default.
pub(crate) async fn remove_person(InRealm(st): InRealm, Caller(who): Caller, Path(person): Path<String>, body: axum::body::Bytes) -> Res {
    require_human(&who, "removing someone")?;
    require_power(&st, &who, &[Power::Admin], "removing someone")?;
    if person == who.sub { return Err(ApiError(StatusCode::BAD_REQUEST, "to leave the organization yourself: `POST /v1/organization/leave`".into())); }
    require_handled(&st, "member.removed", "removing someone").await?;
    let b: Value = if body.is_empty() { json!({}) } else { serde_json::from_slice(&body).map_err(|_| ApiError(StatusCode::BAD_REQUEST, "the body is not JSON".into()))? };
    let mut owners = BTreeMap::new();
    for a in apps_of(&st).await?.into_iter().filter(|a| a.owner.as_deref() == Some(person.as_str())) {
        let to = b["owners"][&a.id].as_str().map(str::trim).filter(|o| !o.is_empty() && *o != person).unwrap_or(&who.sub).to_string();
        owners.insert(a.id, to);
    }
    let id = new_request(&st, &who, "remove", "member.removed", "", "sent", &json!({ "person": person, "roles": [], "details": { "owners": owners } }), json!({ "person": person })).await?;
    request_answer(&st, &id).await
}

/// When `person` left the organization (the latest time Cori heard of it), as unix seconds: their tokens issued before are
/// refused. Read again every 15 seconds in a process (another replica's record shows within that).
pub(crate) async fn removed_at(st: &AppState, person: &str) -> Result<Option<i64>, ApiError> {
    let key = format!("{}/{person}", st.realm().map(|r| r.as_str().to_string()).unwrap_or_default());
    if let Some((checked, at)) = st.removed.lock().unwrap_or_else(|e| e.into_inner()).get(&key).cloned() && checked.elapsed() < Duration::from_secs(15) { return Ok(at); }
    let at = sqlx::query_scalar::<_, String>("SELECT at FROM people_removed WHERE sub = $1").bind(person).fetch_optional(&st.db).await?
        .and_then(|a| chrono::DateTime::parse_from_rfc3339(&a).ok()).map(|a| a.timestamp());
    st.removed.lock().unwrap_or_else(|e| e.into_inner()).insert(key, (std::time::Instant::now(), at));
    Ok(at)
}

/// What leaving the organization does in Cori, once the identity provider removed someone (or told Cori they left):
/// their tokens issued until now are refused, their agent sessions revoked, they build no app any more, and the apps
/// they own go to the owners `owners` names; one it does not name keeps them, shown as an owner who left, for an admin
/// to reassign. `how`: removed, left, left elsewhere, deleted.
pub(crate) async fn person_left(st: &AppState, person: &str, owners: &BTreeMap<String, String>, by: &str, how: &str) -> anyhow::Result<()> {
    let now = chrono::Utc::now();
    sqlx::query("INSERT INTO people_removed (sub, at, how) VALUES ($1, $2, $3) ON CONFLICT ON CONSTRAINT people_removed_pkey DO UPDATE SET at = EXCLUDED.at, how = EXCLUDED.how")
        .bind(person).bind(now.to_rfc3339()).bind(how).execute(&st.db).await?;
    st.removed.lock().unwrap_or_else(|e| e.into_inner()).insert(format!("{}/{person}", st.realm().map(|r| r.as_str().to_string()).unwrap_or_default()), (std::time::Instant::now(), Some(now.timestamp())));
    let revoked = sqlx::query("UPDATE agent_sessions SET revoked_at = $1 WHERE sub = $2 AND revoked_at IS NULL").bind(now.to_rfc3339()).bind(person).execute(&st.db).await?.rows_affected();
    // the accounts they connected here go, and their providers are told
    let connections = crate::connections::forget(st, &crate::connections::Owner::Person(person.to_string())).await?;
    let apps = apps_of(st).await.map_err(|e| anyhow::anyhow!("{}", e.1))?;
    let mut handed = BTreeMap::new();
    for a in &apps {
        let builders: Vec<&String> = a.builders.iter().filter(|b| *b != person).collect();
        let owner = match (a.owner.as_deref() == Some(person), owners.get(&a.id)) { (true, Some(to)) => Some(to.as_str()), _ => a.owner.as_deref() };
        if builders.len() == a.builders.len() && owner == a.owner.as_deref() { continue; }
        let builders: Vec<&String> = builders.into_iter().filter(|b| Some(b.as_str()) != owner).collect();
        sqlx::query("UPDATE apps SET builders = $1, owner = $2 WHERE id = $3").bind(serde_json::to_string(&builders)?).bind(owner).bind(&a.id).execute(&st.db).await?;
        if owner != a.owner.as_deref() {
            handed.insert(a.id.clone(), owner.unwrap_or_default().to_string());
            // the identity provider keeps the new owner (queued: this may run inside a delivery)
            if let Some(body) = client_hook::updated_body(st, &a.id).await? { client_hook::enqueue_later(st, &a.id, "app.updated", &body).await?; }
        }
    }
    db::audit(&st.db, by, "people.left", person, json!({ "how": how, "agent_sessions_revoked": revoked, "connections_removed": connections, "apps_handed": handed })).await?;
    tracing::info!(person, how, revoked, apps = handed.len(), "someone left the organization");
    Ok(())
}

/// What Cori does itself once the identity provider made a change it asked for: someone removed, or who left, leaves
/// Cori's records too (`person_left`).
pub(crate) async fn request_settled(st: &AppState, id: &str) -> anyhow::Result<()> {
    let Some(r) = sqlx::query("SELECT kind, status, person, details, asked_by FROM access_requests WHERE id = $1").bind(id).fetch_optional(&st.db).await? else { return Ok(()) };
    let (kind, status): (String, String) = (r.get("kind"), r.get("status"));
    if status != "done" || !matches!(kind.as_str(), "remove" | "leave") { return Ok(()); }
    let Some(person) = r.get::<Option<String>, _>("person") else { return Ok(()) };
    let details: Value = r.get::<Option<String>, _>("details").and_then(|d| serde_json::from_str(&d).ok()).unwrap_or(Value::Null);
    let owners: BTreeMap<String, String> = details["owners"].as_object().map(|o| o.iter().filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_string()))).collect()).unwrap_or_default();
    person_left(st, &person, &owners, &r.get::<String, _>("asked_by"), if kind == "leave" { "left" } else { "removed" }).await
}

/// A receiver (or a directory of the organization's own) that answers `people.list` but sends no notifications: the people
/// Cori has seen and it no longer lists left elsewhere, and Cori does its part (as for `person.removed`). Only on a whole
/// answer that names somebody, and everybody by id, never for a service account. The leader runs it every 10 minutes.
/// Returns how many left.
pub(crate) async fn compare_directory(st: &Arc<AppState>) -> anyhow::Result<usize> {
    if !client_hook::answers(st, "people.list").await { return Ok(0); }
    let mut listed = std::collections::BTreeSet::new();
    let mut cursor: Option<String> = None;
    for page in 0.. {
        if page == 50 { return Ok(0); }   // too many pages to be sure of the whole list
        let Ok(Some(p)) = client_hook::ask(st, "people.list", json!({ "by": "cori", "cursor": cursor })).await else { return Ok(0) };
        let Some(people) = p["people"].as_array() else { return Ok(0) };
        // someone listed by no id could be anyone Cori has seen: such a list says nobody left
        if !people.iter().all(named) { tracing::warn!("people.list names people without an `id`: nobody compared"); return Ok(0); }
        for x in people { if let Some(id) = x["id"].as_str() { listed.insert(id.to_string()); } }
        match p["next"].as_str().filter(|n| !n.is_empty()) { Some(n) => cursor = Some(n.to_string()), None => break }
    }
    if listed.is_empty() { return Ok(0); }
    // the people seen since they last left, if they ever did
    let gone: Vec<String> = sqlx::query_scalar("SELECT s.sub FROM people_seen s LEFT JOIN people_removed r ON r.sub = s.sub WHERE s.email IS NOT NULL AND (r.at IS NULL OR r.at < s.last_at)")
        .fetch_all(&st.db).await?.into_iter().filter(|s: &String| !listed.contains(s) && !s.starts_with("service-account-")).collect();
    for p in &gone { person_left(st, p, &BTreeMap::new(), "cori", "left elsewhere").await?; }
    if !gone.is_empty() { client_hook::forget_answers(st); }
    Ok(gone.len())
}

/// `POST /v1/organization/leave`: the caller leaves the organization (its last owner may not). The apps they own keep them,
/// shown as an owner who left, for an admin to reassign.
pub(crate) async fn leave_organization(InRealm(st): InRealm, Caller(who): Caller) -> Res {
    require_human(&who, "leaving the organization")?;
    require_handled(&st, "member.removed", "leaving the organization").await?;
    let id = new_request(&st, &who, "leave", "member.removed", "", "sent", &json!({ "person": who.sub, "roles": [] }), json!({ "person": who.sub })).await?;
    request_answer(&st, &id).await
}

/// `GET /v1/organization`: the organization a request acts in, as its identity provider names it (`organization.get`): its
/// name, slug and member roles; what its receiver handles; what the caller may change; where else people are managed.
pub(crate) async fn get_organization(InRealm(st): InRealm, Caller(who): Caller) -> Res {
    require_human(&who, "the organization")?;
    let caps = client_hook::handled(&st).await;
    let info = client_hook::ask_cached(&st, "organization.get", json!({ "by": who.sub }), ORG_FRESH).await.unwrap_or_else(|e| { tracing::warn!(error = %e, "organization.get"); None });
    let mut v = organization_of(&st).unwrap_or_else(|| json!({}));
    if let Some(i) = &info { for k in ["name", "slug", "roles"] { if !i[k].is_null() { v[k] = i[k].clone(); } } }
    v["receiver"] = caps.json();
    if let Some(roles) = v["roles"].as_array_mut() {
        for r in roles { let n = r["name"].as_str().unwrap_or_default().to_string(); r["powers"] = json!(powers_of(&st, &n)); }
    }
    v["can"] = can_json(&caps, who.is_admin());
    v["can"]["delete"] = json!(st.tenancy.as_ref().is_some_and(|t| t.may_delete(&who)));
    v["can"]["export"] = json!(who.is_admin());
    v["admin_url"] = json!(st.cfg.identity_admin_url.as_deref().map(|u| u.replace("{app}", "").replace("{client_id}", "")));
    Ok(Json(v).into_response())
}

/// `DELETE /v1/organization?confirm=`: deletes the organization a request acts in, for good, as the deployment's tenancy
/// does it (its owner; `confirm` names it). A server of a single organization is not deleted from its console: 404.
pub(crate) async fn delete_organization(InRealm(st): InRealm, Caller(who): Caller, axum::extract::Query(q): axum::extract::Query<BTreeMap<String, String>>) -> Res {
    require_human(&who, "deleting the organization")?;
    let none = || ApiError(StatusCode::NOT_FOUND, "this server's organization is not deleted from its console".into());
    let (Some(tenancy), Some(realm)) = (st.tenancy.clone(), st.realm().cloned()) else { return Err(none()) };
    match tenancy.delete(&st, &realm, &who, q.get("confirm").map(String::as_str)).await {
        Some(r) => Ok((StatusCode::ACCEPTED, Json(r?)).into_response()),
        None => Err(none()),
    }
}

/// `PATCH /v1/organization` `{ name?, slug? }`: renames the organization (the identity provider decides who may: changing
/// its slug moves every front). Admins.
pub(crate) async fn patch_organization(InRealm(st): InRealm, Caller(who): Caller, Json(b): Json<Value>) -> Res {
    require_human(&who, "renaming the organization")?;
    require_power(&st, &who, &[Power::Admin], "renaming the organization")?;
    require_handled(&st, "organization.updated", "renaming the organization").await?;
    let (name, slug) = (b["name"].as_str().map(str::trim).filter(|n| !n.is_empty()), b["slug"].as_str().map(str::trim).filter(|s| !s.is_empty()));
    if name.is_none() && slug.is_none() { return Err(ApiError(StatusCode::BAD_REQUEST, "`name` or `slug`".into())); }
    let id = new_request(&st, &who, "organization", "organization.updated", "", "sent", &json!({ "roles": [], "details": { "name": name, "slug": slug } }), json!({ "name": name, "slug": slug })).await?;
    request_answer(&st, &id).await
}

/// The roles an app declares, with their descriptions from cori.toml.
async fn declared_roles_of(st: &AppState, app: &str) -> Result<BTreeMap<String, crate::client_hook::RoleDecl>, ApiError> {
    Ok(client_hook::kept_roles(st, app).await?)
}

/// `GET /v1/apps/{app}/access`: who builds the app (owner, builders) and who holds each role it declares (the identity
/// provider's answer to `access.list`, else to `people.list`), the requests about it, what the caller may change, whether
/// its roles are given through Cori at all (`grants_here`), and what the deployment says of giving them (`message`,
/// `CORI_IDENTITY_ACCESS_MESSAGE`). Its owner and builders, and admins.
pub(crate) async fn app_access(InRealm(st): InRealm, Caller(who): Caller, Path(app): Path<String>) -> Res {
    require_human(&who, "an app's access")?;
    let rec = require_app(&st, &app).await?;
    if !is_owner_or_builder(&who, &rec) { return Err(ApiError(StatusCode::FORBIDDEN, format!("`{app}`'s access is for its owner, its builders and admins"))); }
    let declared = declared_roles_of(&st, &app).await?;
    let seen = seen_by_id(&st).await?;
    let person = |id: &str| -> Value { let s = seen.get(id); json!({ "id": id, "email": s.map(|s| s["email"].clone()), "name": s.map(|s| s["name"].clone()) }) };
    let mut holders: BTreeMap<String, Vec<Value>> = declared.keys().map(|r| (r.clone(), vec![])).collect();
    let mut source = "none";
    if let Some(client) = rec.client_id.as_deref() {
        let listed = client_hook::ask_cached(&st, "access.list", json!({ "client_id": client, "app": app, "by": who.sub }), PEOPLE_FRESH).await.unwrap_or_else(|e| { tracing::warn!(error = %e, "access.list"); None });
        let grants: Vec<(Value, Vec<String>)> = match listed {
            Some(l) => { source = "directory"; l["grants"].as_array().cloned().unwrap_or_default().into_iter().map(|g| (g["person"].clone(), strings(&g["roles"]))).collect() }
            None => match directory_people(&st, &who.sub).await.unwrap_or(None) {
                Some(people) => { source = "directory"; people.into_iter().map(|mut p| { fill_from_seen(&mut p, &seen); let roles = strings(&p["grants"][client]); (json!({ "id": p["id"], "email": p["email"], "name": p["name"], "guest": p["guest"] }), roles) }).collect() }
                None => vec![],
            },
        };
        for (p, roles) in grants {
            let p = if p.is_object() { p } else { person(p.as_str().unwrap_or_default()) };
            for r in roles { holders.entry(r).or_default().push(p.clone()); }
        }
    }
    let caps = client_hook::handled(&st).await;
    let may = !who.is_agent() && (who.is_admin() || rec.owner.as_deref() == Some(who.sub.as_str()));
    let roles: Vec<Value> = holders.into_iter().map(|(role, who_holds)| json!({ "role": role, "description": declared.get(&role).and_then(|d| d.description.clone()), "declared": declared.contains_key(&role), "holders": who_holds })).collect();
    let owner_left = match rec.owner.as_deref() { Some(o) => removed_at(&st, o).await?.is_some(), None => false };
    Ok(Json(json!({ "app": app, "client_id": rec.client_id, "owner": rec.owner, "owner_left": owner_left, "builders": rec.builders, "roles": roles, "source": source,
        "requests": recent_requests(&st, Some(&app)).await?, "can": { "grant": may && caps.handles("access.granted"), "revoke": may && caps.handles("access.revoked"), "invite": who.is_admin() && caps.handles("member.invited") },
        "grants_here": caps.handles("access.granted"), "message": st.cfg.access_message,
        "admin_url": if caps.handles("access.granted") { None } else { crate::api::admin_url_for(&st, &app, rec.client_id.as_deref()) } })).into_response())
}

/// `POST /v1/apps/{app}/access` `{ person | email, roles, granted, until?, note? }`: grants roles in the app, or takes them
/// back. The app's owner and admins; a role the app declares only. With `email`, someone outside the organization: a share
/// with a guest, which waits for an admin's approval (`awaiting_approval`) unless an admin made it, may end at `until`, and
/// reaches the identity provider once approved (`access.granted` with `guest`, the address and who approved).
pub(crate) async fn change_app_access(InRealm(st): InRealm, Caller(who): Caller, Path(app): Path<String>, Json(b): Json<Value>) -> Res {
    require_human(&who, "changing an app's access")?;
    let rec = require_app(&st, &app).await?;
    require_owner_or_admin(&who, &rec, "changing the app's access")?;
    let granted = b["granted"].as_bool().ok_or_else(|| ApiError(StatusCode::BAD_REQUEST, "`granted`: true (grant) or false (take back)".into()))?;
    let event = if granted { "access.granted" } else { "access.revoked" };
    require_handled(&st, event, if granted { "granting an app's roles" } else { "taking an app's roles back" }).await?;
    let email = b["email"].as_str().map(|e| e.trim().to_lowercase()).filter(|e| !e.is_empty());
    if b["person"].as_str().map(str::trim).filter(|p| !p.is_empty()).is_none() && let Some(email) = email {
        return share_outside(&st, &who, &rec, granted, email, &b).await;
    }
    let person = b["person"].as_str().map(str::trim).filter(|p| !p.is_empty()).ok_or_else(|| ApiError(StatusCode::BAD_REQUEST, "`person`: the id of a member of the organization (or `email`: someone outside it)".into()))?.to_string();
    let roles = strings(&b["roles"]);
    if roles.is_empty() { return Err(ApiError(StatusCode::BAD_REQUEST, "`roles`: at least one".into())); }
    let client_id = rec.client_id.clone().ok_or_else(|| ApiError(StatusCode::CONFLICT, format!("`{app}`'s sign-in client is not ready yet: its roles cannot be granted")))?;
    if granted {
        let declared = declared_roles_of(&st, &app).await?;
        if let Some(r) = roles.iter().find(|r| !declared.contains_key(*r)) { return Err(ApiError(StatusCode::BAD_REQUEST, format!("`{r}` is not one of `{app}`'s roles ({})", declared.keys().cloned().collect::<Vec<_>>().join(", ")))); }
    }
    let fields = json!({ "person": person, "app": app, "roles": roles });
    let body = json!({ "app": app, "client_id": client_id, "person": person, "roles": roles, "guest": false, "until": null });
    let id = new_request(&st, &who, if granted { "grant" } else { "revoke" }, event, &app, "sent", &fields, body).await?;
    request_answer(&st, &id).await
}

/// An app shared with someone outside the organization (`email`), or that share taken back before they joined: see
/// `change_app_access`.
async fn share_outside(st: &Arc<AppState>, who: &Principal, rec: &AppRecord, granted: bool, email: String, b: &Value) -> Res {
    let app = rec.id.clone();
    if let Some(what) = email_problem(&email) { return Err(ApiError(StatusCode::BAD_REQUEST, format!("`email`: {what}"))); }
    let roles = strings(&b["roles"]);
    if roles.is_empty() { return Err(ApiError(StatusCode::BAD_REQUEST, "`roles`: at least one".into())); }
    let client_id = rec.client_id.clone().ok_or_else(|| ApiError(StatusCode::CONFLICT, format!("`{app}`'s sign-in client is not ready yet: it cannot be shared")))?;
    if !granted {
        let fields = json!({ "email": email, "app": app, "roles": roles, "guest": true });
        let body = json!({ "app": app, "client_id": client_id, "email": email, "person": null, "roles": roles, "guest": true });
        let id = new_request(st, who, "revoke", "access.revoked", &app, "sent", &fields, body).await?;
        return request_answer(st, &id).await;
    }
    let declared = declared_roles_of(st, &app).await?;
    if let Some(r) = roles.iter().find(|r| !declared.contains_key(*r)) { return Err(ApiError(StatusCode::BAD_REQUEST, format!("`{r}` is not one of `{app}`'s roles ({})", declared.keys().cloned().collect::<Vec<_>>().join(", ")))); }
    // an end date: a day (its end, UTC) or a moment, in the future
    let until = match b["until"].as_str().map(str::trim).filter(|u| !u.is_empty()) {
        None => None,
        Some(u) => {
            let at = chrono::DateTime::parse_from_rfc3339(u).map(|d| d.with_timezone(&chrono::Utc)).ok()
                .or_else(|| chrono::NaiveDate::parse_from_str(u, "%Y-%m-%d").ok().and_then(|d| d.and_hms_opt(23, 59, 59)).map(|d| d.and_utc()))
                .ok_or_else(|| ApiError(StatusCode::BAD_REQUEST, "`until`: a date (YYYY-MM-DD) or a moment (RFC 3339)".into()))?;
            if at <= chrono::Utc::now() { return Err(ApiError(StatusCode::BAD_REQUEST, "`until`: a date to come".into())); }
            Some(at.to_rfc3339())
        }
    };
    let note = b["note"].as_str().map(|n| n.trim().chars().take(500).collect::<String>()).filter(|n| !n.is_empty());
    // an admin's own share is approved as it is made; anyone else's waits for an admin
    let admin = who.is_admin();
    let fields = json!({ "email": email, "app": app, "roles": roles, "guest": true, "until": until, "note": note });
    let body = json!({ "app": app, "client_id": client_id, "email": email, "roles": roles, "guest": true, "until": until, "note": note, "approved_by": who.sub });
    let id = new_request(st, who, "share", "access.granted", &app, if admin { "sent" } else { "awaiting_approval" }, &fields, body).await?;
    if admin { sqlx::query("UPDATE access_requests SET approved_by = $1, decided_at = asked_at WHERE id = $2").bind(&who.sub).bind(&id).execute(&st.db).await?; }
    request_answer(st, &id).await
}

/// A request waiting for an admin's approval, or a 404/409 saying why not.
async fn awaiting(st: &AppState, id: &str) -> Result<sqlx::postgres::PgRow, ApiError> {
    let r = sqlx::query("SELECT * FROM access_requests WHERE id = $1").bind(id).fetch_optional(&st.db).await?.ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "no such request".into()))?;
    let status: String = r.get("status");
    if status != "awaiting_approval" { return Err(ApiError(StatusCode::CONFLICT, format!("this request is {}, not waiting for an approval", status.replace('_', " ")))); }
    Ok(r)
}

/// `POST /v1/access-requests/{id}/approve` `{ note? }`: an admin approves an app shared with someone outside the
/// organization. Cori then asks the identity provider, in the name of who shared it and of who approved.
pub(crate) async fn approve_access_request(InRealm(st): InRealm, Caller(who): Caller, Path(id): Path<String>, Json(b): Json<Value>) -> Res {
    require_human(&who, "approving a share")?;
    require_power(&st, &who, &[Power::Admin], "approving a share")?;
    require_handled(&st, "access.granted", "sharing an app").await?;
    let r = awaiting(&st, &id).await?;
    let app: String = r.get::<Option<String>, _>("app").unwrap_or_default();
    let rec = require_app(&st, &app).await?;
    let client_id = rec.client_id.clone().ok_or_else(|| ApiError(StatusCode::CONFLICT, format!("`{app}`'s sign-in client is not ready yet")))?;
    let note = b["note"].as_str().map(|n| n.trim().chars().take(500).collect::<String>()).filter(|n| !n.is_empty());
    let now = chrono::Utc::now().to_rfc3339();
    let n = sqlx::query("UPDATE access_requests SET status = 'sent', approved_by = $1, decided_at = $2, decided_note = $3, updated_at = $2 WHERE id = $4 AND status = 'awaiting_approval'")
        .bind(&who.sub).bind(&now).bind(&note).bind(&id).execute(&st.db).await?.rows_affected();
    if n == 0 { return Err(ApiError(StatusCode::CONFLICT, "this request was decided meanwhile".into())); }
    db::audit(&st.db, &who.sub, "people.approve", &app, json!({ "request": id, "note": note })).await?;
    let parse = |k: &str| r.get::<Option<String>, _>(k).and_then(|v| serde_json::from_str::<Value>(&v).ok()).unwrap_or(Value::Null);
    let asker: String = r.get("asked_by");
    let body = json!({ "app": app, "client_id": client_id, "email": r.get::<Option<String>, _>("email"), "roles": parse("roles"), "guest": true, "until": r.get::<Option<String>, _>("until"),
        "note": r.get::<Option<String>, _>("note"), "by": asker, "approved_by": who.sub });
    send_request(&st, &id, &app, "access.granted", body, &who).await?;
    request_answer(&st, &id).await
}

/// `POST /v1/access-requests/{id}/refuse` `{ note }`: an admin refuses a share, saying why.
pub(crate) async fn refuse_access_request(InRealm(st): InRealm, Caller(who): Caller, Path(id): Path<String>, Json(b): Json<Value>) -> Res {
    require_human(&who, "refusing a share")?;
    require_power(&st, &who, &[Power::Admin], "refusing a share")?;
    awaiting(&st, &id).await?;
    let note = b["note"].as_str().map(|n| n.trim().chars().take(500).collect::<String>()).filter(|n| !n.is_empty());
    let now = chrono::Utc::now().to_rfc3339();
    sqlx::query("UPDATE access_requests SET status = 'refused', approved_by = NULL, decided_at = $1, decided_note = $2, answer = $3, updated_at = $1 WHERE id = $4 AND status = 'awaiting_approval'")
        .bind(&now).bind(&note).bind(note.as_ref().map(|n| format!("refused by an admin: {n}")).unwrap_or_else(|| "refused by an admin".into())).bind(&id).execute(&st.db).await?;
    db::audit(&st.db, &who.sub, "people.refuse", &id, json!({ "note": note })).await?;
    let r = sqlx::query("SELECT * FROM access_requests WHERE id = $1").bind(&id).fetch_one(&st.db).await?;
    Ok(Json(request_json(&r)).into_response())
}

/// `DELETE /v1/access-requests/{id}`: who asked, or an admin, withdraws a share before it is decided.
pub(crate) async fn cancel_access_request(InRealm(st): InRealm, Caller(who): Caller, Path(id): Path<String>) -> Res {
    require_human(&who, "withdrawing a request")?;
    let r = awaiting(&st, &id).await?;
    if !who.is_admin() && r.get::<String, _>("asked_by") != who.sub { return Err(ApiError(StatusCode::FORBIDDEN, "a request is withdrawn by who asked, or an admin".into())); }
    let now = chrono::Utc::now().to_rfc3339();
    sqlx::query("UPDATE access_requests SET status = 'cancelled', decided_at = $1, updated_at = $1 WHERE id = $2 AND status = 'awaiting_approval'").bind(&now).bind(&id).execute(&st.db).await?;
    db::audit(&st.db, &who.sub, "people.withdraw", &id, json!({})).await?;
    let r = sqlx::query("SELECT * FROM access_requests WHERE id = $1").bind(&id).fetch_one(&st.db).await?;
    Ok(Json(request_json(&r)).into_response())
}

/// Guests whose access reached its end date (`until`): `access.revoked` for each, in the name of the admin who approved
/// it, and the share marked `ended`. The leader runs it. Returns how many.
pub(crate) async fn end_guest_access(st: &Arc<AppState>) -> anyhow::Result<usize> {
    if client_hook::hook_url(st).is_none() { return Ok(0); }
    let now = chrono::Utc::now().to_rfc3339();
    let due = sqlx::query("SELECT * FROM access_requests WHERE kind = 'share' AND status = 'done' AND until IS NOT NULL AND until < $1").bind(&now).fetch_all(&st.db).await?;
    let mut n = 0;
    for r in due {
        let (id, app): (String, String) = (r.get("id"), r.get::<Option<String>, _>("app").unwrap_or_default());
        sqlx::query("UPDATE access_requests SET status = 'ended', updated_at = $1 WHERE id = $2").bind(&now).bind(&id).execute(&st.db).await?;
        let Some(client_id) = app_record(st, &app).await.map_err(|e| anyhow::anyhow!("{}", e.1))?.and_then(|a| a.client_id) else { continue };
        let by: String = r.get::<Option<String>, _>("approved_by").unwrap_or_else(|| r.get("asked_by"));
        let person = r.get::<Option<String>, _>("result").and_then(|v| serde_json::from_str::<Value>(&v).ok()).and_then(|v| v["person"].as_str().map(String::from));
        let roles = r.get::<String, _>("roles");
        let roles: Value = serde_json::from_str(&roles).unwrap_or(Value::Null);
        let email = r.get::<Option<String>, _>("email");
        let who = Principal { sub: by.clone(), roles: vec![], email: None, claims: Value::Null, token: None, kind: crate::auth::PrincipalKind::Human, scopes: vec![], app: None, session_id: None, platform: None };
        let fields = json!({ "person": person, "email": email, "app": app, "roles": roles, "guest": true, "note": "its end date", "details": { "share": id } });
        let body = json!({ "app": app, "client_id": client_id, "person": person, "email": email, "roles": roles, "guest": true, "by": by });
        new_request(st, &who, "end", "access.revoked", &app, "sent", &fields, body).await.map_err(|e| anyhow::anyhow!("{}", e.1))?;
        n += 1;
    }
    Ok(n)
}

/// `GET /v1/access-requests[?status=&app=]`: the requests about people and access, newest first. Admins see them all; anyone
/// else those they asked, and those about the apps they own.
pub(crate) async fn list_access_requests(InRealm(st): InRealm, Caller(who): Caller, axum::extract::Query(q): axum::extract::Query<BTreeMap<String, String>>) -> Res {
    require_human(&who, "the access requests")?;
    let (status, app) = (q.get("status").cloned().unwrap_or_default(), q.get("app").cloned().unwrap_or_default());
    let rows = sqlx::query("SELECT * FROM access_requests WHERE ($1 = '' OR status = $1) AND ($2 = '' OR app = $2) ORDER BY asked_at DESC LIMIT 500").bind(&status).bind(&app).fetch_all(&st.db).await?;
    let owned: Vec<String> = sqlx::query_scalar("SELECT id FROM apps WHERE owner = $1").bind(&who.sub).fetch_all(&st.db).await?;
    let mine = |r: &sqlx::postgres::PgRow| who.is_admin() || r.get::<String, _>("asked_by") == who.sub || r.get::<Option<String>, _>("app").is_some_and(|a| owned.contains(&a));
    Ok(Json(rows.iter().filter(|r| mine(r)).map(request_json).collect::<Vec<_>>()).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A directory that names its people and nothing more: what their last token carried fills the rest, never over what it
    /// says; someone it names by no id is nobody Cori knows.
    #[test]
    fn what_a_directory_leaves_out_is_filled_from_what_was_seen() {
        let seen = BTreeMap::from([("u1".to_string(), json!({ "id": "u1", "email": "ann@acme.example", "name": "Ann", "last_seen": "2025-10-27T09:00:00Z", "roles": ["builder"], "grants": { "desk": ["agent"] } }))]);
        let mut p = json!({ "id": "u1", "name": "Ann Lee (HR)" });
        fill_from_seen(&mut p, &seen);
        assert_eq!(p, json!({ "id": "u1", "name": "Ann Lee (HR)", "email": "ann@acme.example", "last_seen": "2025-10-27T09:00:00Z", "roles": ["builder"], "grants": { "desk": ["agent"] } }));
        let mut full = json!({ "id": "u1", "email": "a.lee@acme.example", "name": "Ann", "roles": [], "grants": {} });
        fill_from_seen(&mut full, &seen);
        assert_eq!((full["email"].clone(), full["roles"].clone(), full["grants"].clone()), (json!("a.lee@acme.example"), json!([]), json!({})), "what the directory says stays");
        let mut never = json!({ "id": "u2", "email": "bo@acme.example" });
        fill_from_seen(&mut never, &seen);
        assert_eq!(never, json!({ "id": "u2", "email": "bo@acme.example" }));
        assert!(named(&json!({ "id": "u1" })) && !named(&json!({ "email": "ann@acme.example" })) && !named(&json!({ "id": " " })) && !named(&json!("u1")));
    }

    #[tokio::test]
    async fn people_are_recorded_once_per_window_with_what_their_token_says() {
        let Some(db) = crate::db::test_db().await else { eprintln!("CORI_STORE_DATABASE_URL not set: skipped"); return };
        let seen = std::sync::Mutex::new(std::collections::HashMap::new());
        let claims = json!({ "name": "Ann", "resource_access": { "desk": { "roles": ["agent"] }, "account": { "roles": [] } } });
        let mut who = Principal { sub: "u1".into(), roles: vec!["builder".into()], email: Some("ann@acme.example".into()), claims, token: None, kind: crate::auth::PrincipalKind::Human, scopes: vec![], app: None, session_id: None, platform: None };
        record_person(&db, &seen, &who).await;
        // within the window nothing is written again, whatever changed
        who.email = Some("other@acme.example".into());
        record_person(&db, &seen, &who).await;
        let rows = sqlx::query("SELECT sub, email, name, first_at, last_at, roles, app_roles FROM people_seen").fetch_all(&db).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!((rows[0].get::<Option<String>, _>("email").as_deref(), rows[0].get::<Option<String>, _>("name").as_deref()), (Some("ann@acme.example"), Some("Ann")));
        // the roles the token carried: member roles, and the roles in each app's client (none: left out)
        assert_eq!(seen_json(&rows[0])["roles"], json!(["builder"]));
        assert_eq!(seen_json(&rows[0])["grants"], json!({ "desk": ["agent"] }));
        // roles that changed are written at once
        who.roles = vec!["builder".into(), "approver".into()];
        record_person(&db, &seen, &who).await;
        let r = sqlx::query("SELECT sub, email, name, first_at, last_at, roles, app_roles FROM people_seen").fetch_one(&db).await.unwrap();
        assert_eq!(seen_json(&r)["roles"], json!(["builder", "approver"]));
        // and so are roles back to the set written first in the window: a role given, then taken back
        who.roles = vec!["builder".into()];
        record_person(&db, &seen, &who).await;
        let r = sqlx::query("SELECT sub, email, name, first_at, last_at, roles, app_roles FROM people_seen").fetch_one(&db).await.unwrap();
        assert_eq!(seen_json(&r)["roles"], json!(["builder"]));
        // an agent session is never recorded
        let agent = Principal { sub: "u2".into(), kind: crate::auth::PrincipalKind::Agent, ..who.clone() };
        record_person(&db, &seen, &agent).await;
        assert_eq!(sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM people_seen").fetch_one(&db).await.unwrap(), 1);
        // past the window the row is updated, and a token without a name keeps the one known
        seen.lock().unwrap().clear();
        who.claims = json!({});
        record_person(&db, &seen, &who).await;
        let r = sqlx::query("SELECT email, name, app_roles FROM people_seen WHERE sub = 'u1'").fetch_one(&db).await.unwrap();
        assert_eq!((r.get::<Option<String>, _>("email").as_deref(), r.get::<Option<String>, _>("name").as_deref()), (Some("other@acme.example"), Some("Ann")));
        assert_eq!(r.get::<Option<String>, _>("app_roles").as_deref(), Some(r#"{"desk":["agent"]}"#), "a token without app roles keeps those known");
    }
}
