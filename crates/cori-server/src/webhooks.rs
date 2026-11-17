// SPDX-License-Identifier: Apache-2.0
//! Outgoing webhooks: `[[webhooks]]` in cori.toml, HMAC-SHA256 signed, three attempts with backoff, deliveries logged.
//! A builder writes the URL and the server posts from inside its network: a delivery goes only to an address the app
//! egress allows (`CORI_APP_EGRESS`: public addresses by default, private networks too with `private`, never the server's
//! own machine or its cloud metadata endpoint), pinned to the address checked, and never follows a redirect.
use crate::state::AppState;
use cori_engine::config::WebhookConfig;
use cori_engine::egress::Egress;
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::sync::Arc;

pub async fn deliver(st: &Arc<AppState>, app: &str, hooks: &[WebhookConfig], event: &str, payload: Value) {
    for h in hooks.iter().filter(|h| h.events.iter().any(|e| e == event || e == "*")) {
        if h.format == "email" { email(st, app, h, event, &payload).await; continue; }
        let secret = match (&h.secret, &h.secret_env) { (Some(n), _) => st.broker.resolve(&st.db, app, n).await.ok().map(|(v, _)| v), (None, Some(e)) => cori_engine::activities::env_credential(e), _ => None };
        if secret.is_none() && (h.secret.is_some() || h.secret_env.is_some()) { tracing::warn!(app, url = %h.url, "webhook sent unsigned: its secret is unknown, empty, or one of Cori's own CORI_* variables"); }
        let body = if h.format == "slack" { slack_message(st, app, h, event, &payload).to_string() } else { json!({ "id": uuid::Uuid::new_v4().to_string(), "event": event, "at": chrono::Utc::now().to_rfc3339(), "app": app, "data": payload }).to_string() };
        let sig = secret.as_deref().map(|s| { use hmac::{Hmac, Mac, KeyInit}; let mut m = <Hmac<sha2::Sha256>>::new_from_slice(s.as_bytes()).expect("hmac key"); m.update(body.as_bytes()); format!("sha256={}", hex::encode(m.finalize().into_bytes())) });
        let mut client = cori_engine::http::builder().timeout(std::time::Duration::from_secs(10)).redirect(reqwest::redirect::Policy::none());
        match destination(&h.url, st.cfg.app_egress()).await {
            Ok(Some((host, addr))) => client = client.resolve(&host, addr),
            Ok(None) => {}
            Err(e) => { log(st, app, event, &h.url, 0, 0, Some(format!("refused: {e}"))).await; continue; }
        }
        let client = client.build().unwrap();
        let (mut status, mut attempts, mut error) = (0i64, 0i64, None::<String>);
        for i in 0..3 {
            attempts = i + 1;
            let mut req = client.post(&h.url).header("content-type", "application/json").header("x-cori-event", event).header("user-agent", "cori-webhooks/2");
            if let Some(s) = &sig { req = req.header("x-cori-signature", s); }
            match req.body(body.clone()).send().await {
                Ok(r) => { status = r.status().as_u16() as i64; if r.status().is_success() { error = None; break; } error = Some(format!("http {status}")); }
                Err(e) => { error = Some(e.to_string()); }
            }
            tokio::time::sleep(std::time::Duration::from_millis(500 * (1 << i))).await;
        }
        log(st, app, event, &h.url, status, attempts, error).await;
    }
}

async fn log(st: &Arc<AppState>, app: &str, event: &str, url: &str, status: i64, attempts: i64, error: Option<String>) {
    let _ = sqlx::query("INSERT INTO webhook_deliveries (id, app, event, url, status, attempts, at, error) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)")
        .bind(uuid::Uuid::new_v4().to_string()).bind(app).bind(event).bind(url).bind(status).bind(attempts).bind(chrono::Utc::now().to_rfc3339()).bind(&error).execute(&st.db).await;
    if let Some(e) = &error { tracing::warn!(url, event, "webhook delivery failed: {e}"); }
}

/// Where a webhook may be delivered under `egress` (`cori_engine::egress`): none to pin to for an IP literal or `any`.
async fn destination(url: &str, egress: Egress) -> Result<Option<(String, SocketAddr)>, String> {
    let u = reqwest::Url::parse(url).map_err(|e| format!("{url}: {e}"))?;
    cori_engine::egress::destination(&u, egress).await.map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_webhook_goes_where_the_app_egress_allows() {
        for refused in ["http://127.0.0.1:8123/hook", "http://localhost/hook", "http://[::1]/hook", "http://169.254.169.254/latest", "file:///etc/passwd"] {
            assert!(destination(refused, Egress::Private).await.is_err(), "{refused}");
        }
        assert!(destination("https://10.0.0.7/hook", Egress::Public).await.is_err(), "a private network needs CORI_APP_EGRESS=private");
        assert!(destination("https://10.0.0.7/hook", Egress::Private).await.unwrap().is_none(), "an IP literal is checked as written");
        assert!(destination("http://127.0.0.1:8123/hook", Egress::Any).await.is_ok(), "the laptop modes");
    }
}

/// A Slack incoming-webhook message: one line per event, with a button to the console page where the human acts.
fn slack_message(st: &Arc<AppState>, app: &str, h: &WebhookConfig, event: &str, p: &Value) -> Value {
    let console = h.console_url.clone().unwrap_or_else(|| st.cfg.public_url.clone());
    let (inbox, run_page) = (crate::names::console_approvals_url(&console, app), crate::names::console_run_url(&console, app, p["run_id"].as_str().unwrap_or("")));
    let (text, url, button) = match event {
        "approval.pending" => (format!(":raised_hand: *{app}* · `{}` needs a *{}* decision (requested by {})", p["step"].as_str().unwrap_or(""), p["to"].as_str().unwrap_or(""), p["payload"]["requested_by"].as_str().or_else(|| p["payload"]["user"].as_str()).unwrap_or("a run")), inbox, "Open inbox"),
        "run.failed" => (format!(":x: *{app}* · `{}` failed: {}", p["workflow"].as_str().unwrap_or(""), p["error"].as_str().unwrap_or("").chars().take(200).collect::<String>()), run_page.clone(), "Open run"),
        _ => (format!(":white_check_mark: *{app}* · `{}` {}{}", p["workflow"].as_str().unwrap_or(""), p["status"].as_str().unwrap_or(""), p["result"]["headline"].as_str().map(|h| format!(" — {h}")).unwrap_or_default()), run_page, "Open run"),
    };
    let mut elements = vec![json!({ "type": "button", "text": { "type": "plain_text", "text": button }, "url": url })];
    if event == "approval.pending" && st.cfg.slack_signing_secret.is_some() {
        let id = p["approval_id"].as_str().unwrap_or("");
        elements.insert(0, json!({ "type": "button", "style": "primary", "action_id": "cori_approve", "text": { "type": "plain_text", "text": "Approve" }, "value": crate::names::slack_action(st.db.realm(), "approve", app, id) }));
        elements.insert(1, json!({ "type": "button", "style": "danger", "action_id": "cori_decline", "text": { "type": "plain_text", "text": "Decline" }, "value": crate::names::slack_action(st.db.realm(), "decline", app, id) }));
    }
    json!({ "text": text, "blocks": [
        { "type": "section", "text": { "type": "mrkdwn", "text": text } },
        { "type": "actions", "elements": elements }
    ] })
}

/// Email notification: plain text with a console link, sent through `CORI_SMTP_URL` (smtp://[user:pass@]host:port,
/// `smtps://` for implicit TLS) from `CORI_SMTP_FROM`. Delivery is logged like a webhook (status 250 on success).
async fn email(st: &Arc<AppState>, app: &str, h: &WebhookConfig, event: &str, p: &Value) {
    use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};
    let Some((url, from)) = st.cfg.smtp.clone() else { tracing::warn!("email webhook configured but CORI_SMTP_URL / CORI_SMTP_FROM are not set"); return };
    let console = h.console_url.clone().unwrap_or_else(|| st.cfg.public_url.clone());
    let (inbox, run_page) = (crate::names::console_approvals_url(&console, app), crate::names::console_run_url(&console, app, p["run_id"].as_str().unwrap_or("")));
    let brand = &st.brand.name;
    let (subject, body) = match event {
        "approval.pending" => (format!("[{brand}] {app}: {} needs a {} decision", p["step"].as_str().unwrap_or(""), p["to"].as_str().unwrap_or("")), format!("A run of {app} is waiting for a decision on step `{}` (routed to {}).\n\nRequested by: {}\nPayload: {}\n\nDecide in the console: {inbox}\n", p["step"].as_str().unwrap_or(""), p["to"].as_str().unwrap_or(""), p["payload"]["requested_by"].as_str().or_else(|| p["payload"]["user"].as_str()).unwrap_or("a run"), serde_json::to_string_pretty(&p["payload"]["payload"]).unwrap_or_default())),
        "run.failed" => (format!("[{brand}] {app}: {} failed", p["workflow"].as_str().unwrap_or("")), format!("Run {} of workflow {} failed:\n{}\n\nOpen: {run_page}\n", p["run_id"].as_str().unwrap_or(""), p["workflow"].as_str().unwrap_or(""), p["error"].as_str().unwrap_or(""))),
        _ => (format!("[{brand}] {app}: {} {}", p["workflow"].as_str().unwrap_or(""), p["status"].as_str().unwrap_or("")), format!("Run {} of workflow {} {}.\n{}\n\nOpen: {run_page}\n", p["run_id"].as_str().unwrap_or(""), p["workflow"].as_str().unwrap_or(""), p["status"].as_str().unwrap_or(""), p["result"]["headline"].as_str().unwrap_or(""))),
    };
    let transport = match AsyncSmtpTransport::<Tokio1Executor>::from_url(&url).map(|b| b.timeout(Some(std::time::Duration::from_secs(10))).build()) { Ok(t) => t, Err(e) => { tracing::warn!(error = %e, "bad CORI_SMTP_URL"); return } };
    let (mut status, mut error) = (250i64, None::<String>);
    for to in &h.to {
        let msg = match Message::builder().from(from.parse().unwrap_or_else(|_| "cori@localhost".parse().unwrap())).to(match to.parse() { Ok(m) => m, Err(_) => { error = Some(format!("bad recipient {to}")); status = 0; continue } }).subject(&subject).body(body.clone()) { Ok(m) => m, Err(e) => { error = Some(e.to_string()); status = 0; continue } };
        if let Err(e) = transport.send(msg).await { error = Some(e.to_string()); status = 0; tracing::warn!(to, error = %e, "email delivery failed"); }
    }
    let _ = sqlx::query("INSERT INTO webhook_deliveries (id, app, event, url, status, attempts, at, error) VALUES ($1, $2, $3, $4, $5, 1, $6, $7)")
        .bind(uuid::Uuid::new_v4().to_string()).bind(app).bind(event).bind(format!("mailto:{}", h.to.join(","))).bind(status).bind(chrono::Utc::now().to_rfc3339()).bind(&error).execute(&st.db).await;
}
