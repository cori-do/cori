// SPDX-License-Identifier: Apache-2.0
//! What the identity provider is told: `cori identity catalog|events|retry`, and the conformance kits an organization runs
//! before it goes live: `cori identity test-hook` against its receiver of the client hook,
//! `cori identity test-people` against a directory of its people, `cori identity test-access` against its access webhook.
use crate::cli::*;
use crate::client::Api;
use serde_json::{Value, json};

pub async fn run(cmd: Cmd) -> anyhow::Result<()> {
    let Cmd::Identity { what } = cmd else { unreachable!("main dispatches only this module's commands") };
    match what {
        IdentityCmd::Catalog { json, conn: Conn { server, token } } => {
            let v = Api::new(&server, &token).get("/v1/identity/catalog").await?;
            if json { println!("{}", serde_json::to_string_pretty(&v)?); return Ok(()); }
            println!("client hook: {} · role events: {} (sent at {})", if v["hook"].as_bool() == Some(true) { "on" } else { "off" }, if v["role_events"].as_bool() == Some(true) { "on" } else { "off" }, v["roles_at"].as_str().unwrap_or("deploy"));
            for a in v["apps"].as_array().cloned().unwrap_or_default() {
                let client = match a["client_status"].as_str() { Some("ready") => a["client_id"].as_str().unwrap_or("").to_string(), Some("pending") => format!("pending ({})", a["client_note"].as_str().unwrap_or("")), _ => "none".into() };
                println!("\n{}  client {client}  {}", a["app"].as_str().unwrap_or(""), a["origin"].as_str().unwrap_or(""));
                for r in a["roles"].as_array().cloned().unwrap_or_default() {
                    let since = r["undeclared_since"].as_str().map(|s| format!(" (no version declares it since {s})")).unwrap_or_default();
                    println!("  {:<24} {:<10} {}{since}", r["role"].as_str().unwrap_or(""), r["state"].as_str().unwrap_or(""), r["description"].as_str().unwrap_or(""));
                }
            }
        }
        IdentityCmd::Events { app, status, limit, json, conn: Conn { server, token } } => {
            let mut q = vec![format!("limit={limit}")];
            if let Some(a) = &app { q.push(format!("app={a}")); }
            if let Some(s) = &status { q.push(format!("status={s}")); }
            let v = Api::new(&server, &token).get(&format!("/v1/identity/events?{}", q.join("&"))).await?;
            if json { println!("{}", serde_json::to_string_pretty(&v)?); return Ok(()); }
            for e in v.as_array().cloned().unwrap_or_default() {
                let detail = match e["status"].as_str() {
                    Some("queued") => format!("next {} · {} attempt(s){}", e["next_at"].as_str().unwrap_or(""), e["attempts"], e["last_answer"].as_str().map(|a| format!(" · last: {a}")).unwrap_or_default()),
                    Some("done") => format!("answered {} at {}", e["last_status"], e["delivered_at"].as_str().unwrap_or("")),
                    _ => e["last_answer"].as_str().unwrap_or("").to_string(),
                };
                println!("{:<36} {:<20} {:<13} {:<9} {detail}", e["id"].as_str().unwrap_or(""), e["app"].as_str().unwrap_or(""), e["event"].as_str().unwrap_or(""), e["status"].as_str().unwrap_or(""));
            }
        }
        IdentityCmd::Retry { id, conn: Conn { server, token } } => {
            let v = Api::new(&server, &token).post(&format!("/v1/identity/events/{id}/retry"), &json!({})).await?;
            println!("event {id} ({}): {}", v["app"].as_str().unwrap_or(""), v["status"].as_str().unwrap_or(""));
        }
        IdentityCmd::TestHook { url, secret_env, wait, origin } => {
            let secret = std::env::var(&secret_env).ok().filter(|s| !s.is_empty());
            let failed = print_report(&test_hook(&url, secret.as_deref(), std::time::Duration::from_secs(wait), &origin).await);
            if secret.is_none() { println!("note: no secret in {secret_env}: deliveries were sent unsigned, as a server without CORI_AUTH_CLIENT_HOOK_SECRET would"); }
            if failed > 0 { anyhow::bail!("the receiver does not follow the client hook's contract"); }
        }
        IdentityCmd::TestPeople { url, secret_env } => {
            let secret = std::env::var(&secret_env).ok().filter(|s| !s.is_empty());
            let failed = print_report(&test_directory(&url, secret.as_deref()).await);
            if secret.is_none() { println!("note: no secret in {secret_env}: questions were sent unsigned, as a server without CORI_IDENTITY_PEOPLE_SECRET would"); }
            if failed > 0 { anyhow::bail!("the directory does not follow its contract"); }
        }
        IdentityCmd::TestAccess { url, secret_env } => {
            let secret = std::env::var(&secret_env).ok().filter(|s| !s.is_empty());
            let failed = print_report(&test_access(&url, secret.as_deref()).await);
            if secret.is_none() { println!("note: no secret in {secret_env}: deliveries were sent unsigned, as a server without CORI_IDENTITY_ACCESS_SECRET would"); }
            if failed > 0 { anyhow::bail!("the access webhook does not follow its contract"); }
        }
    }
    Ok(())
}

/// The checks, one a line, and how many failed.
fn print_report(report: &[Check]) -> usize {
    let (mut failed, mut warned) = (0, 0);
    for c in report {
        match c.verdict { Verdict::Pass => {}, Verdict::Warn => warned += 1, Verdict::Fail => failed += 1 }
        println!("{:<5} {:<44} {}", c.verdict.label(), c.name, c.detail);
    }
    println!("\n{} check(s): {failed} failed, {warned} warning(s)", report.len());
    failed
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Verdict { Pass, Warn, Fail }
impl Verdict { fn label(self) -> &'static str { match self { Verdict::Pass => "pass", Verdict::Warn => "warn", Verdict::Fail => "FAIL" } } }

#[derive(Debug)]
struct Check { name: String, verdict: Verdict, detail: String }

fn check(name: &str, verdict: Verdict, detail: impl Into<String>) -> Check { Check { name: name.into(), verdict, detail: detail.into() } }

/// One delivery as the server sends it: the event's fields, `at` the time given (now, or a stale one), signed with `secret`
/// (or `signature` as given, to send a bad one).
async fn deliver(http: &reqwest::Client, url: &str, event: &str, id: &str, at: chrono::DateTime<chrono::Utc>, mut body: Value, secret: Option<&str>, signature: Option<&str>) -> Result<(u16, Value), String> {
    body["id"] = json!(id); body["event"] = json!(event); body["occurred_at"] = json!(at.to_rfc3339()); body["at"] = json!(at.to_rfc3339());
    let text = body.to_string();
    let mut req = http.post(url).header("content-type", "application/json").header("x-cori-event", event).header("x-cori-delivery", id).header("user-agent", "cori-client-hook/2 (conformance)");
    match (signature, secret) { (Some(s), _) => req = req.header("x-cori-signature", s), (None, Some(sec)) => req = req.header("x-cori-signature", cori_server::client_hook::sign(sec, &text)), (None, None) => {} }
    let r = req.body(text).send().await.map_err(|e| e.to_string())?;
    let status = r.status().as_u16();
    Ok((status, r.json().await.unwrap_or(Value::Null)))
}

fn ok(s: u16) -> bool { (200..300).contains(&s) && s != 202 }

/// A question as the server asks it (`x-cori-question`), `at` the time given (now, or a stale one): its status and answer.
async fn ask(http: &reqwest::Client, url: &str, question: &str, mut body: Value, secret: Option<&str>, at: chrono::DateTime<chrono::Utc>) -> Result<(u16, Value), String> {
    let (id, at) = (uuid::Uuid::new_v4().to_string(), at.to_rfc3339());
    body["id"] = json!(id); body["event"] = json!(question); body["question"] = json!(true); body["occurred_at"] = json!(at); body["at"] = json!(at);
    let text = body.to_string();
    let mut req = http.post(url).header("content-type", "application/json").header("x-cori-event", question).header("x-cori-question", question).header("x-cori-delivery", &id).header("user-agent", "cori-client-hook/2 (conformance)");
    if let Some(sec) = secret { req = req.header("x-cori-signature", cori_server::client_hook::sign(sec, &text)); }
    let r = req.body(text).send().await.map_err(|e| e.to_string())?;
    let status = r.status().as_u16();
    Ok((status, r.json().await.unwrap_or(Value::Null)))
}

/// What a receiver listing people and access checks: each question it lists answered in its shape, and each people event
/// it lists refusing someone who does not exist, with a message (nothing is invited, renamed or removed for real).
async fn test_people(http: &reqwest::Client, url: &str, secret: Option<&str>, caps: &cori_server::client_hook::Capabilities, app: &str, cid: &Value) -> Vec<Check> {
    let mut out = Vec::new();
    if caps.events.is_empty() && caps.questions.is_empty() {
        out.push(check("people and access", Verdict::Pass, "the receiver lists none: the console manages no people here, and shows the people Cori sees"));
        return out;
    }
    let shapes: [(&str, Value, &str); 5] = [
        ("organization.get", json!({ "by": "cori-conformance" }), "name"),
        ("people.list", json!({ "by": "cori-conformance", "cursor": null }), "people"),
        ("invitations.list", json!({ "by": "cori-conformance" }), "invitations"),
        ("access.list", json!({ "app": app, "client_id": cid, "by": "cori-conformance" }), "grants"),
        ("person.organizations", json!({ "person": "cori-conformance-nobody" }), "organizations"),
    ];
    for (q, body, key) in shapes {
        if !caps.answers(q) { continue; }
        let name = format!("{q} is answered");
        match ask(http, url, q, body, secret, chrono::Utc::now()).await {
            Ok((s, b)) if ok(s) && (!b[key].is_null()) && (key == "name" || b[key].is_array()) => out.push(check(&name, Verdict::Pass, format!("http {s}"))),
            Ok((s, b)) => out.push(check(&name, Verdict::Fail, format!("http {s}, without `{key}`: {b}"))),
            Err(e) => out.push(check(&name, Verdict::Fail, e)),
        }
    }
    let nobody = "cori-conformance-nobody";
    let refusals: [(&str, Value); 5] = [
        ("member.roles", json!({ "person": nobody, "roles": ["member"] })),
        ("member.removed", json!({ "person": nobody })),
        ("access.granted", json!({ "app": app, "client_id": cid, "person": nobody, "roles": ["member"], "guest": false, "until": null })),
        ("access.revoked", json!({ "app": app, "client_id": cid, "person": nobody, "roles": ["member"] })),
        ("invitation.resent", json!({ "invitation": "cori-conformance-nothing" })),
    ];
    for (event, mut body) in refusals {
        if !caps.handles(event) { continue; }
        body["request"] = json!(uuid::Uuid::new_v4().to_string()); body["by"] = json!("cori-conformance");
        let name = format!("{event} for nobody is refused, saying why");
        match deliver(http, url, event, &uuid::Uuid::new_v4().to_string(), chrono::Utc::now(), body, secret, None).await {
            Ok((s, b)) if (400..500).contains(&s) && (b["message"].is_string() || b["error"].is_string()) => out.push(check(&name, Verdict::Pass, format!("http {s}"))),
            Ok((202, _)) => out.push(check(&name, Verdict::Warn, "answered 202: its refusal comes later (`request.settled`)")),
            Ok((s, b)) => out.push(check(&name, Verdict::Fail, format!("http {s} {b}: a request about nobody must be refused (4xx) with a `message`"))),
            Err(e) => out.push(check(&name, Verdict::Fail, e)),
        }
    }
    for event in ["member.invited", "organization.updated"] {
        if caps.handles(event) { out.push(check(&format!("{event} is listed"), Verdict::Pass, "not sent: it would email an address, or rename the organization")); }
    }
    out
}

/// The checks of `cori identity test-hook`, in order; a receiver that fails one early may fail the rest for the same reason.
async fn test_hook(url: &str, secret: Option<&str>, wait: std::time::Duration, origin: &str) -> Vec<Check> {
    let http = cori_engine::http::builder().timeout(std::time::Duration::from_secs(15)).build().unwrap_or_default();
    let now = chrono::Utc::now;
    let id = || uuid::Uuid::new_v4().to_string();
    let app = format!("cori-conformance-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    let mut out = Vec::new();

    // reachable, and a ping answered, which lists what it handles besides apps and roles
    let mut caps = cori_server::client_hook::Capabilities::default();
    match deliver(&http, url, "ping", &id(), now(), json!({}), secret, None).await {
        Ok((s, b)) if ok(s) => { caps = cori_server::client_hook::Capabilities::of(&b); out.push(check("ping is answered", Verdict::Pass, format!("http {s}"))); }
        Ok((s, b)) => { out.push(check("ping is answered", Verdict::Fail, format!("http {s} {b}"))); }
        Err(e) => { out.push(check("ping is answered", Verdict::Fail, e)); return out; }
    }
    // signatures and freshness: what keeps anyone else from registering clients
    if secret.is_some() {
        match deliver(&http, url, "ping", &id(), now(), json!({}), secret, Some("sha256=0000000000000000000000000000000000000000000000000000000000000000")).await {
            Ok((s, _)) if (400..500).contains(&s) => out.push(check("a bad signature is refused", Verdict::Pass, format!("http {s}"))),
            Ok((s, _)) => out.push(check("a bad signature is refused", Verdict::Fail, format!("http {s}: anyone who reaches the receiver could register clients"))),
            Err(e) => out.push(check("a bad signature is refused", Verdict::Fail, e)),
        }
        match deliver(&http, url, "ping", &id(), now(), json!({}), None, Some("")).await {
            Ok((s, _)) if (400..500).contains(&s) => out.push(check("an unsigned delivery is refused", Verdict::Pass, format!("http {s}"))),
            Ok((s, _)) => out.push(check("an unsigned delivery is refused", Verdict::Fail, format!("http {s}"))),
            Err(e) => out.push(check("an unsigned delivery is refused", Verdict::Fail, e)),
        }
    } else {
        out.push(check("a bad signature is refused", Verdict::Warn, "not checked: no secret (set it on the server and the receiver)"));
    }
    match deliver(&http, url, "ping", &id(), now() - chrono::Duration::minutes(15), json!({}), secret, None).await {
        Ok((s, _)) if (400..500).contains(&s) => out.push(check("a stale delivery (15 min old) is refused", Verdict::Pass, format!("http {s}"))),
        Ok((s, _)) => out.push(check("a stale delivery (15 min old) is refused", Verdict::Warn, format!("http {s}: a recorded delivery could be replayed later; refuse `at` older than 5 minutes"))),
        Err(e) => out.push(check("a stale delivery (15 min old) is refused", Verdict::Fail, e)),
    }

    // the app's client, at once or later
    let created = json!({ "app": app, "owner": "conformance", "roles": ["member"], "redirect_uris": [format!("{origin}/apps/{app}/callback")], "post_logout_redirect_uris": [format!("{origin}/apps/{app}/")], "origin": origin, "resource": "https://cori.example.invalid" });
    let first = id();
    let deadline = std::time::Instant::now() + wait;
    let mut answer = deliver(&http, url, "app.created", &first, now(), created.clone(), secret, None).await;
    let mut pending = false;
    while let Ok((202, _)) = &answer {
        pending = true;
        if std::time::Instant::now() >= deadline { break; }
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        answer = deliver(&http, url, "app.created", &first, now(), created.clone(), secret, None).await;
    }
    let client = match answer {
        Ok((s, b)) if ok(s) => match b["client_id"].as_str().filter(|c| !c.is_empty()) {
            Some(c) => { out.push(check("app.created answers a client_id", Verdict::Pass, format!("{c}{}", if pending { " (after 202: created later)" } else { "" }))); Some(c.to_string()) }
            None => { out.push(check("app.created answers a client_id", Verdict::Fail, format!("http {s} without `client_id`: {b}"))); None }
        },
        Ok((202, _)) => { out.push(check("app.created answers a client_id", Verdict::Fail, format!("still 202 after {}s (--wait)", wait.as_secs()))); None }
        Ok((s, b)) => { out.push(check("app.created answers a client_id", Verdict::Fail, format!("http {s} {b}"))); None }
        Err(e) => { out.push(check("app.created answers a client_id", Verdict::Fail, e)); None }
    };
    if let Some(c) = client.as_deref() {
        // idempotency: the same delivery again, and a new delivery of the same app
        let again: [(&str, String); 2] = [("the same delivery again answers the same client", first.clone()), ("app.created again answers the same client", id())];
        for (name, delivery) in again {
            match deliver(&http, url, "app.created", &delivery, now(), created.clone(), secret, None).await {
                Ok((s, b)) if ok(s) && b["client_id"].as_str() == Some(c) => out.push(check(name, Verdict::Pass, format!("http {s}"))),
                Ok((s, b)) => out.push(check(name, Verdict::Fail, format!("http {s} {b}"))),
                Err(e) => out.push(check(name, Verdict::Fail, e)),
            }
        }
    }
    // roles, the front moving, the app going
    let cid = json!(client);
    let steps: Vec<(&str, &str, Value)> = vec![
        ("role.created", "role.created is answered", json!({ "app": app, "client_id": cid, "role": "conformance_reader", "description": "Reads, for the conformance check", "note": null })),
        ("role.created", "role.created again is answered (idempotent)", json!({ "app": app, "client_id": cid, "role": "conformance_reader", "description": "Reads, for the conformance check", "note": null })),
        ("role.updated", "role.updated is answered", json!({ "app": app, "client_id": cid, "role": "conformance_reader", "description": "Reads, described again", "note": "group:readers" })),
        ("role.deleted", "role.deleted is answered", json!({ "app": app, "client_id": cid, "role": "conformance_reader" })),
        ("role.deleted", "role.deleted again is answered (idempotent)", json!({ "app": app, "client_id": cid, "role": "conformance_reader" })),
        ("app.updated", "app.updated is answered", json!({ "app": app, "client_id": cid, "owner": "conformance", "redirect_uris": [format!("{origin}/apps/{app}/callback")], "post_logout_redirect_uris": [format!("{origin}/apps/{app}/")], "origin": origin })),
    ];
    for (event, name, body) in steps {
        let mut a = deliver(&http, url, event, &id(), now(), body.clone(), secret, None).await;
        // 202 is a fine answer to these too: the server delivers again until it is answered for good
        let deadline = std::time::Instant::now() + wait;
        let mut waited = false;
        while matches!(a, Ok((202, _))) && std::time::Instant::now() < deadline { waited = true; tokio::time::sleep(std::time::Duration::from_secs(5)).await; a = deliver(&http, url, event, &id(), now(), body.clone(), secret, None).await; }
        match a {
            Ok((s, _)) if ok(s) => out.push(check(name, Verdict::Pass, format!("http {s}{}", if waited { " (after 202)" } else { "" }))),
            Ok((s, b)) => out.push(check(name, Verdict::Fail, format!("http {s} {b}"))),
            Err(e) => out.push(check(name, Verdict::Fail, e)),
        }
    }
    out.extend(test_people(&http, url, secret, &caps, &app, &cid).await);
    match deliver(&http, url, "x.unknown-event", &id(), now(), json!({ "app": app }), secret, None).await {
        Ok((s, _)) if ok(s) => out.push(check("an unknown event is answered 2xx (ignored)", Verdict::Pass, format!("http {s}"))),
        Ok((s, _)) => out.push(check("an unknown event is answered 2xx (ignored)", Verdict::Warn, format!("http {s}: a newer server's events would be retried for days; answer 200 to events you do not handle"))),
        Err(e) => out.push(check("an unknown event is answered 2xx (ignored)", Verdict::Fail, e)),
    }
    for name in ["app.deleted is answered", "app.deleted again is answered (idempotent)"] {
        match deliver(&http, url, "app.deleted", &id(), now(), json!({ "app": app, "client_id": cid }), secret, None).await {
            Ok((s, _)) if ok(s) || s == 202 => out.push(check(name, Verdict::Pass, format!("http {s}"))),
            Ok((s, b)) => out.push(check(name, Verdict::Fail, format!("http {s} {b}"))),
            Err(e) => out.push(check(name, Verdict::Fail, e)),
        }
    }
    out
}

/// The checks of `cori identity test-people`: signatures and freshness, then `people.list` page after page as the server
/// reads them (50 at most), each person named by id, and what the server fills in for what the directory leaves out.
async fn test_directory(url: &str, secret: Option<&str>) -> Vec<Check> {
    let http = cori_engine::http::builder().timeout(std::time::Duration::from_secs(15)).build().unwrap_or_default();
    let now = chrono::Utc::now;
    let list = |cursor: Option<&str>| json!({ "by": "cori-conformance", "cursor": cursor });
    let mut out = Vec::new();
    // signatures and freshness: what keeps anyone else from reading the organization's people
    if secret.is_some() {
        let tries: [(&str, Option<&str>); 2] = [("a bad signature is refused", Some("cori-conformance-another-secret")), ("an unsigned question is refused", None)];
        for (name, with) in tries {
            match ask(&http, url, "people.list", list(None), with, now()).await {
                Ok((s, _)) if (400..500).contains(&s) => out.push(check(name, Verdict::Pass, format!("http {s}"))),
                Ok((s, _)) => out.push(check(name, Verdict::Fail, format!("http {s}: anyone who reaches the directory could read the organization's people"))),
                Err(e) => { out.push(check(name, Verdict::Fail, e)); return out; }
            }
        }
    } else {
        out.push(check("a bad signature is refused", Verdict::Warn, "not checked: no secret (set it on the server and the directory)"));
    }
    match ask(&http, url, "people.list", list(None), secret, now() - chrono::Duration::minutes(15)).await {
        Ok((s, _)) if (400..500).contains(&s) => out.push(check("a stale question (15 min old) is refused", Verdict::Pass, format!("http {s}"))),
        Ok((s, _)) => out.push(check("a stale question (15 min old) is refused", Verdict::Warn, format!("http {s}: a recorded question could be replayed later; refuse `at` older than 5 minutes"))),
        Err(e) => { out.push(check("a stale question (15 min old) is refused", Verdict::Fail, e)); return out; }
    }
    // every page, as the server reads them
    let (mut people, mut pages, mut cursor, mut cursors) = (Vec::new(), 0, None::<String>, std::collections::HashSet::new());
    loop {
        match ask(&http, url, "people.list", list(cursor.as_deref()), secret, now()).await {
            Ok((s, b)) if ok(s) && b["people"].is_array() => {
                pages += 1;
                people.extend(b["people"].as_array().cloned().unwrap_or_default());
                let Some(next) = b["next"].as_str().filter(|n| !n.is_empty()) else { break };
                if !cursors.insert(next.to_string()) { out.push(check("its pages end", Verdict::Fail, format!("the cursor `{next}` comes back: the pages never end"))); return out; }
                if pages == 50 { out.push(check("its pages end", Verdict::Warn, "more than 50 pages: the server reads 50 at most, and then compares nobody (answer larger pages)")); break; }
                cursor = Some(next.to_string());
            }
            Ok((s, b)) => { out.push(check("people.list is answered", Verdict::Fail, format!("http {s}, without `people`{}: {b}", if pages > 0 { format!(" (page {})", pages + 1) } else { String::new() }))); return out; }
            Err(e) => { out.push(check("people.list is answered", Verdict::Fail, e)); return out; }
        }
    }
    out.push(check("people.list is answered", Verdict::Pass, format!("{} people in {pages} page(s)", people.len())));
    let unnamed = people.iter().filter(|p| p["id"].as_str().is_none_or(|id| id.trim().is_empty())).count();
    out.push(if unnamed > 0 {
        check("every person has an id", Verdict::Fail, format!("{unnamed} of {} have none: an id is the subject (`sub`) of the person's tokens (Keycloak: the user's id); the server leaves them out, and compares nobody", people.len()))
    } else if people.is_empty() {
        check("every person has an id", Verdict::Warn, "it lists nobody: the console would show no people, and the server compares nobody")
    } else {
        check("every person has an id", Verdict::Pass, format!("{} people", people.len()))
    });
    let mut ids = std::collections::HashSet::new();
    let twice = people.iter().filter_map(|p| p["id"].as_str()).filter(|id| !ids.insert(*id)).count();
    if twice > 0 { out.push(check("each person once", Verdict::Warn, format!("{twice} listed twice: the console shows them twice"))); }
    // what the server fills in from each person's last token, for what the directory leaves out
    let left_out: Vec<String> = ["email", "name", "roles", "grants"].iter().filter_map(|k| { let n = people.iter().filter(|p| p[*k].is_null()).count(); (n > 0).then(|| format!("{k} ({n})")) }).collect();
    if !left_out.is_empty() { out.push(check("what tokens fill in", Verdict::Pass, format!("not given: {}; Cori shows what each person's last token carried", left_out.join(", ")))); }
    out
}

/// The checks of `cori identity test-access`: signatures and freshness, an event it does not handle answered 200, then
/// `access.granted` and `access.revoked` about nobody (refused, saying why: nothing is granted), the same request again
/// answered alike, and a guest's access without an admin's approval refused.
async fn test_access(url: &str, secret: Option<&str>) -> Vec<Check> {
    let http = cori_engine::http::builder().timeout(std::time::Duration::from_secs(15)).build().unwrap_or_default();
    let now = chrono::Utc::now;
    let id = || uuid::Uuid::new_v4().to_string();
    let app = format!("cori-conformance-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    let mut out = Vec::new();
    // reachable, and a ping answered: an event it does not handle
    match deliver(&http, url, "ping", &id(), now(), json!({}), secret, None).await {
        Ok((s, _)) if ok(s) => out.push(check("ping is answered", Verdict::Pass, format!("http {s}"))),
        Ok((s, b)) => out.push(check("ping is answered", Verdict::Fail, format!("http {s} {b}: answer 200 to events you do not handle"))),
        Err(e) => { out.push(check("ping is answered", Verdict::Fail, e)); return out; }
    }
    // signatures and freshness: what keeps anyone else from granting roles
    if secret.is_some() {
        let tries: [(&str, &str); 2] = [("a bad signature is refused", "sha256=0000000000000000000000000000000000000000000000000000000000000000"), ("an unsigned delivery is refused", "")];
        for (name, signature) in tries {
            match deliver(&http, url, "ping", &id(), now(), json!({}), secret, Some(signature)).await {
                Ok((s, _)) if (400..500).contains(&s) => out.push(check(name, Verdict::Pass, format!("http {s}"))),
                Ok((s, _)) => out.push(check(name, Verdict::Fail, format!("http {s}: anyone who reaches the webhook could grant roles"))),
                Err(e) => out.push(check(name, Verdict::Fail, e)),
            }
        }
    } else {
        out.push(check("a bad signature is refused", Verdict::Warn, "not checked: no secret (set it on the server and the webhook)"));
    }
    match deliver(&http, url, "ping", &id(), now() - chrono::Duration::minutes(15), json!({}), secret, None).await {
        Ok((s, _)) if (400..500).contains(&s) => out.push(check("a stale delivery (15 min old) is refused", Verdict::Pass, format!("http {s}"))),
        Ok((s, _)) => out.push(check("a stale delivery (15 min old) is refused", Verdict::Warn, format!("http {s}: a recorded grant could be replayed later; refuse `at` older than 5 minutes"))),
        Err(e) => out.push(check("a stale delivery (15 min old) is refused", Verdict::Fail, e)),
    }
    // a grant and a revocation about nobody: refused, saying why
    let nobody = |request: &str| json!({ "app": app, "client_id": app, "person": "cori-conformance-nobody", "roles": ["cori_conformance_reader"], "guest": false, "until": null, "request": request, "by": "cori-conformance", "by_email": null });
    let (grant, grant_delivery) = (id(), id());
    let first = deliver(&http, url, "access.granted", &grant_delivery, now(), nobody(&grant), secret, None).await;
    let name = "access.granted for nobody is refused, saying why";
    match &first {
        Ok((s, b)) if (400..500).contains(s) && (b["message"].is_string() || b["error"].is_string()) => out.push(check(name, Verdict::Pass, format!("http {s}"))),
        Ok((202, _)) => out.push(check(name, Verdict::Warn, "answered 202: its refusal comes later (`request.settled`, or an answer to a later delivery)")),
        Ok((s, b)) if ok(*s) => out.push(check(name, Verdict::Fail, format!("http {s} {b}: the console would show it done; refuse (404) a person who does not exist, with a `message`"))),
        Ok((s, b)) => out.push(check(name, Verdict::Fail, format!("http {s} {b}: refuse with a 4xx and a `message`"))),
        Err(e) => out.push(check(name, Verdict::Fail, e.clone())),
    }
    // the same delivery again (Cori delivers until it has an answer for good): answered alike
    if let Ok((s, _)) = &first {
        let name = "the same delivery again is answered alike";
        match deliver(&http, url, "access.granted", &grant_delivery, now(), nobody(&grant), secret, None).await {
            Ok((again, _)) if again == *s => out.push(check(name, Verdict::Pass, format!("http {again}"))),
            Ok((again, b)) => out.push(check(name, Verdict::Warn, format!("http {again} {b}, after http {s}: key your answers on `request` (or `x-cori-delivery`)"))),
            Err(e) => out.push(check(name, Verdict::Fail, e)),
        }
    }
    let name = "access.revoked for nobody is refused, saying why";
    match deliver(&http, url, "access.revoked", &id(), now(), nobody(&id()), secret, None).await {
        Ok((s, b)) if (400..500).contains(&s) && (b["message"].is_string() || b["error"].is_string()) => out.push(check(name, Verdict::Pass, format!("http {s}"))),
        Ok((s, _)) if s == 202 || ok(s) => out.push(check(name, Verdict::Warn, format!("http {s}: the console would show it done (or pending); refusing (404) says the person is unknown"))),
        Ok((s, b)) => out.push(check(name, Verdict::Fail, format!("http {s} {b}: refuse with a 4xx and a `message`"))),
        Err(e) => out.push(check(name, Verdict::Fail, e)),
    }
    // someone outside the organization, approved by nobody who is an admin there
    let guest = json!({ "app": app, "client_id": app, "email": "nobody@cori-conformance.example.invalid", "roles": ["cori_conformance_reader"], "guest": true, "until": null, "note": null, "request": id(), "by": "cori-conformance", "approved_by": "cori-conformance-nobody" });
    let name = "a guest's access without an admin's approval is refused";
    match deliver(&http, url, "access.granted", &id(), now(), guest, secret, None).await {
        Ok((s, _)) if (400..500).contains(&s) => out.push(check(name, Verdict::Pass, format!("http {s}"))),
        Ok((s, b)) => out.push(check(name, Verdict::Warn, format!("http {s} {b}: check that `approved_by` is one of your admins (or refuse guests if you take none)"))),
        Err(e) => out.push(check(name, Verdict::Fail, e)),
    }
    match deliver(&http, url, "x.unknown-event", &id(), now(), json!({ "app": app }), secret, None).await {
        Ok((s, _)) if ok(s) => out.push(check("an unknown event is answered 2xx (ignored)", Verdict::Pass, format!("http {s}"))),
        Ok((s, _)) => out.push(check("an unknown event is answered 2xx (ignored)", Verdict::Warn, format!("http {s}: a newer server's events would be retried for days; answer 200 to events you do not handle"))),
        Err(e) => out.push(check("an unknown event is answered 2xx (ignored)", Verdict::Fail, e)),
    }
    out
}
