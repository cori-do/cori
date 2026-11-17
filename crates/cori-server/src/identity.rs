// SPDX-License-Identifier: Apache-2.0
//! The identity a run presents to connectors (builtin auth, e.g. Keycloak), for as long as the run lasts.
//!
//! A person's access token lives minutes (5 on a default Keycloak realm) while a run can wait days on a `human` step.
//! At launch the caller's token is exchanged for a refresh token of the platform client (standard token exchange with
//! "Allow refresh token in Standard Token Exchange"); the pair is sealed on the run row. A lease then refreshes the
//! access token when it is about to expire, exchanges it for the connector's audience, and caches the result per
//! (run, audience) until shortly before it expires. Without a refresh token the lease works until the caller's token
//! expires and then says how to fix the realm.
//!
//! Scheduled runs have no caller: they act as a service account, a confidential client of the issuer whose secret is
//! in the broker. Its roles come from its own token (`service_account_user`), never from the workflow's YAML.
use crate::state::AppState;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Seconds before expiry at which a token is treated as expired (clock skew, request time).
const MARGIN: i64 = 30;

/// Short-lived tokens the server obtained, by key, with their expiry (unix seconds).
#[derive(Default)]
pub struct TokenCache { map: Mutex<HashMap<String, (String, i64)>> }

impl TokenCache {
    fn get(&self, key: &str) -> Option<String> {
        let now = chrono::Utc::now().timestamp();
        let mut m = self.map.lock().ok()?;
        m.retain(|_, (_, exp)| *exp - MARGIN > now);
        m.get(key).map(|(t, _)| t.clone())
    }
    fn put(&self, key: String, token: &str) {
        let exp = crate::auth::jwt_exp(token).unwrap_or_else(|| chrono::Utc::now().timestamp() + 60);
        if let Ok(mut m) = self.map.lock() { m.insert(key, (token.to_string(), exp)); }
    }
    pub fn forget_run(&self, realm: Option<&crate::db::Realm>, run_id: &str) {
        let prefix = crate::names::run_token_prefix(realm, run_id);
        if let Ok(mut m) = self.map.lock() { m.retain(|k, _| !k.starts_with(&prefix)); }
    }
}

/// What is sealed on a run row: the caller's access token and, when the issuer granted one, a refresh token.
/// Rows written before refresh tokens existed hold the bare access token.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunTokens { pub access: String, #[serde(default)] pub refresh: Option<String> }

impl RunTokens {
    pub fn parse(plain: &str) -> Self {
        if plain.trim_start().starts_with('{') { if let Ok(t) = serde_json::from_str(plain) { return t; } }
        RunTokens { access: plain.to_string(), refresh: None }
    }
}

fn aad(st: &AppState, run_id: &str) -> String { crate::names::run_aad(st.db.realm(), run_id) }

/// Seals the caller's token for a run starting now, as it is: the run's row holds it from the start (`refresh_for_run`
/// adds a refresh token once the run is under way).
pub fn seal_for_run(st: &AppState, run_id: &str, access: &str) -> anyhow::Result<String> {
    st.broker.seal_value(&serde_json::to_string(&RunTokens { access: access.to_string(), refresh: None })?, &aad(st, run_id))
}

/// Under builtin auth, once a run is under way: asks for a refresh token for it (`CORI_AUTH_RUN_REFRESH=false` skips that;
/// `CORI_AUTH_RUN_OFFLINE=true` asks for an offline one, for runs that outlive the SSO session) and seals the pair on its
/// row, unless it has ended meanwhile. The run does not wait for it: the caller's own token lasts minutes. A refused
/// exchange is logged and the run keeps the bare access token.
pub async fn refresh_for_run(st: &Arc<AppState>, run_id: &str, access: &str) {
    if !(matches!(st.cfg.auth, crate::auth::AuthMode::Builtin { .. }) && st.cfg.run_refresh) { return; }
    let (access, refresh) = match st.auth.exchange_for_refresh(access, st.cfg.run_offline).await {
        Ok(t) => t,
        Err(e) => { tracing::warn!(run = run_id, error = %e, "no refresh token for the run: connector calls stop working when the caller's access token expires"); return; }
    };
    let sealed = serde_json::to_string(&RunTokens { access, refresh: Some(refresh) }).map_err(anyhow::Error::from).and_then(|t| st.broker.seal_value(&t, &aad(st, run_id)));
    let r = match sealed {
        Ok(s) => sqlx::query("UPDATE runs SET principal_token = $1 WHERE id = $2 AND status = 'running'").bind(s).bind(run_id).execute(&st.db).await.map(|_| ()).map_err(anyhow::Error::from),
        Err(e) => Err(e),
    };
    if let Err(e) = r { tracing::warn!(run = run_id, error = %e, "the run's refresh token was not recorded: connector calls stop working when the caller's access token expires"); }
}

/// A valid access token of the run's own user: the sealed one while it lasts, else a refreshed one (re-sealed, since
/// the issuer may rotate the refresh token).
async fn run_access_token(st: &Arc<AppState>, run_id: &str, sealed: &str) -> Result<String, String> {
    let plain = st.broker.open_value(sealed, &aad(st, run_id)).map_err(|e| e.to_string())?;
    let tokens = RunTokens::parse(&plain);
    let now = chrono::Utc::now().timestamp();
    if crate::auth::jwt_exp(&tokens.access).map(|exp| exp - MARGIN > now).unwrap_or(true) { return Ok(tokens.access); }
    let Some(refresh) = tokens.refresh else {
        return Err(format!("the run's identity expired: the caller's access token ended at {} and no refresh token was obtained when the run started \
            (Keycloak: enable \"Allow refresh token in Standard Token Exchange\" on the platform client, or raise the access token lifespan)",
            crate::auth::jwt_exp(&tokens.access).and_then(|e| chrono::DateTime::from_timestamp(e, 0)).map(|d| d.to_rfc3339()).unwrap_or_default()));
    };
    let (access, rotated) = st.auth.refresh(&refresh).await.map_err(|e| format!("the run's identity could not be refreshed (SSO session ended? CORI_AUTH_RUN_OFFLINE=true keeps it for long runs): {e}"))?;
    let next = RunTokens { access: access.clone(), refresh: Some(rotated.unwrap_or(refresh)) };
    if let Ok(resealed) = st.broker.seal_value(&serde_json::to_string(&next).unwrap_or_default(), &aad(st, run_id)) {
        let _ = sqlx::query("UPDATE runs SET principal_token = $1 WHERE id = $2").bind(resealed).bind(run_id).execute(&st.db).await;
    }
    Ok(access)
}

/// The token a connector with `audience` receives for this run: the run's user exchanged for that audience, or for a
/// scheduled run its service account's. Cached per (run, audience).
pub async fn audience_token(st: &Arc<AppState>, run_id: &str, app: &str, sealed: Option<&str>, user: &Value, audience: &str) -> Result<String, String> {
    let key = crate::names::run_token_key(st.db.realm(), run_id, audience);
    if let Some(t) = st.tokens.get(&key) { return Ok(t); }
    let subject = match sealed {
        Some(s) => run_access_token(st, run_id, s).await?,
        None if user.get("service_account").and_then(|v| v.as_bool()) == Some(true) => {
            let client = user.get("client_id").and_then(|v| v.as_str()).ok_or("the scheduled run's service account has no client_id")?;
            service_account_token(st, Some(app), client, user.get("secret").and_then(|v| v.as_str())).await?
        }
        None => return Err("the run has no identity to exchange (started without a token)".into()),
    };
    let token = st.auth.exchange(&subject, audience, &st.cfg.auth_client_id).await?;
    st.tokens.put(key, &token);
    Ok(token)
}

/// The broker secret holding a service account's client secret: `schedule_as.secret`, else `schedule.<client_id>`.
pub fn service_account_secret_name(client_id: &str, secret: Option<&str>) -> String {
    secret.map(String::from).unwrap_or_else(|| format!("schedule.{client_id}"))
}

/// A token of the service account `client_id` (client credentials, secret from the broker: `app`'s own, else the
/// deployment's), cached until it expires.
pub async fn service_account_token(st: &Arc<AppState>, app: Option<&str>, client_id: &str, secret: Option<&str>) -> Result<String, String> {
    let key = crate::names::client_token_key(st.db.realm(), client_id);
    if let Some(t) = st.tokens.get(&key) { return Ok(t); }
    let name = service_account_secret_name(client_id, secret);
    let found = match app { Some(a) => st.broker.resolve(&st.db, a, &name).await.map(|(v, _)| v), None => st.broker.reveal(&st.db, &name).await };
    let value = found.map_err(|_| match app {
        Some(a) => format!("service account `{client_id}`: no secret `{name}` in the broker for app `{a}` (`cori secrets set {name} --app {a}`)"),
        None => format!("service account `{client_id}`: no secret `{name}` in the broker (`cori secrets set {name}`)"),
    })?;
    let token = st.auth.client_credentials(client_id, &value).await.map_err(|e| format!("service account `{client_id}`: {e}"))?;
    st.tokens.put(key, &token);
    Ok(token)
}

/// The user a scheduled workflow runs as. Under builtin auth: the service account's own token decides its subject and
/// roles (`schedule_as.client_id`, secret in the broker); roles written in the workflow are ignored. Under dev auth:
/// the workflow's `schedule_as` as written (nothing is verified in dev anyway).
pub async fn service_account_user(st: &Arc<AppState>, app: Option<&str>, schedule_as: &Value) -> Result<Value, String> {
    let client = schedule_as.get("client_id").or_else(|| schedule_as.get("service_account")).and_then(|v| v.as_str());
    let secret = schedule_as.get("secret").and_then(|v| v.as_str());
    match &st.cfg.auth {
        crate::auth::AuthMode::Builtin { issuer, audience, roles_claim, subject_claim } => {
            let client = client.ok_or("`schedule_as.client_id` is required: scheduled runs act as a service account of the issuer")?;
            let token = service_account_token(st, app, client, secret).await?;
            let claims = st.auth.verify_issuer_token(issuer, audience.as_deref().as_slice(), &token).await
                .map_err(|e| format!("service account `{client}`: its token is not accepted by this server ({e}); add the platform audience to the client"))?;
            // named as people are (`subject_claim`); a claim only people's tokens carry (an address) falls back to `sub`
            let sub = crate::auth::subject(&claims, subject_claim).or_else(|| crate::auth::subject(&claims, "sub")).unwrap_or_default().to_string();
            // under a tenancy, the account is one of the schedule's organization
            if let Some(tenancy) = &st.tenancy {
                let account = crate::auth::Principal { sub: sub.clone(), roles: vec![], email: None, claims: claims.clone(), token: None, kind: crate::auth::PrincipalKind::Service, scopes: vec![], app: None, session_id: None, platform: None };
                let realm = tenancy.realm_of(&st.db.outside(), &account).await.map_err(|e| format!("service account `{client}`: {}", e.1))?;
                if Some(&realm) != st.realm() { return Err(format!("service account `{client}` belongs to another organization")); }
            }
            let mut roles: Vec<Value> = crate::auth::lookup(&claims, roles_claim).and_then(|v| v.as_array()).cloned().unwrap_or_default();
            // an app with a client of its own: the account's roles in the app are that client's roles in its token
            // (`auth::app_roles_claim`, as for a person signed in to the app's front)
            if let Some(app) = app && let Some(cid) = sqlx::query_scalar::<_, Option<String>>("SELECT client_id FROM apps WHERE id = $1").bind(app).fetch_optional(&st.db).await.ok().flatten().flatten() {
                let app_roles = crate::auth::app_roles_claim().roles(&claims, &cid);
                if !app_roles.is_empty() { roles = app_roles.into_iter().map(Value::from).collect(); }
            }
            if schedule_as.get("roles").is_some() { tracing::warn!(client, "schedule_as.roles is ignored under builtin auth: roles come from the service account's token"); }
            let mut user = json!({ "sub": sub, "roles": roles, "service_account": true, "client_id": client });
            if let Some(s) = secret { user["secret"] = json!(s); }
            Ok(user)
        }
        _ => {
            let roles: Vec<Value> = schedule_as.get("roles").and_then(|r| r.as_array()).cloned().unwrap_or_default();
            Ok(json!({ "sub": client.unwrap_or("schedule"), "roles": roles, "service_account": true, "client_id": client }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sealed_forms() {
        assert_eq!(RunTokens::parse("eyJ.a.b").access, "eyJ.a.b");
        let t = RunTokens::parse(r#"{"access":"a","refresh":"r"}"#);
        assert_eq!((t.access.as_str(), t.refresh.as_deref()), ("a", Some("r")));
    }

    #[test]
    fn cache_expires() {
        let c = TokenCache::default();
        let b64 = |v: &Value| base64::Engine::encode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, v.to_string());
        let now = chrono::Utc::now().timestamp();
        let fresh = format!("h.{}.s", b64(&json!({ "exp": now + 600 })));
        let stale = format!("h.{}.s", b64(&json!({ "exp": now + 10 })));
        c.put("run:r1:aud".into(), &fresh); c.put("run:r2:aud".into(), &stale);
        assert_eq!(c.get("run:r1:aud").as_deref(), Some(fresh.as_str()));
        assert!(c.get("run:r2:aud").is_none());
        c.forget_run(None, "r1");
        assert!(c.get("run:r1:aud").is_none());
    }
}
