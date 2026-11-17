// SPDX-License-Identifier: Apache-2.0
//! The deployment's connectors, and the connections their calls carry.
//!
//! A deployment declares, in a file (`CORI_CONNECTORS_FILE`, YAML), connectors every realm's registry shows: MCP servers
//! over HTTP that nobody in a realm changes or removes, their tools read when the server starts (without a token: the
//! servers this is for answer `tools/list` to anyone), and the OAuth providers some of them call with. A provider's
//! client is the deployment's, its id and secret in the environment variables the file names; a provider whose variables
//! are not set is off, and so are its connectors.
//!
//! A connection is an account at a provider, its refresh token sealed with the realm's key: a person's own (each person
//! connects theirs once, for the scopes of the provider's connectors: `POST /v1/connections/{provider}`, the provider's
//! consent page, then `GET /v1/connections/callback`), or an app's, which its owner or an admin connects
//! (`/v1/apps/{app}/connections/{provider}`) for runs with no person (schedules) or an account its people share
//! (`account = "app"` on the connector in cori.toml). A run's lease (`/v1/leases` with `connection`) gets an access token,
//! refreshed here and kept until a minute before it expires. Starting a run checks the connections it needs first
//! (`before_run`), so that it does not stop halfway.
//!
//! Only the deployment's connectors call with a connection: an app's declaration and an admin's never do
//! (`AppConfig::merge_registry_hosted`, `api::declare_connector`), since the token would go wherever they point. A run an
//! agent session started never gets a person's token: its code is the agent's, and so is its output.
use crate::api::{ApiError, Caller, InRealm};
use crate::auth::{Principal, PrincipalKind};
use crate::state::AppState;
use axum::Json;
use axum::extract::{Path, Query};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use cori_engine::config::{ConnectorConfig, OAuthUse, ResolvedConnector};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::Row;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex, RwLock};

type Res = Result<Response, ApiError>;

/// How long a consent may take, from its start to the provider's answer.
const CONSENT_SECS: i64 = 900;
/// How often the deployment's connectors' tools are read again; a connector that did not answer is asked a minute later.
const TOOLS_EVERY: std::time::Duration = std::time::Duration::from_secs(6 * 3600);
/// What the state of a consent in flight is sealed for.
const STATE_AAD: &str = "connection-state";
/// The parameters of an authorization request that are Cori's, which a provider's `params` cannot set.
const OWN_PARAMS: [&str; 7] = ["response_type", "client_id", "redirect_uri", "scope", "state", "code_challenge", "code_challenge_method"];

// ---- the deployment's file ----

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    #[serde(default)] oauth: BTreeMap<String, ProviderEntry>,
    #[serde(default)] connectors: BTreeMap<String, ConnectorEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProviderEntry {
    #[serde(default)] label: Option<String>,
    authorize_url: String,
    token_url: String,
    #[serde(default)] revoke_url: Option<String>,
    client_id_env: String,
    client_secret_env: String,
    #[serde(default)] scopes: Vec<String>,
    #[serde(default)] params: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConnectorEntry {
    #[serde(default)] description: String,
    url: String,
    #[serde(default)] oauth: Option<String>,
    #[serde(default)] scopes: Vec<String>,
    #[serde(default)] roles: Vec<String>,
}

/// An OAuth provider of the deployment's: where people consent, where tokens come from, and the deployment's client.
#[derive(Clone)]
pub struct Provider {
    pub name: String,
    /// What people read (`Google`).
    pub label: String,
    pub authorize_url: String,
    pub token_url: String,
    pub revoke_url: Option<String>,
    client_id: String,
    client_secret: String,
    /// Asked with every connection besides its connectors' (`openid`, `email`: whose account it is).
    pub scopes: Vec<String>,
    /// Sent with every authorization request (`access_type=offline`, `prompt=consent` for Google's refresh tokens).
    pub params: BTreeMap<String, String>,
}

// the client's secret never in a log
impl std::fmt::Debug for Provider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Provider").field("name", &self.name).field("authorize_url", &self.authorize_url).field("token_url", &self.token_url).field("client_id", &self.client_id).finish_non_exhaustive()
    }
}

/// A connector of the deployment's: an MCP server over HTTP, the provider its calls carry a token of, and the scopes its
/// tools need.
#[derive(Debug, Clone)]
pub struct Connector { pub name: String, pub description: String, pub url: String, pub provider: Option<String>, pub scopes: Vec<String>, pub roles: Vec<String> }

impl Connector {
    /// Its registry declaration.
    pub fn config(&self) -> ConnectorConfig {
        let oauth = self.provider.as_ref().map(|p| OAuthUse { provider: p.clone(), scopes: self.scopes.clone() });
        serde_json::from_value(json!({ "kind": "mcp", "url": self.url, "oauth": oauth })).expect("a connector's declaration")
    }

    /// As a worker calls it (for its tools).
    fn resolved(&self) -> ResolvedConnector {
        ResolvedConnector { name: self.name.clone(), kind: "mcp".into(), base_url: None, auth_env: None, task_queue: None, url: Some(self.url.clone()), command: None, worker: None, secret: None, audience: None, mock: None, egress: cori_engine::egress::Egress::Any, oauth: None, app_account: false }
    }
}

/// The deployment's connectors and providers, the tools last read from them, and the access tokens of the connections
/// that called lately (in this process).
pub struct Deployment {
    pub providers: BTreeMap<String, Provider>,
    pub connectors: BTreeMap<String, Connector>,
    /// The deployment's key for the state of a consent in flight, which the callback opens before it knows the realm.
    state_key: [u8; 32],
    tools: RwLock<BTreeMap<String, (Vec<Value>, String)>>,
    tokens: Mutex<HashMap<String, (String, i64)>>,
}

impl Deployment {
    /// No connector of the deployment's.
    pub fn none(state_key: [u8; 32]) -> Self { Self { providers: BTreeMap::new(), connectors: BTreeMap::new(), state_key, tools: Default::default(), tokens: Default::default() } }

    /// `CORI_CONNECTORS_FILE`, read and checked: a file that does not read stops the server.
    pub fn from_env(state_key: [u8; 32]) -> anyhow::Result<Self> {
        let Some(path) = std::env::var("CORI_CONNECTORS_FILE").ok().map(|p| p.trim().to_string()).filter(|p| !p.is_empty()) else { return Ok(Self::none(state_key)) };
        let text = std::fs::read_to_string(&path).map_err(|e| anyhow::anyhow!("CORI_CONNECTORS_FILE `{path}`: {e}"))?;
        Self::parse(&path, &text, |k| std::env::var(k).ok(), state_key)
    }

    fn parse(path: &str, text: &str, env: impl Fn(&str) -> Option<String>, state_key: [u8; 32]) -> anyhow::Result<Self> {
        use anyhow::bail;
        let file: File = serde_yaml::from_str(text).map_err(|e| anyhow::anyhow!("{path}: {e}"))?;
        let declared: BTreeSet<String> = file.oauth.keys().cloned().collect();
        let mut providers = BTreeMap::new();
        for (name, p) in file.oauth {
            if !provider_name(&name) { bail!("{path}: provider `{name}`: 1 to 32 lowercase letters, digits and `-`, not `callback`"); }
            for (what, u) in [("authorize_url", Some(&p.authorize_url)), ("token_url", Some(&p.token_url)), ("revoke_url", p.revoke_url.as_ref())] {
                if let Some(problem) = u.and_then(|u| url_problem(u)) { bail!("{path}: provider `{name}`: {what}: {problem}"); }
            }
            for var in [&p.client_id_env, &p.client_secret_env] {
                // a setting of Cori's own (its master key) would go to the provider's token endpoint
                if var.starts_with("CORI_") && !var.starts_with("CORI_OAUTH_") { bail!("{path}: provider `{name}`: `{var}` is one of Cori's own settings; name a variable of its own (CORI_OAUTH_*)"); }
            }
            if let Some(k) = p.params.keys().find(|k| OWN_PARAMS.contains(&k.as_str())) { bail!("{path}: provider `{name}`: `{k}` is Cori's to send, not a parameter of the file"); }
            if let Some(s) = p.scopes.iter().find(|s| !scope_ok(s)) { bail!("{path}: provider `{name}`: scope `{s}`"); }
            let value = |k: &str| env(k).map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
            let (Some(client_id), Some(client_secret)) = (value(&p.client_id_env), value(&p.client_secret_env)) else {
                tracing::warn!(provider = %name, "the deployment's OAuth provider is off, and its connectors with it: {} and {} are not both set", p.client_id_env, p.client_secret_env);
                continue;
            };
            let label = p.label.filter(|l| !l.trim().is_empty()).unwrap_or_else(|| name.clone());
            providers.insert(name.clone(), Provider { name, label, authorize_url: p.authorize_url, token_url: p.token_url, revoke_url: p.revoke_url, client_id, client_secret, scopes: p.scopes, params: p.params });
        }
        let mut connectors = BTreeMap::new();
        for (name, c) in file.connectors {
            if name.is_empty() || name.len() > 64 || !name.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-') { bail!("{path}: connector `{name}`: letters, digits, `_` and `-`"); }
            if let Some(problem) = url_problem(&c.url) { bail!("{path}: connector `{name}`: url: {problem}"); }
            if let Some(s) = c.scopes.iter().find(|s| !scope_ok(s)) { bail!("{path}: connector `{name}`: scope `{s}`"); }
            match &c.oauth {
                Some(p) if !declared.contains(p) => bail!("{path}: connector `{name}` calls with `{p}`, which the file does not declare under `oauth`"),
                Some(p) if !providers.contains_key(p) => { tracing::warn!(connector = %name, provider = %p, "the deployment's connector is off: its provider is"); continue; }
                None if !c.scopes.is_empty() => bail!("{path}: connector `{name}` has scopes and no `oauth` provider"),
                _ => {}
            }
            connectors.insert(name.clone(), Connector { name, description: c.description.trim().to_string(), url: c.url, provider: c.oauth, scopes: c.scopes, roles: c.roles });
        }
        Ok(Self { providers, connectors, state_key, tools: Default::default(), tokens: Default::default() })
    }

    /// Whether `name` is one of the deployment's connectors: no realm declares, refreshes or removes one of that name.
    pub fn reserves(&self, name: &str) -> bool { self.connectors.contains_key(name) }

    /// The deployment's connectors as the registry lists them (`GET /v1/connectors`), each with its declaration (`config`).
    pub fn rows(&self) -> Vec<Value> {
        let tools = self.tools.read().unwrap_or_else(|e| e.into_inner());
        self.connectors.values().map(|c| {
            let (t, at) = tools.get(&c.name).cloned().unwrap_or_default();
            let oauth = c.provider.as_ref().and_then(|p| self.providers.get(p)).map(|p| json!({ "provider": p.name, "label": p.label, "scopes": c.scopes }));
            json!({ "name": c.name, "kind": "mcp", "description": c.description, "roles": c.roles, "tools": t, "tools_hash": crate::api::tools_hash(&t),
                "captured_at": (!at.is_empty()).then_some(at), "declared_by": "", "declared_at": "", "provided": "deployment",
                "transport": { "url": c.url, "command": null, "base_url": null, "task_queue": null }, "audience": null, "secret": null, "worker": null,
                "oauth": oauth, "config": c.config() })
        }).collect()
    }

    /// The connectors that call with `provider`.
    fn of(&self, provider: &str) -> impl Iterator<Item = &Connector> { self.connectors.values().filter(move |c| c.provider.as_deref() == Some(provider)) }

    /// What a connection to `p` is asked for: the provider's own scopes, and those of its connectors (`names`, else all).
    fn scopes_for(&self, p: &Provider, names: Option<&[String]>) -> Result<Vec<String>, String> {
        let mut out: Vec<String> = p.scopes.clone();
        let mut add = |s: &String| if !out.contains(s) { out.push(s.clone()); };
        match names {
            None => self.of(&p.name).flat_map(|c| &c.scopes).for_each(&mut add),
            Some(names) => for n in names {
                let c = self.connectors.get(n).filter(|c| c.provider.as_deref() == Some(p.name.as_str())).ok_or_else(|| format!("`{n}` is not a connector that calls with {}", p.label))?;
                c.scopes.iter().for_each(&mut add);
            },
        }
        Ok(out)
    }

    fn cached(&self, key: &str) -> Option<String> {
        let now = chrono::Utc::now().timestamp();
        let mut m = self.tokens.lock().unwrap_or_else(|e| e.into_inner());
        m.retain(|_, (_, exp)| *exp - 60 > now);
        m.get(key).map(|(t, _)| t.clone())
    }
    fn keep(&self, key: String, token: &str, expires_in: i64) {
        self.tokens.lock().unwrap_or_else(|e| e.into_inner()).insert(key, (token.to_string(), chrono::Utc::now().timestamp() + expires_in.clamp(60, 24 * 3600)));
    }
    fn forget(&self, key: &str) { self.tokens.lock().unwrap_or_else(|e| e.into_inner()).remove(key); }
}

fn provider_name(n: &str) -> bool { (1..=32).contains(&n.len()) && n != "callback" && n.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-') }
fn scope_ok(s: &str) -> bool { !s.is_empty() && s.len() <= 300 && !s.chars().any(|c| c.is_whitespace() || c.is_control()) }

/// Why `u` cannot be one of the file's addresses: https, or http on this machine (a test's stand-in).
fn url_problem(u: &str) -> Option<String> {
    let Ok(url) = reqwest::Url::parse(u) else { return Some(format!("`{u}` is not a URL")) };
    let local = matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"));
    match url.scheme() { "https" => None, "http" if local => None, _ => Some(format!("`{u}`: https (http on this machine only)")) }
}

/// Reads the tools of the deployment's connectors (`tools/list`, without a token) now and every few hours.
pub async fn capture_tools(st: Arc<AppState>) {
    if st.deployment.connectors.is_empty() { return; }
    loop {
        let mut again = TOOLS_EVERY;
        for c in st.deployment.connectors.values() {
            match cori_engine::mcp::list_tools(&c.resolved(), std::path::Path::new("."), None, cori_engine::egress::Egress::Any).await {
                Ok(t) => {
                    tracing::info!(connector = %c.name, tools = t.len(), "the deployment's connector listed its tools");
                    st.deployment.tools.write().unwrap_or_else(|e| e.into_inner()).insert(c.name.clone(), (t, chrono::Utc::now().to_rfc3339()));
                }
                Err(e) => { again = std::time::Duration::from_secs(60); tracing::warn!(connector = %c.name, error = %e, "the deployment's connector did not list its tools: asked again in a minute"); }
            }
        }
        tokio::time::sleep(again).await;
    }
}

// ---- connections ----

/// Whose a connection is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Owner { Person(String), App(String) }

impl Owner {
    /// The `owner` column.
    pub fn key(&self) -> String { match self { Owner::Person(s) => format!("person:{s}"), Owner::App(a) => format!("app:{a}") } }
    fn parse(k: &str) -> Option<Self> {
        k.strip_prefix("person:").map(|s| Owner::Person(s.into())).or_else(|| k.strip_prefix("app:").map(|a| Owner::App(a.into())))
    }
    /// Where its connections are made, in the console.
    fn page(&self) -> String { match self { Owner::Person(_) => "#/connections".into(), Owner::App(a) => format!("#/app/{}/settings", urlencoding::encode(a)) } }
    fn whose(&self, p: &Provider) -> String { match self { Owner::Person(_) => format!("your {} account", p.label), Owner::App(a) => format!("app `{a}`'s {} account", p.label) } }
}

/// The console's address of a page (`#/…`).
fn console(st: &AppState, page: &str) -> String { format!("{}/{page}", st.cfg.public_url.trim_end_matches('/')) }
/// Where providers send people back.
pub fn callback_url(st: &AppState) -> String { format!("{}/v1/connections/callback", st.cfg.public_url.trim_end_matches('/')) }

/// A connection as its row holds it.
struct Stored { account: Option<String>, scopes: Vec<String>, sealed: String, created_at: String, updated_at: String, used_at: Option<String> }

async fn stored(st: &AppState, owner: &Owner, provider: &str) -> Result<Option<Stored>, ApiError> {
    let r = sqlx::query("SELECT account, scopes, refresh_sealed, created_at, updated_at, used_at FROM connections WHERE owner = $1 AND provider = $2").bind(owner.key()).bind(provider).fetch_optional(&st.db).await?;
    Ok(r.map(|r| Stored { account: r.get("account"), scopes: serde_json::from_str(&r.get::<String, _>("scopes")).unwrap_or_default(), sealed: r.get("refresh_sealed"), created_at: r.get("created_at"), updated_at: r.get("updated_at"), used_at: r.get("used_at") }))
}

fn aad(st: &AppState, owner: &Owner, provider: &str) -> String { crate::names::connection_aad(st.realm(), &owner.key(), provider) }

fn missing<'a>(needs: &'a [String], granted: &[String]) -> Vec<&'a String> { needs.iter().filter(|s| !granted.contains(s)).collect() }

/// What a person who builds or runs apps sees of the providers: each one's connectors and scopes, and `owner`'s connection.
async fn overview(st: &AppState, owner: &Owner) -> Result<Vec<Value>, ApiError> {
    let mut out = Vec::new();
    for p in st.deployment.providers.values() {
        let connectors: Vec<&Connector> = st.deployment.of(&p.name).collect();
        let mut needs: Vec<String> = Vec::new();
        for s in connectors.iter().flat_map(|c| &c.scopes) { if !needs.contains(s) { needs.push(s.clone()); } }
        let c = stored(st, owner, &p.name).await?;
        out.push(json!({ "provider": p.name, "label": p.label, "connectors": connectors.iter().map(|c| &c.name).collect::<Vec<_>>(), "scopes": needs,
            "connection": c.map(|c| json!({ "account": c.account, "scopes": c.scopes, "missing": missing(&needs, &c.scopes), "connected_at": c.updated_at, "since": c.created_at, "used_at": c.used_at })) }));
    }
    Ok(out)
}

/// A person signed in, never an agent session nor a machine: their own accounts.
fn require_person(who: &Principal, what: &str) -> Result<(), ApiError> {
    crate::api::require_human(who, what)?;
    if who.kind != PrincipalKind::Human || who.sub.is_empty() || who.sub == "anonymous" { return Err(ApiError(StatusCode::FORBIDDEN, format!("{what}: a person's own, signed in"))); }
    Ok(())
}

/// `GET /v1/connections`: the providers, and the caller's connection to each.
pub(crate) async fn list_mine(InRealm(st): InRealm, Caller(who): Caller) -> Res {
    require_person(&who, "your connections")?;
    Ok(Json(json!({ "providers": overview(&st, &Owner::Person(who.sub.clone())).await? })).into_response())
}

/// `POST /v1/connections/{provider}` `{ connectors?, return_to? }`: the provider's consent page for the caller's own
/// account (`url`), for the scopes of those connectors (all of the provider's by default).
pub(crate) async fn start_mine(InRealm(st): InRealm, Caller(who): Caller, Path(provider): Path<String>, body: Option<Json<Value>>) -> Res {
    require_person(&who, "connecting an account")?;
    begin(&st, &who, Owner::Person(who.sub.clone()), &provider, body.map(|b| b.0).unwrap_or_default()).await
}

/// `DELETE /v1/connections/{provider}`: the caller's connection goes, and the provider is told.
pub(crate) async fn delete_mine(InRealm(st): InRealm, Caller(who): Caller, Path(provider): Path<String>) -> Res {
    require_person(&who, "disconnecting an account")?;
    disconnect(&st, &who, &Owner::Person(who.sub.clone()), &provider).await
}

/// `GET /v1/apps/{app}/connections`: the app's own connections, for its owner, its builders and the admins.
pub(crate) async fn list_app(InRealm(st): InRealm, Caller(who): Caller, Path(app): Path<String>) -> Res {
    let rec = crate::api::require_app(&st, &app).await?;
    if !(who.is_admin() || rec.owner.as_deref() == Some(who.sub.as_str()) || rec.builders.iter().any(|b| b == &who.sub)) {
        return Err(ApiError(StatusCode::FORBIDDEN, format!("`{app}`'s connections are for its owner, its builders and the admins")));
    }
    Ok(Json(json!({ "app": app, "providers": overview(&st, &Owner::App(app.clone())).await? })).into_response())
}

/// `POST /v1/apps/{app}/connections/{provider}`: the consent page for the account the app's runs use (`account = "app"`),
/// for its owner or an admin.
pub(crate) async fn start_app(InRealm(st): InRealm, Caller(who): Caller, Path((app, provider)): Path<(String, String)>, body: Option<Json<Value>>) -> Res {
    require_person(&who, "connecting an app's account")?;
    let rec = crate::api::require_app(&st, &app).await?;
    crate::api::require_owner_or_admin(&who, &rec, "connecting the app's account")?;
    begin(&st, &who, Owner::App(app), &provider, body.map(|b| b.0).unwrap_or_default()).await
}

/// `DELETE /v1/apps/{app}/connections/{provider}`: for its owner or an admin.
pub(crate) async fn delete_app(InRealm(st): InRealm, Caller(who): Caller, Path((app, provider)): Path<(String, String)>) -> Res {
    require_person(&who, "disconnecting an app's account")?;
    let rec = crate::api::require_app(&st, &app).await?;
    crate::api::require_owner_or_admin(&who, &rec, "disconnecting the app's account")?;
    disconnect(&st, &who, &Owner::App(app), &provider).await
}

/// A consent in flight, sealed into the authorization request's `state` with the deployment's key.
#[derive(Debug, Serialize, Deserialize)]
struct Pending { id: String, realm: Option<String>, owner: String, provider: String, scopes: Vec<String>, verifier: String, nonce: String, back: String, by: String, exp: i64 }

fn seal(st: &AppState, p: &Pending) -> Result<String, ApiError> {
    crate::secrets::seal_native(&st.deployment.state_key, STATE_AAD, &serde_json::to_string(p).map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?, rand::random())
        .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}
fn open(st: &AppState, sealed: &str) -> Option<Pending> {
    serde_json::from_str(&crate::secrets::open_native(&st.deployment.state_key, STATE_AAD, sealed).ok()?).ok()
}

/// The browser that starts a consent is the one that finishes it: a cookie of its own, which the callback reads.
fn cookie_name(id: &str) -> String { format!("cori_connect_{id}") }
fn cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    headers.get_all(header::COOKIE).iter().filter_map(|v| v.to_str().ok()).flat_map(|v| v.split(';')).map(str::trim)
        .find_map(|c| c.strip_prefix(name).and_then(|r| r.strip_prefix('='))).map(String::from)
}

fn b64url(b: &[u8]) -> String { base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b) }
fn sha(s: &str) -> String { use sha2::Digest; hex::encode(sha2::Sha256::digest(s.as_bytes())) }

/// A console page to come back to: `#/…`, short, with no query of its own.
fn console_route(r: &str) -> Option<String> {
    (r.starts_with("#/") && r.len() <= 200 && !r.contains('?') && r.chars().all(|c| c.is_ascii_graphic())).then(|| r.to_string())
}

async fn begin(st: &AppState, who: &Principal, owner: Owner, provider: &str, body: Value) -> Res {
    let p = st.deployment.providers.get(provider).ok_or_else(|| ApiError(StatusCode::NOT_FOUND, match st.deployment.providers.keys().map(|k| format!("`{k}`")).collect::<Vec<_>>() {
        none if none.is_empty() => format!("no provider `{provider}`: this server connects to none"),
        some => format!("no provider `{provider}` here ({})", some.join(", ")),
    }))?;
    let names: Option<Vec<String>> = body.get("connectors").and_then(|c| c.as_array()).map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect());
    let scopes = st.deployment.scopes_for(p, names.as_deref()).map_err(|e| ApiError(StatusCode::BAD_REQUEST, e))?;
    let back = match body.get("return_to").and_then(|r| r.as_str()) {
        Some(r) => console_route(r).ok_or_else(|| ApiError(StatusCode::BAD_REQUEST, "return_to is a console page (`#/…`, without a query)".into()))?,
        None => owner.page(),
    };
    let (id, nonce, verifier) = (hex::encode(rand::random::<[u8; 6]>()), hex::encode(rand::random::<[u8; 32]>()), b64url(&rand::random::<[u8; 32]>()));
    let challenge = { use sha2::Digest; b64url(&sha2::Sha256::digest(verifier.as_bytes())) };
    let pending = Pending { id: id.clone(), realm: st.realm().map(|r| r.as_str().to_string()), owner: owner.key(), provider: p.name.clone(), scopes: scopes.clone(), verifier, nonce: sha(&nonce), back, by: who.sub.clone(), exp: chrono::Utc::now().timestamp() + CONSENT_SECS };
    let mut url = reqwest::Url::parse(&p.authorize_url).map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, format!("{}: {e}", p.authorize_url)))?;
    {
        let mut q = url.query_pairs_mut();
        q.append_pair("response_type", "code").append_pair("client_id", &p.client_id).append_pair("redirect_uri", &callback_url(st))
            .append_pair("scope", &scopes.join(" ")).append_pair("state", &seal(st, &pending)?).append_pair("code_challenge", &challenge).append_pair("code_challenge_method", "S256");
        for (k, v) in &p.params {
            // the account an app's people share is chosen, not the one the owner is signed in with
            let v = if k == "prompt" && matches!(owner, Owner::App(_)) && !v.split(' ').any(|x| x == "select_account") { format!("select_account {v}") } else { v.clone() };
            q.append_pair(k, &v);
        }
        if let (Owner::Person(_), Some(email), false) = (&owner, &who.email, p.params.contains_key("login_hint")) { q.append_pair("login_hint", email); }
    }
    let secure = if st.cfg.public_url.starts_with("https://") { "; Secure" } else { "" };
    let set = format!("{}={nonce}; Path=/v1/connections/callback; Max-Age={CONSENT_SECS}; HttpOnly; SameSite=Lax{secure}", cookie_name(&id));
    Ok(([(header::SET_COOKIE, set)], Json(json!({ "url": url.to_string(), "provider": p.name, "scopes": scopes }))).into_response())
}

/// `GET /v1/connections/callback`: the provider sends the person back here with a code, which becomes the connection;
/// then back to the console page the consent started from, saying how it went (`?connected=<provider>`, `?error=…`).
pub(crate) async fn callback(InRealm(st): InRealm, headers: HeaderMap, Query(q): Query<BTreeMap<String, String>>) -> Response {
    let back = |page: &str, id: Option<&str>, (k, v): (&str, &str)| -> Response {
        let mut r = (StatusCode::SEE_OTHER, [(header::LOCATION, format!("{}?{k}={}", console(&st, page), urlencoding::encode(v)))]).into_response();
        if let Some(id) = id && let Ok(c) = format!("{}=; Path=/v1/connections/callback; Max-Age=0; HttpOnly; SameSite=Lax", cookie_name(id)).parse() { r.headers_mut().insert(header::SET_COOKIE, c); }
        r
    };
    let Some(p) = q.get("state").and_then(|s| open(&st, s)) else { return back("#/connections", None, ("error", "this answer is not one of this server's requests: connect again")) };
    let fail = |msg: &str| back(&p.back, Some(&p.id), ("error", msg));
    if p.exp < chrono::Utc::now().timestamp() { return fail("the consent took too long: connect again"); }
    if cookie(&headers, &cookie_name(&p.id)).map(|c| sha(&c)) != Some(p.nonce.clone()) { return fail("this consent was started in another browser: connect again from this one"); }
    if let Some(e) = q.get("error") {
        return fail(&if e == "access_denied" { "the access was not allowed".to_string() } else { q.get("error_description").cloned().unwrap_or_else(|| e.clone()) });
    }
    let Some(code) = q.get("code").filter(|c| !c.is_empty()) else { return fail("the provider sent no code: connect again") };
    // the realm the consent started in (the core's single one: this)
    let st = match p.realm.as_deref().map(crate::db::Realm::parse) {
        None => st.clone(),
        Some(None) => return fail("this answer names no organization: connect again"),
        Some(Some(r)) => match st.in_realm(r).await { Ok(s) => s, Err(e) => return fail(&e.1) },
    };
    let (Some(owner), Some(provider)) = (Owner::parse(&p.owner), st.deployment.providers.get(&p.provider)) else { return fail("that provider is off on this server") };
    match connect(&st, provider, &owner, &p, code).await {
        Ok(()) => back(&p.back, Some(&p.id), ("connected", &p.provider)),
        Err(e) => { tracing::warn!(provider = %p.provider, owner = %p.owner, error = %e, "a connection did not complete"); fail(&e) }
    }
}

/// What a provider's token endpoint answered: a refusal of the grant itself (`invalid_grant`: revoked, expired, a code used
/// twice), or anything else that went wrong (the provider down, the deployment's client refused).
enum ProviderError { Grant(String), Other(String) }

impl std::fmt::Display for ProviderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { match self { ProviderError::Grant(e) | ProviderError::Other(e) => f.write_str(e) } }
}

impl Provider {
    async fn token(&self, mut form: Vec<(&'static str, String)>) -> Result<Value, ProviderError> {
        form.push(("client_id", self.client_id.clone()));
        form.push(("client_secret", self.client_secret.clone()));
        let res = cori_engine::http::client().post(&self.token_url).header(header::ACCEPT, "application/json").form(&form).send().await
            .map_err(|e| ProviderError::Other(format!("{} did not answer: {e}", self.label)))?;
        let status = res.status();
        let v: Value = res.json().await.unwrap_or(Value::Null);
        if status.is_success() && v["access_token"].is_string() { return Ok(v); }
        let (error, description) = (v["error"].as_str().unwrap_or(""), v["error_description"].as_str().unwrap_or(""));
        let said = if description.is_empty() { error.to_string() } else { description.to_string() };
        if error == "invalid_grant" { return Err(ProviderError::Grant(said)); }
        Err(ProviderError::Other(format!("{} answered {status}{}", self.label, if said.is_empty() { String::new() } else { format!(": {said}") })))
    }

    /// The provider forgets a grant (RFC 7009), as far as it answers: the connection goes here whatever it says.
    async fn revoke(&self, refresh: &str) {
        let Some(url) = &self.revoke_url else { return };
        let form = [("token", refresh), ("token_type_hint", "refresh_token"), ("client_id", self.client_id.as_str()), ("client_secret", self.client_secret.as_str())];
        match cori_engine::http::client().post(url).form(&form).send().await {
            Ok(r) if r.status().is_success() => {}
            Ok(r) => tracing::warn!(provider = %self.name, status = %r.status(), "the provider did not revoke a grant"),
            Err(e) => tracing::warn!(provider = %self.name, error = %e, "the provider did not revoke a grant"),
        }
    }
}

/// The address an OpenID provider's `id_token` names (it came from the token endpoint itself, over TLS).
fn id_token_email(t: &str) -> Option<String> {
    let payload = t.split('.').nth(1)?;
    let v: Value = serde_json::from_slice(&base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).ok()?).ok()?;
    v["email"].as_str().map(String::from)
}

/// The code becomes the connection: its refresh token sealed, its scopes and account kept, its access token cached.
async fn connect(st: &AppState, p: &Provider, owner: &Owner, pending: &Pending, code: &str) -> Result<(), String> {
    let v = p.token(vec![("grant_type", "authorization_code".into()), ("code", code.to_string()), ("redirect_uri", callback_url(st)), ("code_verifier", pending.verifier.clone())]).await.map_err(|e| e.to_string())?;
    let access = v["access_token"].as_str().unwrap_or_default();
    let granted: Vec<String> = v["scope"].as_str().map(|s| s.split_whitespace().map(String::from).collect()).unwrap_or_else(|| pending.scopes.clone());
    let account = v["id_token"].as_str().and_then(id_token_email);
    let old = stored(st, owner, &p.name).await.map_err(|e| e.1)?;
    // a provider that already had the person's consent may give no new refresh token: the one kept goes on
    let sealed = match (v["refresh_token"].as_str(), &old) {
        (Some(r), _) => st.broker.seal_value(r, &aad(st, owner, &p.name)).map_err(|e| e.to_string())?,
        (None, Some(o)) => o.sealed.clone(),
        (None, None) => return Err(format!("{} gave no refresh token: remove the access it lists for this app in your account's settings, then connect again", p.label)),
    };
    let now = chrono::Utc::now().to_rfc3339();
    sqlx::query("INSERT INTO connections (owner, provider, account, scopes, refresh_sealed, created_by, created_at, updated_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $7) ON CONFLICT ON CONSTRAINT connections_pkey DO UPDATE SET account = $3, scopes = $4, refresh_sealed = $5, created_by = $6, updated_at = $7")
        .bind(owner.key()).bind(&p.name).bind(&account).bind(serde_json::to_string(&granted).unwrap_or_default()).bind(&sealed).bind(&pending.by).bind(&now).execute(&st.db).await.map_err(|e| format!("database: {e}"))?;
    if !access.is_empty() { st.deployment.keep(aad(st, owner, &p.name), access, v["expires_in"].as_i64().unwrap_or(3600)); }
    crate::db::audit(&st.db, &pending.by, "connection.connect", &format!("{}/{}", owner.key(), p.name), json!({ "provider": p.name, "account": account, "scopes": granted })).await.map_err(|e| format!("{e:#}"))?;
    Ok(())
}

async fn disconnect(st: &AppState, who: &Principal, owner: &Owner, provider: &str) -> Res {
    let c = stored(st, owner, provider).await?.ok_or_else(|| ApiError(StatusCode::NOT_FOUND, format!("no connection to `{provider}` to remove")))?;
    if let (Some(p), Ok(refresh)) = (st.deployment.providers.get(provider), st.broker.open_value(&c.sealed, &aad(st, owner, provider))) { p.revoke(&refresh).await; }
    sqlx::query("DELETE FROM connections WHERE owner = $1 AND provider = $2").bind(owner.key()).bind(provider).execute(&st.db).await?;
    st.deployment.forget(&aad(st, owner, provider));
    crate::db::audit(&st.db, &who.sub, "connection.disconnect", &format!("{}/{provider}", owner.key()), json!({ "provider": provider, "account": c.account })).await?;
    Ok(Json(json!({ "disconnected": provider, "account": c.account })).into_response())
}

/// Every connection of `owner` goes (a person left the organization, an app was deleted), and the providers are told, in
/// the background.
pub async fn forget(st: &AppState, owner: &Owner) -> anyhow::Result<u64> {
    let rows = sqlx::query("SELECT provider, refresh_sealed FROM connections WHERE owner = $1").bind(owner.key()).fetch_all(&st.db).await?;
    let n = sqlx::query("DELETE FROM connections WHERE owner = $1").bind(owner.key()).execute(&st.db).await?.rows_affected();
    for r in rows {
        let provider: String = r.get("provider");
        st.deployment.forget(&aad(st, owner, &provider));
        if let (Some(p), Ok(refresh)) = (st.deployment.providers.get(&provider).cloned(), st.broker.open_value(&r.get::<String, _>("refresh_sealed"), &aad(st, owner, &provider))) {
            tokio::spawn(async move { p.revoke(&refresh).await });
        }
    }
    Ok(n)
}

// ---- runs ----

/// Why a run gets no access token: refused (with what to do), or the provider did not answer (retried).
enum TokenError { Refused(&'static str, String), Unavailable(String) }

/// `owner`'s access token for `p`, with the scopes `needs`: the cached one, else refreshed (one refresh at a time per
/// connection, every replica included: a provider that rotates refresh tokens hands each out once). A grant the provider
/// no longer knows (revoked, expired) ends the connection.
async fn access_token(st: &AppState, owner: &Owner, p: &Provider, needs: &[String]) -> Result<String, TokenError> {
    let db = |e: sqlx::Error| TokenError::Unavailable(format!("database: {e}"));
    let again = format!("connect it again ({})", console(st, &owner.page()));
    let Some(c) = stored(st, owner, &p.name).await.map_err(|e| TokenError::Unavailable(e.1))? else {
        return Err(TokenError::Refused("no_connection", match owner {
            Owner::Person(_) => format!("this calls {} as you: connect your account first ({})", p.label, console(st, &owner.page())),
            Owner::App(a) => format!("this calls {} with app `{a}`'s account, which its owner has not connected yet ({})", p.label, console(st, &owner.page())),
        }));
    };
    let lacks = missing(needs, &c.scopes);
    if !lacks.is_empty() {
        return Err(TokenError::Refused("missing_scopes", format!("{} does not allow {}: {again}, and allow it", owner.whose(p), lacks.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", "))));
    }
    let key = aad(st, owner, &p.name);
    if let Some(t) = st.deployment.cached(&key) { return Ok(t); }
    let mut tx = st.db.begin().await.map_err(db)?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1))").bind(&key).execute(&mut *tx).await.map_err(db)?;
    if let Some(t) = st.deployment.cached(&key) { return Ok(t); }
    let Some(sealed) = sqlx::query_scalar::<_, String>("SELECT refresh_sealed FROM connections WHERE owner = $1 AND provider = $2").bind(owner.key()).bind(&p.name).fetch_optional(&mut *tx).await.map_err(db)? else {
        return Err(TokenError::Refused("no_connection", format!("{} was just disconnected: {again}", owner.whose(p))));
    };
    let refresh = st.broker.open_value(&sealed, &key).map_err(|e| TokenError::Unavailable(format!("the connection does not open here: {e}")))?;
    match p.token(vec![("grant_type", "refresh_token".into()), ("refresh_token", refresh.clone())]).await {
        Ok(v) => {
            let access = v["access_token"].as_str().unwrap_or_default().to_string();
            if let Some(r) = v["refresh_token"].as_str().filter(|r| *r != refresh) {
                let resealed = st.broker.seal_value(r, &key).map_err(|e| TokenError::Unavailable(e.to_string()))?;
                sqlx::query("UPDATE connections SET refresh_sealed = $1, updated_at = $2 WHERE owner = $3 AND provider = $4").bind(resealed).bind(chrono::Utc::now().to_rfc3339()).bind(owner.key()).bind(&p.name).execute(&mut *tx).await.map_err(db)?;
            }
            tx.commit().await.map_err(db)?;
            st.deployment.keep(key, &access, v["expires_in"].as_i64().unwrap_or(3600));
            Ok(access)
        }
        Err(ProviderError::Grant(said)) => {
            sqlx::query("DELETE FROM connections WHERE owner = $1 AND provider = $2").bind(owner.key()).bind(&p.name).execute(&mut *tx).await.map_err(db)?;
            tx.commit().await.map_err(db)?;
            crate::db::audit(&st.db, "cori", "connection.expired", &format!("{}/{}", owner.key(), p.name), json!({ "provider": p.name, "said": said })).await.ok();
            Err(TokenError::Refused("connection_expired", format!("{} ended its access (revoked, or expired: {said}): {again}", p.label)))
        }
        Err(ProviderError::Other(e)) => Err(TokenError::Unavailable(e)),
    }
}

async fn record(st: &AppState, provider: &str, run: &str, connector: &str, worker: &crate::workers::Worker, outcome: &str) {
    sqlx::query("INSERT INTO secret_use (id, name, run_id, connector, worker, at, outcome) VALUES ($1, $2, $3, $4, $5, $6, $7)")
        .bind(uuid::Uuid::new_v4().to_string()).bind(format!("connection:{provider}")).bind(run).bind(connector).bind(worker.label()).bind(chrono::Utc::now().to_rfc3339()).bind(outcome).execute(&st.db).await.ok();
}

/// `POST /v1/leases` with `connection`: the access token a deployment's connector calls with, for a running run of an app
/// that uses it: its person's connection, or the app's (`account = "app"`). Refused with what to do otherwise.
pub(crate) async fn lease(st: &Arc<AppState>, worker: &crate::workers::Worker, body: &Value, provider: &str) -> Res {
    let (run, connector) = (body["run_id"].as_str().unwrap_or(""), body["connector"].as_str().unwrap_or(""));
    let refuse = |outcome: &'static str, msg: String| async move {
        record(st, provider, run, connector, worker, outcome).await;
        Err::<Response, ApiError>(ApiError(StatusCode::FORBIDDEN, msg))
    };
    let Some(r) = crate::workers::run_ref(st, run).await? else { return refuse("unknown_run", "lease refused: unknown run".into()).await };
    if r.status != "running" { return refuse("run_not_running", "lease refused: the run is not running".into()).await; }
    let connectors = crate::workers::app_connectors(st, &r.app, &r.version).await?;
    let Some(c) = connectors.get(connector) else { return refuse("connector_not_in_app", format!("lease refused: `{connector}` is not one of `{}`'s connectors", r.app)).await };
    if !crate::workers::serves_connector(worker, c) { return refuse("worker_does_not_serve_connector", "lease refused: this worker does not serve the connector".into()).await; }
    // the deployment's connector, as the deployment declares it: nothing else calls with a connection
    let Some(d) = st.deployment.connectors.get(connector).filter(|d| d.provider.as_deref() == Some(provider)) else {
        return refuse("connection_not_declared", format!("lease refused: `{connector}` does not call with `{provider}`")).await;
    };
    let Some(p) = st.deployment.providers.get(provider) else { return refuse("provider_off", format!("{provider} is off on this server")).await };
    let user: Value = sqlx::query_scalar::<_, Option<String>>("SELECT user_json FROM runs WHERE id = $1").bind(run).fetch_one(&st.db).await?
        .and_then(|u| serde_json::from_str(&u).ok()).unwrap_or(Value::Null);
    let owner = if c.account.as_deref() == Some("app") {
        Owner::App(r.app.clone())
    } else if user["service_account"].as_bool() == Some(true) {
        return refuse("no_person", format!("`{connector}` calls {} as the run's person, and this run has none (a schedule's): set `account = \"app\"` under [connectors.{connector}] in cori.toml, then connect the app's account ({})", p.label, console(st, &Owner::App(r.app.clone()).page()))).await;
    } else if user["via"].as_str() == Some("agent") {
        return refuse("agent_session", format!("a run an agent session started never acts as its person, and `{connector}` would call {} as them: run it yourself (the release's page), or try it with `cori run` and a fixture", p.label)).await;
    } else {
        match user["sub"].as_str().filter(|s| !s.is_empty() && *s != "anonymous") {
            Some(s) => Owner::Person(s.to_string()),
            None => return refuse("no_person", format!("`{connector}` calls {} as the run's person, and this run has none", p.label)).await,
        }
    };
    match access_token(st, &owner, p, &d.scopes).await {
        Ok(token) => {
            record(st, provider, run, connector, worker, "granted").await;
            sqlx::query("UPDATE connections SET used_at = $1 WHERE owner = $2 AND provider = $3").bind(chrono::Utc::now().to_rfc3339()).bind(owner.key()).bind(provider).execute(&st.db).await.ok();
            Ok(Json(json!({ "value": token, "ttl_secs": 300 })).into_response())
        }
        Err(TokenError::Refused(outcome, msg)) => refuse(outcome, msg).await,
        Err(TokenError::Unavailable(e)) => {
            record(st, provider, run, connector, worker, "provider_unavailable").await;
            Err(ApiError(StatusCode::BAD_GATEWAY, e))
        }
    }
}

/// Before a run starts: the connections the workflow's connectors call with, `who`'s own or the app's. A missing one
/// answers 409, naming the provider and the console page that connects it, rather than a run stopped halfway.
pub(crate) async fn before_run(st: &AppState, who: &Principal, app: &str, cori_toml: &str, entry: &crate::bundles::WorkflowEntry) -> Result<Option<Response>, ApiError> {
    if st.deployment.connectors.is_empty() { return Ok(None); }
    let cfg = cori_engine::config::AppConfig::from_toml(cori_toml, std::path::Path::new(".")).ok();
    for name in &entry.compiled.manifest.connectors {
        let Some(c) = st.deployment.connectors.get(name) else { continue };
        let Some(p) = c.provider.as_ref().and_then(|p| st.deployment.providers.get(p)) else { continue };
        let app_account = cfg.as_ref().and_then(|cfg| cfg.connectors.get(name)).and_then(|x| x.account.as_deref()) == Some("app");
        let owner = if app_account { Owner::App(app.to_string()) } else {
            if who.is_agent() {
                return Ok(Some(conflict(format!("`{}` calls {} as the person who runs it ({name}), and a run an agent session starts never acts as its person: run it yourself (the release's page), or try it with `cori run` and a fixture", entry.id, p.label), p, None)));
            }
            Owner::Person(who.sub.clone())
        };
        // the page that connects it, in the words too: an app's own page may show nothing else of the answer
        let page = console(st, &owner.page());
        let Some(conn) = stored(st, &owner, &p.name).await? else {
            let msg = match &owner {
                Owner::Person(_) => format!("`{}` calls {} as you ({name}): connect your account first ({page})", entry.id, p.label),
                Owner::App(_) => format!("`{}` calls {} with the app's account ({name}), which its owner has not connected yet ({page})", entry.id, p.label),
            };
            return Ok(Some(conflict(msg, p, Some(page))));
        };
        let lacks = missing(&c.scopes, &conn.scopes);
        if !lacks.is_empty() {
            return Ok(Some(conflict(format!("`{}` needs {} access that {} does not allow ({}): connect it again ({page})", entry.id, p.label, owner.whose(p), lacks.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")), p, Some(page))));
        }
    }
    Ok(None)
}

fn conflict(msg: String, p: &Provider, url: Option<String>) -> Response {
    (StatusCode::CONFLICT, Json(json!({ "error": msg, "connect": { "provider": p.name, "label": p.label, "url": url } }))).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOGLE: &str = r#"
oauth:
  google:
    label: Google
    authorize_url: https://accounts.google.com/o/oauth2/v2/auth
    token_url: https://oauth2.googleapis.com/token
    revoke_url: https://oauth2.googleapis.com/revoke
    client_id_env: CORI_OAUTH_GOOGLE_CLIENT_ID
    client_secret_env: CORI_OAUTH_GOOGLE_CLIENT_SECRET
    scopes: [openid, email]
    params: { access_type: offline, prompt: consent }
connectors:
  gmail:
    description: Gmail
    url: https://gmailmcp.googleapis.com/mcp/v1
    oauth: google
    scopes: [https://www.googleapis.com/auth/gmail.readonly, https://www.googleapis.com/auth/gmail.compose]
  google-drive:
    url: https://drivemcp.googleapis.com/mcp/v1
    oauth: google
    scopes: [https://www.googleapis.com/auth/drive.readonly, https://www.googleapis.com/auth/drive.file]
  docs-search:
    url: https://docs.example/mcp
"#;

    fn env(set: bool) -> impl Fn(&str) -> Option<String> { move |k: &str| set.then(|| format!("{k}-value")) }

    #[test]
    fn the_file_declares_providers_and_connectors() {
        let d = Deployment::parse("f", GOOGLE, env(true), [0; 32]).unwrap();
        assert_eq!(d.connectors.keys().map(String::as_str).collect::<Vec<_>>(), ["docs-search", "gmail", "google-drive"]);
        let g = &d.providers["google"];
        assert_eq!((g.label.as_str(), g.client_id.as_str()), ("Google", "CORI_OAUTH_GOOGLE_CLIENT_ID-value"));
        assert!(!format!("{g:?}").contains("SECRET-value"), "the client's secret never in a log");
        let cfg = d.connectors["gmail"].config();
        assert_eq!((cfg.kind.as_str(), cfg.url.as_deref()), ("mcp", Some("https://gmailmcp.googleapis.com/mcp/v1")));
        assert_eq!(cfg.oauth.as_ref().map(|o| o.scopes.len()), Some(2));
        assert!(d.connectors["docs-search"].config().oauth.is_none(), "a connector without a provider calls with nothing");
        // a connection asks for the provider's own scopes and its connectors', each once
        let all = d.scopes_for(g, None).unwrap();
        assert_eq!(all.len(), 6);
        assert_eq!(d.scopes_for(g, Some(&["gmail".into()])).unwrap(), ["openid", "email", "https://www.googleapis.com/auth/gmail.readonly", "https://www.googleapis.com/auth/gmail.compose"]);
        assert!(d.scopes_for(g, Some(&["docs-search".into()])).is_err(), "a connector of no provider");
        let rows = d.rows();
        assert_eq!(rows[1]["oauth"]["label"], "Google");
        assert_eq!(rows[1]["provided"], "deployment");
        assert!(rows[1]["config"]["oauth"]["provider"] == "google");
    }

    #[test]
    fn the_google_workspace_file_reads() {
        let d = Deployment::parse("google-workspace.yaml", include_str!("../tests/fixtures/google-workspace.yaml"), env(true), [0; 32]).unwrap();
        assert_eq!(d.connectors.keys().map(String::as_str).collect::<Vec<_>>(), ["gmail", "google-docs", "google-drive", "google-sheets", "google-slides"]);
        assert!(d.connectors.values().all(|c| c.provider.as_deref() == Some("google") && c.url.starts_with("https://") && c.url.ends_with("mcp.googleapis.com/mcp/v1")));
        let g = &d.providers["google"];
        assert_eq!(g.params.get("access_type").map(String::as_str), Some("offline"), "a refresh token");
        assert_eq!(d.scopes_for(g, None).unwrap().len(), 2 + 10, "openid, email and the ten scopes of the five servers, each once");
    }

    #[test]
    fn a_provider_without_its_client_is_off_with_its_connectors() {
        let d = Deployment::parse("f", GOOGLE, env(false), [0; 32]).unwrap();
        assert!(d.providers.is_empty());
        assert_eq!(d.connectors.keys().map(String::as_str).collect::<Vec<_>>(), ["docs-search"]);
    }

    #[test]
    fn the_file_is_checked() {
        let bad = |from: &str, to: &str| Deployment::parse("f", &GOOGLE.replace(from, to), env(true), [0; 32]).err().map(|e| e.to_string()).unwrap_or_default();
        assert!(bad("CORI_OAUTH_GOOGLE_CLIENT_SECRET", "CORI_MASTER_KEY").contains("one of Cori's own settings"));
        assert!(bad("https://oauth2.googleapis.com/token", "http://oauth2.googleapis.com/token").contains("https"));
        assert!(bad("access_type: offline", "redirect_uri: https://evil.example").contains("Cori's to send"));
        assert!(bad("oauth: google\n    scopes: [https://www.googleapis.com/auth/drive", "oauth: microsoft\n    scopes: [https://www.googleapis.com/auth/drive").contains("does not declare"));
        assert!(bad("  google:\n    label", "  Google:\n    label").contains("provider `Google`"));
        assert!(bad("label: Google", "label: Google\n    secret: x").contains("unknown field"));
        assert!(Deployment::parse("f", &GOOGLE.replace("https://oauth2.googleapis.com/token", "http://127.0.0.1:9/token"), env(true), [0; 32]).is_ok(), "a stand-in on this machine");
    }

    #[test]
    fn a_consent_comes_back_to_the_console_only() {
        assert_eq!(console_route("#/connections").as_deref(), Some("#/connections"));
        assert_eq!(console_route("#/app/desk/settings").as_deref(), Some("#/app/desk/settings"));
        for bad in ["https://evil.example", "//evil.example", "#/x?connected=1", "#/a b", "/v1/apps"] { assert!(console_route(bad).is_none(), "{bad}"); }
        let mut h = HeaderMap::new();
        h.insert(header::COOKIE, "a=1; cori_connect_ab12=nonce; cori_connect_ab=other".parse().unwrap());
        assert_eq!(cookie(&h, "cori_connect_ab12").as_deref(), Some("nonce"));
        assert_eq!(cookie(&h, "cori_connect_ab").as_deref(), Some("other"));
        assert_eq!(Owner::parse("person:jean"), Some(Owner::Person("jean".into())));
        assert_eq!(Owner::parse(&Owner::App("desk".into()).key()), Some(Owner::App("desk".into())));
        assert_eq!(missing(&["a".to_string(), "b".to_string()], &["b".to_string()]), [&"a".to_string()]);
    }

    #[test]
    fn an_id_token_names_the_account() {
        let part = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(br#"{"email":"jean@example.com","sub":"1"}"#);
        assert_eq!(id_token_email(&format!("h.{part}.s")).as_deref(), Some("jean@example.com"));
        assert_eq!(id_token_email("opaque"), None);
    }
}
