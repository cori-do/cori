// SPDX-License-Identifier: Apache-2.0
//! Vault, a module of the core: each app's credentials under a key of its own in the organization's HashiCorp Vault.
//! The values stay in Cori's platform database, sealed by the broker as always (`cori_server::secrets`); what changes is
//! what wraps each secret's data key. Two engines, `CORI_VAULT_ENGINE`:
//!
//! - `transit`, the default: one Transit key per app, `<prefix>-app-<app>` (aes256-gcm96, not exportable), which wraps
//!   and unwraps the app's data keys in Vault. Vault holds the keys and never sees a value; Cori never holds a key.
//! - `kv`, for a Vault without Transit: one master key per app, 32 random bytes in Vault's KV engine (version 2) at
//!   `<mount>/<path>/apps/<app>` (`CORI_VAULT_KV_MOUNT`, `secret`; `CORI_VAULT_KV_PATH`, `cori`), which Cori reads with its
//!   Vault login and wraps the app's data keys with itself. Vault holds the keys and never sees a data key nor a value;
//!   Cori holds an app's key in memory while it uses it.
//!
//! Either way:
//! - Cori creates the app's key with the app when its policy allows (`CORI_VAULT_CREATE_KEYS`, default true); otherwise
//!   the organization's automation creates it, on the client hook's `app.created`, which names it. The deployment's own
//!   secrets keep the master key, unless `CORI_VAULT_DEPLOYMENT_KEY` names a key for them too.
//! - Writing a secret wraps its data key, the secret's scope and name as associated data; a lease unwraps it. What Cori
//!   unwrapped (Transit: a data key; KV: an app's key) is kept `CORI_VAULT_CACHE_SECS` at most (default 60): a key
//!   withdrawn from Cori in Vault stops every lease of its app's credentials within that time, store connections already
//!   open aside.
//! - The organization rotates keys in Vault (Transit: a new key version; KV: a new version of the secret). The leading
//!   replica re-wraps every data key under the newest version (`cori_server::secrets::Broker::rewrap`), after which the
//!   organization may retire the old ones. Rows record which engine wrapped them: after a switch of engine, the other's
//!   rows still open, and the same rewrap moves them to the engine now configured.
//! - Deleting an app deletes its secrets; its key stays for the organization to delete, unless `CORI_VAULT_DELETE_KEYS=true`.
//! - Login: Kubernetes auth with the server's service account (`CORI_VAULT_AUTH=kubernetes`, `CORI_VAULT_ROLE`; the token the
//!   server is given, `CORI_SERVICE_ACCOUNT_TOKEN_FILE`, else the pod's own), AppRole (`approle`, `CORI_VAULT_ROLE_ID`,
//!   `CORI_VAULT_SECRET_ID[_FILE]`), or an account of Vault's userpass or LDAP auth (`userpass`, `ldap`,
//!   `CORI_VAULT_USERNAME`, `CORI_VAULT_PASSWORD[_FILE]`); a static token (`token`, `CORI_VAULT_TOKEN[_FILE]`) only on a
//!   laptop. Tokens are renewed before they expire and a refused one logs in again. `CORI_VAULT_NAMESPACE` for Vault
//!   Enterprise; `CORI_CA_FILE` (the core's) for a private CA.
use anyhow::{Context, Result, anyhow, bail};
use base64::Engine as _;
use cori_server::module::{KeyFuture, KeyWrapper, Module, SecretScope, Wrapped};
use cori_server::preflight::Check;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

mod kv;
mod transit;
pub use kv::Kv;
pub use transit::Transit;

/// The module, which `cori_cli::run` gives every binary built on the core (`cori_cli::core_modules`). It does nothing
/// unless `CORI_VAULT_ADDR` is set.
pub struct Vault;

impl Module for Vault {
    fn id(&self) -> &'static str { "vault" }

    fn key_wrapper(&self) -> Result<Option<Arc<dyn KeyWrapper>>> {
        Ok(Settings::from_env()?.map(|s| Arc::new(Wrapper::new(s)) as Arc<dyn KeyWrapper>))
    }

    fn production_problems(&self) -> Vec<String> {
        match Settings::from_env() {
            Ok(Some(s)) => s.problems(),
            Ok(None) => Vec::new(),
            Err(e) => vec![format!("Vault: {e:#}")],
        }
    }

    fn preflight<'a>(&'a self) -> futures::future::BoxFuture<'a, Vec<Check>> {
        Box::pin(async {
            match Settings::from_env() {
                Ok(None) => vec![Check::skipped("vault", "CORI_VAULT_ADDR is not set: no secret's key is kept in Vault")],
                Err(e) => vec![Check::failed("vault", format!("{e:#}"), "fix the CORI_VAULT_* settings")],
                Ok(Some(s)) => Wrapper::new(s).preflight().await,
            }
        })
    }
}

/// How Cori logs in to Vault.
#[derive(Clone)]
pub enum Auth {
    /// Kubernetes auth: the pod's service account token, exchanged for a Vault token under `role`.
    Kubernetes { mount: String, role: String, token_file: String },
    AppRole { mount: String, role_id: String, secret_id: String },
    /// An account of Vault's userpass or LDAP auth method (`mount`: `userpass`, `ldap`, or where it is mounted).
    Account { mount: String, username: String, password: String },
    /// A static token: a laptop or a test only (`production_problems` says so).
    Token(String),
}

impl Auth {
    fn label(&self) -> String {
        match self {
            Auth::Kubernetes { role, .. } => format!("kubernetes auth (role {role})"),
            Auth::AppRole { .. } => "AppRole".into(),
            Auth::Account { mount, username, .. } => format!("the account `{username}` ({mount} auth)"),
            Auth::Token(_) => "a static token".into(),
        }
    }
}

/// What wraps the apps' data keys in Vault (`CORI_VAULT_ENGINE`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Engine {
    /// A Transit key per app, which never leaves Vault.
    Transit,
    /// A master key per app in the KV engine (version 2), which Cori reads and wraps with.
    Kv,
}

/// The `CORI_VAULT_*` settings.
#[derive(Clone)]
pub struct Settings {
    pub addr: String,
    pub namespace: Option<String>,
    pub auth: Auth,
    pub engine: Engine,
    /// The Transit engine's mount (`transit`).
    pub mount: String,
    /// What every Transit key's name starts with (`cori`): `<prefix>-app-<app>`.
    pub prefix: String,
    /// The KV engine's mount (`secret`), and the path under it where the apps' keys are (`cori`: `cori/apps/<app>`).
    pub kv_mount: String,
    pub kv_path: String,
    pub create_keys: bool,
    pub delete_keys: bool,
    /// The key that wraps the deployment's own secrets too (a Transit key's name, or a KV path under the mount); None, the
    /// master key does.
    pub deployment_key: Option<String>,
    /// How long what Cori unwrapped is kept: a data key (Transit), an app's key (KV).
    pub cache: Duration,
}

/// A path of letters, digits, `.`, `-` and `_` segments, such as Vault policies name.
fn plain_path(p: &str) -> bool {
    !p.is_empty() && p.split('/').all(|seg| !seg.is_empty() && seg != "." && seg != ".." && seg.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')))
}

impl Settings {
    /// The settings, None when `CORI_VAULT_ADDR` is not set (the module then does nothing).
    pub fn from_env() -> Result<Option<Self>> {
        let env = |k: &str| std::env::var(k).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
        let file_or = |k: &str| -> Result<Option<String>> {
            if let Some(v) = env(k) { return Ok(Some(v)); }
            match env(&format!("{k}_FILE")) { Some(p) => Ok(Some(std::fs::read_to_string(&p).with_context(|| format!("{k}_FILE `{p}`"))?.trim().to_string())), None => Ok(None) }
        };
        let flag = |k: &str, default: bool| env(k).map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on")).unwrap_or(default);
        let Some(addr) = env("CORI_VAULT_ADDR") else { return Ok(None) };
        // a token projected for a key service (CORI_SERVICE_ACCOUNT_TOKEN_FILE), else the pod's own
        let sa_token = env("CORI_VAULT_K8S_TOKEN_FILE").or_else(|| env("CORI_SERVICE_ACCOUNT_TOKEN_FILE")).unwrap_or_else(|| "/var/run/secrets/kubernetes.io/serviceaccount/token".into());
        let method = env("CORI_VAULT_AUTH").unwrap_or_else(|| if std::path::Path::new(&sa_token).exists() { "kubernetes".into() } else { "token".into() });
        let auth = match method.as_str() {
            "kubernetes" => Auth::Kubernetes {
                mount: env("CORI_VAULT_AUTH_MOUNT").unwrap_or_else(|| "kubernetes".into()),
                role: env("CORI_VAULT_ROLE").ok_or_else(|| anyhow!("CORI_VAULT_AUTH=kubernetes needs CORI_VAULT_ROLE (the Vault role bound to Cori's service account)"))?,
                token_file: sa_token,
            },
            "approle" => Auth::AppRole {
                mount: env("CORI_VAULT_AUTH_MOUNT").unwrap_or_else(|| "approle".into()),
                role_id: env("CORI_VAULT_ROLE_ID").ok_or_else(|| anyhow!("CORI_VAULT_AUTH=approle needs CORI_VAULT_ROLE_ID"))?,
                secret_id: file_or("CORI_VAULT_SECRET_ID")?.ok_or_else(|| anyhow!("CORI_VAULT_AUTH=approle needs CORI_VAULT_SECRET_ID or CORI_VAULT_SECRET_ID_FILE"))?,
            },
            "userpass" | "ldap" => Auth::Account {
                mount: env("CORI_VAULT_AUTH_MOUNT").unwrap_or_else(|| method.clone()),
                username: env("CORI_VAULT_USERNAME").ok_or_else(|| anyhow!("CORI_VAULT_AUTH={method} needs CORI_VAULT_USERNAME (Cori's account in Vault)"))?,
                password: file_or("CORI_VAULT_PASSWORD")?.ok_or_else(|| anyhow!("CORI_VAULT_AUTH={method} needs CORI_VAULT_PASSWORD or CORI_VAULT_PASSWORD_FILE"))?,
            },
            "token" => Auth::Token(file_or("CORI_VAULT_TOKEN")?.ok_or_else(|| anyhow!("CORI_VAULT_AUTH=token needs CORI_VAULT_TOKEN or CORI_VAULT_TOKEN_FILE"))?),
            other => bail!("CORI_VAULT_AUTH: kubernetes, approle, userpass, ldap or token, not `{other}`"),
        };
        let engine = match env("CORI_VAULT_ENGINE").map(|v| v.to_ascii_lowercase()).as_deref() {
            None | Some("transit") => Engine::Transit,
            Some("kv") => Engine::Kv,
            Some(other) => bail!("CORI_VAULT_ENGINE: transit (a Transit key per app) or kv (a master key per app in the KV engine), not `{other}`"),
        };
        let prefix = env("CORI_VAULT_KEY_PREFIX").unwrap_or_else(|| "cori".into());
        if !prefix.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') { bail!("CORI_VAULT_KEY_PREFIX: letters, digits, - and _ only"); }
        let kv_mount = env("CORI_VAULT_KV_MOUNT").unwrap_or_else(|| "secret".into()).trim_matches('/').to_string();
        let kv_path = env("CORI_VAULT_KV_PATH").map(|p| p.trim_matches('/').to_string()).unwrap_or_else(|| prefix.clone());
        if !plain_path(&kv_mount) { bail!("CORI_VAULT_KV_MOUNT: a mount path of letters, digits, ., - and _ segments"); }
        if !plain_path(&kv_path) { bail!("CORI_VAULT_KV_PATH: a path of letters, digits, ., - and _ segments, such as cori or teams/platform/cori"); }
        let deployment_key = env("CORI_VAULT_DEPLOYMENT_KEY").map(|k| k.trim_matches('/').to_string());
        if engine == Engine::Kv && let Some(k) = &deployment_key && !plain_path(k) { bail!("CORI_VAULT_DEPLOYMENT_KEY: a path under CORI_VAULT_KV_MOUNT, such as cori/deployment"); }
        Ok(Some(Self {
            addr: addr.trim_end_matches('/').to_string(), namespace: env("CORI_VAULT_NAMESPACE"), auth, engine,
            mount: env("CORI_VAULT_TRANSIT_MOUNT").unwrap_or_else(|| "transit".into()).trim_matches('/').to_string(), prefix, kv_mount, kv_path,
            create_keys: flag("CORI_VAULT_CREATE_KEYS", true), delete_keys: flag("CORI_VAULT_DELETE_KEYS", false),
            deployment_key,
            cache: Duration::from_secs(env("CORI_VAULT_CACHE_SECS").and_then(|v| v.parse().ok()).unwrap_or(60)),
        }))
    }

    /// What must not reach a network: plain http to a remote Vault, a static token.
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        let loopback = ["http://127.0.0.1", "http://localhost", "http://[::1]"].iter().any(|p| self.addr.starts_with(p));
        let what = match self.engine { Engine::Transit => "data keys", Engine::Kv => "the apps' keys" };
        if self.addr.starts_with("http://") && !loopback { out.push(format!("CORI_VAULT_ADDR `{}` is not https: {what} would travel in clear", self.addr)); }
        if matches!(self.auth, Auth::Token(_)) { out.push("CORI_VAULT_AUTH=token: a static Vault token; log in with kubernetes, approle, userpass or ldap".into()); }
        out
    }
}

/// What an engine unwrapped or read, by what names it, and when: kept `Settings::cache` at most.
type Cache<K> = Mutex<HashMap<K, ([u8; 32], Instant)>>;

/// A Vault token and when to renew it.
struct Token { value: String, renew_at: Option<Instant>, expires_at: Option<Instant>, renewable: bool }

fn b64(b: &[u8]) -> String { base64::engine::general_purpose::STANDARD.encode(b) }
fn unb64(s: &str) -> Result<Vec<u8>> { base64::engine::general_purpose::STANDARD.decode(s).context("base64") }

/// A refusal of Vault's, with its own words.
#[derive(Debug)]
struct Refused { status: u16, message: String }
impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f, "Vault answered {}: {}", self.status, self.message) }
}
impl std::error::Error for Refused {}

fn status_of(e: &anyhow::Error) -> Option<u16> { e.downcast_ref::<Refused>().map(|r| r.status) }

/// Vault's HTTP API with Cori's login, which both engines share.
pub struct Client {
    s: Settings,
    http: reqwest::Client,
    token: tokio::sync::Mutex<Option<Token>>,
}

impl Client {
    pub fn new(s: Settings) -> Self {
        let http = cori_engine::http::builder().timeout(Duration::from_secs(10)).connect_timeout(Duration::from_secs(5)).build().unwrap_or_default();
        Self { s, http, token: tokio::sync::Mutex::new(None) }
    }

    async fn send(&self, method: reqwest::Method, path: &str, body: Option<&Value>, token: Option<&str>) -> Result<Value> {
        let mut req = self.http.request(method, format!("{}/v1/{path}", self.s.addr));
        if let Some(t) = token { req = req.header("x-vault-token", t); }
        if let Some(ns) = &self.s.namespace { req = req.header("x-vault-namespace", ns); }
        if let Some(b) = body { req = req.json(b); }
        let res = req.send().await.map_err(|e| anyhow!("Vault at {}: {e}", self.s.addr))?;
        let status = res.status().as_u16();
        let v: Value = res.json().await.unwrap_or(Value::Null);
        if (200..300).contains(&status) { return Ok(v); }
        let message = v["errors"].as_array().map(|a| a.iter().filter_map(|e| e.as_str()).collect::<Vec<_>>().join("; ")).filter(|m| !m.is_empty()).unwrap_or_else(|| "no detail".into());
        Err(Refused { status, message }.into())
    }

    async fn login(&self) -> Result<Token> {
        let (path, body) = match &self.s.auth {
            Auth::Token(t) => return Ok(Token { value: t.clone(), renew_at: None, expires_at: None, renewable: false }),
            Auth::Kubernetes { mount, role, token_file } => {
                let jwt = std::fs::read_to_string(token_file).with_context(|| format!("the service account token `{token_file}`"))?;
                (format!("auth/{mount}/login"), json!({ "role": role, "jwt": jwt.trim() }))
            }
            Auth::AppRole { mount, role_id, secret_id } => (format!("auth/{mount}/login"), json!({ "role_id": role_id, "secret_id": secret_id })),
            Auth::Account { mount, username, password } => (format!("auth/{mount}/login/{}", urlencoding::encode(username)), json!({ "password": password })),
        };
        let v = self.send(reqwest::Method::POST, &path, Some(&body), None).await.map_err(|e| anyhow!("logging in to Vault with {}: {e:#}", self.s.auth.label()))?;
        Self::token_of(&v)
    }

    fn token_of(v: &Value) -> Result<Token> {
        let value = v["auth"]["client_token"].as_str().filter(|t| !t.is_empty()).ok_or_else(|| anyhow!("Vault's login answer has no client_token"))?.to_string();
        let lease = v["auth"]["lease_duration"].as_u64().unwrap_or(0);
        let now = Instant::now();
        let (renew_at, expires_at) = if lease == 0 { (None, None) } else { (Some(now + Duration::from_secs(lease * 2 / 3)), Some(now + Duration::from_secs(lease))) };
        Ok(Token { value, renew_at, expires_at, renewable: v["auth"]["renewable"].as_bool().unwrap_or(false) })
    }

    /// A valid token: the current one, renewed when two thirds of its life are gone, else a new login.
    async fn token(&self, fresh: bool) -> Result<String> {
        let mut guard = self.token.lock().await;
        let now = Instant::now();
        if !fresh && let Some(t) = guard.as_ref() {
            if t.renew_at.is_none_or(|r| now < r) { return Ok(t.value.clone()); }
            if t.renewable && t.expires_at.is_some_and(|e| now < e) {
                match self.send(reqwest::Method::POST, "auth/token/renew-self", Some(&json!({})), Some(&t.value)).await {
                    Ok(v) => { let renewed = Self::token_of(&v)?; let value = renewed.value.clone(); *guard = Some(renewed); return Ok(value); }
                    Err(e) => tracing::info!(error = %e, "Vault token not renewed: logging in again"),
                }
            }
        }
        let t = self.login().await?;
        let value = t.value.clone();
        *guard = Some(t);
        Ok(value)
    }

    /// A call with Cori's token; a token Vault refuses (expired, revoked) logs in again once.
    async fn call(&self, method: reqwest::Method, path: &str, body: Option<&Value>) -> Result<Value> {
        let token = self.token(false).await?;
        match self.send(method.clone(), path, body, Some(&token)).await {
            Err(e) if status_of(&e) == Some(403) && !matches!(self.s.auth, Auth::Token(_)) => {
                let token = self.token(true).await?;
                self.send(method, path, body, Some(&token)).await
            }
            r => r,
        }
    }

    /// `cori preflight`'s first check: a login.
    async fn preflight_login(&self) -> Check {
        match self.token(true).await {
            Ok(_) => Check::ok("vault login", format!("{} at {}", self.s.auth.label(), self.s.addr)),
            Err(e) => Check::failed("vault login", format!("{e:#}"), "check CORI_VAULT_ADDR, the auth method and its role or account, and that this pod's service account is bound to it"),
        }
    }
}

/// The key wrapper the module provides: the engine `CORI_VAULT_ENGINE` names wraps every data key; a row the other
/// engine wrapped (before the setting changed) opens with that engine, and the rewrap moves it to this one.
pub struct Wrapper { engine: Engine, client: Arc<Client>, transit: Transit, kv: Kv }

impl Wrapper {
    pub fn new(s: Settings) -> Self {
        let client = Arc::new(Client::new(s.clone()));
        Self { engine: s.engine, transit: Transit::with_client(s.clone(), client.clone()), kv: Kv::with_client(s, client.clone()), client }
    }

    fn of(&self, engine: Engine) -> &dyn KeyWrapper { match engine { Engine::Transit => &self.transit, Engine::Kv => &self.kv } }

    fn configured(&self) -> &dyn KeyWrapper { self.of(self.engine) }

    /// The engine that wrapped a row, from its `key_by`.
    fn engine_of(key_by: &str) -> Result<Engine> {
        if key_by.starts_with(transit::KEY_BY) { Ok(Engine::Transit) } else if key_by.starts_with(kv::KEY_BY) { Ok(Engine::Kv) } else { bail!("`{key_by}` is not a key of Vault's") }
    }

    /// `cori preflight`: a login, then a wrap and an unwrap with the configured engine's preflight key.
    pub async fn preflight(&self) -> Vec<Check> {
        let login = self.client.preflight_login().await;
        if login.is_failed() { return vec![login]; }
        vec![login, match self.engine { Engine::Transit => self.transit.preflight().await, Engine::Kv => self.kv.preflight().await }]
    }
}

impl KeyWrapper for Wrapper {
    fn id(&self) -> &'static str { self.configured().id() }

    fn covers(&self, scope: &SecretScope) -> bool { self.configured().covers(scope) }

    fn prepare<'a>(&'a self, scope: &'a SecretScope) -> KeyFuture<'a, Option<String>> { self.configured().prepare(scope) }

    fn wrap<'a>(&'a self, scope: &'a SecretScope, key: &'a [u8; 32], aad: &'a str) -> KeyFuture<'a, Wrapped> { self.configured().wrap(scope, key, aad) }

    fn unwrap<'a>(&'a self, scope: &'a SecretScope, key_by: &'a str, wrapped: &'a str, aad: &'a str) -> KeyFuture<'a, [u8; 32]> {
        Box::pin(async move { self.of(Self::engine_of(key_by)?).unwrap(scope, key_by, wrapped, aad).await })
    }

    fn rewrap<'a>(&'a self, scope: &'a SecretScope, key_by: &'a str, wrapped: &'a str, aad: &'a str) -> KeyFuture<'a, Option<Wrapped>> {
        Box::pin(async move {
            let from = Self::engine_of(key_by)?;
            if from == self.engine { return self.configured().rewrap(scope, key_by, wrapped, aad).await; }
            // wrapped by the other engine, before CORI_VAULT_ENGINE changed: moved to this one
            let key = self.of(from).unwrap(scope, key_by, wrapped, aad).await?;
            Ok(Some(self.configured().wrap(scope, &key, aad).await?))
        })
    }

    fn forget<'a>(&'a self, scope: &'a SecretScope) -> KeyFuture<'a, ()> { self.configured().forget(scope) }
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_kv;
