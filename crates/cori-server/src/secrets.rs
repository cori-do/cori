// SPDX-License-Identifier: Apache-2.0
//! Credential broker: secrets are written once, envelope-encrypted (AES-256-GCM data key per secret), never returned by
//! any API except as a short-lived lease to a worker for one run and one connector.
//!
//! A secret belongs to the deployment or to one app (its scope, `secrets.scope`: '' or the app's id). An app's credentials
//! are its own: its connectors' secrets, its webhooks' signing secrets, its schedules' service-account secrets and its store
//! databases' logins. A run of an app finds a name in its app's scope first, then in the deployment's
//! (`CORI_SECRETS_APP_SCOPE=require`: in its app's only).
//!
//! What wraps a secret's data key is recorded with it (`secrets.key_by`): the master key (`master`; at
//! <data_dir>/master.key or CORI_MASTER_KEY), or a module's key wrapper (`KeyWrapper`: an external key service with keys
//! of its own per scope) for the scopes it covers. Under a tenancy each realm has its own broker, keyed by the realm's key
//! (`realm::Admitted`).
use aes_gcm::{Aes256Gcm, KeyInit, aead::Aead};
use anyhow::{Result, anyhow};
use base64::Engine;
use sqlx::Row;
use std::sync::Arc;

#[derive(Clone)]
pub struct Broker { master: [u8; 32], wrapper: Option<Arc<dyn KeyWrapper>> }

fn b64(b: &[u8]) -> String { base64::engine::general_purpose::STANDARD.encode(b) }
fn unb64(s: &str) -> Result<Vec<u8>> { Ok(base64::engine::general_purpose::STANDARD.decode(s)?) }

/// Whose a secret is: the deployment's (`app` None) or one app's, in the realm of the database that keeps it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretScope { pub realm: Option<crate::db::Realm>, pub app: Option<String> }

impl SecretScope {
    /// The scope of `app`'s secrets (None: the deployment's) as `db` sees them.
    pub fn of(db: &crate::db::Db, app: Option<&str>) -> Self { Self { realm: db.realm().cloned(), app: app.map(String::from) } }
    /// `secrets.scope`: '' for the deployment, else the app's id.
    pub fn column(&self) -> &str { self.app.as_deref().unwrap_or("") }
}

/// A data key wrapped by a `KeyWrapper`: what wrapped it (the row's `key_by`, e.g. `vault-transit:cori-app-desk:3`) and the
/// wrapped key, as text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wrapped { pub key_by: String, pub wrapped: String }

pub type KeyFuture<'a, T> = std::pin::Pin<Box<dyn std::future::Future<Output = Result<T>> + Send + 'a>>;

/// What wraps the data keys of secrets besides the master key: an external key service with keys of its own per scope,
/// which a module provides (`Module::key_wrapper`). The broker asks it for the scopes it `covers` (an app's, say), the
/// master key wraps the others; each row records which (`key_by`), so rows of either kind are read whatever the setting.
/// The data key is all it ever sees: values are sealed by the broker, in the platform database.
pub trait KeyWrapper: Send + Sync + 'static {
    /// A short name, for the logs and `cori --version` (`vault-transit`).
    fn id(&self) -> &'static str;
    /// Whether it wraps the data keys of `scope`'s secrets; the master key wraps the others.
    fn covers(&self, scope: &SecretScope) -> bool;
    /// Makes the scope's key ready (an app was just created): its name, which the client hook's `app.created` carries so
    /// that the org's automation can set its policies up; None when there is nothing to name.
    fn prepare<'a>(&'a self, _scope: &'a SecretScope) -> KeyFuture<'a, Option<String>> { Box::pin(async { Ok(None) }) }
    /// Wraps a data key of `scope`; `aad` binds it to its secret.
    fn wrap<'a>(&'a self, scope: &'a SecretScope, key: &'a [u8; 32], aad: &'a str) -> KeyFuture<'a, Wrapped>;
    /// The data key `wrap` gave `wrapped` for (`key_by` as recorded).
    fn unwrap<'a>(&'a self, scope: &'a SecretScope, key_by: &'a str, wrapped: &'a str, aad: &'a str) -> KeyFuture<'a, [u8; 32]>;
    /// The wrapped key under the scope's newest key version, without the key leaving the service; None when it already is.
    fn rewrap<'a>(&'a self, _scope: &'a SecretScope, _key_by: &'a str, _wrapped: &'a str, _aad: &'a str) -> KeyFuture<'a, Option<Wrapped>> { Box::pin(async { Ok(None) }) }
    /// The scope is gone (its app was deleted, and its secrets with it): what becomes of its key is the wrapper's call.
    fn forget<'a>(&'a self, _scope: &'a SecretScope) -> KeyFuture<'a, ()> { Box::pin(async { Ok(()) }) }
}

/// Whether a run of an app may fall back on the deployment's secret of a name its app has none of (`CORI_SECRETS_APP_SCOPE`:
/// `prefer`, the default, or `require`).
pub fn deployment_fallback() -> bool {
    !std::env::var("CORI_SECRETS_APP_SCOPE").map(|v| v.trim().eq_ignore_ascii_case("require")).unwrap_or(false)
}

/// The names the broker keeps for Cori itself (a store database's logins): nobody sets them through the API.
pub fn reserved_name(name: &str) -> bool { name.starts_with(STORE_LOGIN_PREFIX) }

/// The secrets holding a store database's logins: `store-login.<database>.owner`, `.runtime` (`stores`).
pub const STORE_LOGIN_PREFIX: &str = "store-login.";

impl Broker {
    /// A master key is loaded (never all zero).
    pub fn ready(&self) -> bool { self.master.iter().any(|b| *b != 0) }
    /// A purpose-bound sub-key of the master key (e.g. `agent-tokens`): the same on every replica sharing the master.
    pub fn derive(&self, label: &str) -> [u8; 32] { use sha2::Digest; let mut h = sha2::Sha256::new(); h.update(b"cori-derive:"); h.update(label.as_bytes()); h.update([0]); h.update(self.master); h.finalize().into() }
    /// A realm's broker, keyed by the realm's own key.
    pub fn from_key(master: [u8; 32]) -> Self { Self { master, wrapper: None } }
    pub fn open(data_dir: &std::path::Path) -> Result<Self> {
        let master: [u8; 32] = if let Ok(hexkey) = std::env::var("CORI_MASTER_KEY") {
            hex::decode(hexkey.trim())?.try_into().map_err(|_| anyhow!("CORI_MASTER_KEY must be 32 bytes hex"))?
        } else {
            let path = data_dir.join("master.key");
            if path.exists() { hex::decode(std::fs::read_to_string(&path)?.trim())?.try_into().map_err(|_| anyhow!("bad master.key"))? }
            else { let k: [u8; 32] = rand::random(); std::fs::create_dir_all(data_dir)?; std::fs::write(&path, hex::encode(k))?; #[cfg(unix)] { use std::os::unix::fs::PermissionsExt; std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?; } k }
        };
        Ok(Self { master, wrapper: None })
    }
    /// The broker with a module's key wrapper for the scopes it covers.
    pub fn with_wrapper(mut self, wrapper: Option<Arc<dyn KeyWrapper>>) -> Self { self.wrapper = wrapper; self }
    /// The module's key wrapper, if any.
    pub fn wrapper(&self) -> Option<&Arc<dyn KeyWrapper>> { self.wrapper.as_ref() }

    /// Wraps a data key of `scope`: (key_by, dek_nonce, dek_wrapped) as the row records them.
    async fn wrap_key(&self, scope: &SecretScope, dek: &[u8; 32], aad: &str) -> Result<(String, String, String)> {
        match self.wrapper.as_ref().filter(|w| w.covers(scope)) {
            Some(w) => { let x = w.wrap(scope, dek, aad).await.map_err(|e| anyhow!("{}: {e:#}", w.id()))?; Ok((x.key_by, String::new(), x.wrapped)) }
            None => { let (n, c) = Self::seal(&self.master, dek, aad.as_bytes())?; Ok(("master".into(), n, c)) }
        }
    }

    /// The data key a row's `key_by`, `dek_nonce` and `dek_wrapped` hold.
    async fn unwrap_key(&self, scope: &SecretScope, key_by: &str, nonce: &str, wrapped: &str, aad: &str) -> Result<[u8; 32]> {
        if key_by == "master" { return Self::open_sealed(&self.master, nonce, wrapped, aad.as_bytes())?.try_into().map_err(|_| anyhow!("bad dek")); }
        let w = self.wrapper.as_ref().ok_or_else(|| anyhow!("the secret's key is wrapped by `{key_by}`, and this server has no key wrapper"))?;
        w.unwrap(scope, key_by, wrapped, aad).await.map_err(|e| anyhow!("{}: {e:#}", w.id()))
    }
    fn seal(key: &[u8; 32], plain: &[u8], aad: &[u8]) -> Result<(String, String)> {
        let cipher = Aes256Gcm::new(key.into());
        let nonce_bytes: [u8; 12] = rand::random();
        let nonce = aes_gcm::Nonce::from(nonce_bytes);
        let ct = cipher.encrypt(&nonce, aes_gcm::aead::Payload { msg: plain, aad }).map_err(|_| anyhow!("encrypt"))?;
        Ok((b64(&nonce_bytes), b64(&ct)))
    }
    fn open_sealed(key: &[u8; 32], nonce: &str, ct: &str, aad: &[u8]) -> Result<Vec<u8>> {
        let cipher = Aes256Gcm::new(key.into());
        let n = unb64(nonce)?; let c = unb64(ct)?;
        let n: [u8; 12] = n.as_slice().try_into().map_err(|_| anyhow!("bad nonce length"))?;
        cipher.decrypt(&aes_gcm::Nonce::from(n), aes_gcm::aead::Payload { msg: &c, aad }).map_err(|_| anyhow!("decrypt failed (wrong master key or tampered secret)"))
    }
    /// Store (or rotate) a deployment secret. Returns the new version.
    pub async fn put(&self, db: &crate::db::Db, name: &str, kind: &str, value: &str, by: &str) -> Result<i64> { self.put_in(db, None, name, kind, value, by).await }

    /// Store (or rotate) a secret of `app` (None: the deployment's). Returns the new version.
    pub async fn put_in(&self, db: &crate::db::Db, app: Option<&str>, name: &str, kind: &str, value: &str, by: &str) -> Result<i64> {
        let scope = SecretScope::of(db, app);
        let dek: [u8; 32] = rand::random();
        let aad = crate::names::secret_aad_in(db.realm(), app, name);
        let (n1, c1) = Self::seal(&dek, value.as_bytes(), aad.as_bytes())?;
        let (key_by, n2, c2) = self.wrap_key(&scope, &dek, &aad).await?;
        let now = chrono::Utc::now().to_rfc3339();
        let mut tx = db.begin().await?;
        let version: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(version), 0) + 1 FROM secrets WHERE scope = $1 AND name = $2").bind(scope.column()).bind(name).fetch_one(&mut *tx).await?;
        sqlx::query("UPDATE secrets SET revoked_at = $1 WHERE scope = $2 AND name = $3 AND revoked_at IS NULL").bind(&now).bind(scope.column()).bind(name).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO secrets (id, scope, name, kind, version, key_by, dek_nonce, dek_wrapped, nonce, ciphertext, created_by, created_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)")
            .bind(uuid::Uuid::new_v4().to_string()).bind(scope.column()).bind(name).bind(kind).bind(version).bind(&key_by).bind(n2).bind(c2).bind(n1).bind(c1).bind(by).bind(&now).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(version)
    }

    /// Decrypt the current version of a deployment secret. Only leases and the server's own calls read values; no API returns them.
    pub async fn reveal(&self, db: &crate::db::Db, name: &str) -> Result<String> { self.reveal_in(db, None, name).await }

    /// Decrypt the current version of a secret of `app` (None: the deployment's).
    pub async fn reveal_in(&self, db: &crate::db::Db, app: Option<&str>, name: &str) -> Result<String> {
        let scope = SecretScope::of(db, app);
        let r = sqlx::query("SELECT key_by, dek_nonce, dek_wrapped, nonce, ciphertext FROM secrets WHERE scope = $1 AND name = $2 AND revoked_at IS NULL ORDER BY version DESC LIMIT 1")
            .bind(scope.column()).bind(name).fetch_optional(db).await?
            .ok_or_else(|| match app { Some(a) => anyhow!("unknown secret `{name}` of app `{a}`"), None => anyhow!("unknown secret `{name}`") })?;
        let aad = crate::names::secret_aad_in(db.realm(), app, name);
        let dek = self.unwrap_key(&scope, &r.get::<String, _>("key_by"), &r.get::<String, _>("dek_nonce"), &r.get::<String, _>("dek_wrapped"), &aad).await?;
        Ok(String::from_utf8(Self::open_sealed(&dek, &r.get::<String, _>("nonce"), &r.get::<String, _>("ciphertext"), aad.as_bytes())?)?)
    }

    /// The secret `name` as a run of `app` sees it: its app's own, else the deployment's (unless `deployment_fallback` says
    /// otherwise). Returns the value and the app whose it was (None: the deployment's).
    pub async fn resolve(&self, db: &crate::db::Db, app: &str, name: &str) -> Result<(String, Option<String>)> {
        if self.exists_in(db, Some(app), name).await? { return Ok((self.reveal_in(db, Some(app), name).await?, Some(app.to_string()))); }
        if !deployment_fallback() { return Err(anyhow!("unknown secret `{name}` of app `{app}` (CORI_SECRETS_APP_SCOPE=require: an app's credentials are its own)")); }
        Ok((self.reveal_in(db, None, name).await?, None))
    }

    /// Whether `app`'s scope (None: the deployment's) holds a current version of `name`.
    pub async fn exists_in(&self, db: &crate::db::Db, app: Option<&str>, name: &str) -> Result<bool> {
        Ok(sqlx::query("SELECT 1 FROM secrets WHERE scope = $1 AND name = $2 AND revoked_at IS NULL LIMIT 1").bind(app.unwrap_or("")).bind(name).fetch_optional(db).await?.is_some())
    }

    /// Deletes every secret of `app` (the app is being deleted) and tells the key wrapper its scope is gone. Returns the
    /// rows deleted.
    pub async fn forget_app(&self, db: &crate::db::Db, app: &str) -> Result<u64> {
        let n = sqlx::query("DELETE FROM secrets WHERE scope = $1").bind(app).execute(db).await?.rows_affected();
        let scope = SecretScope::of(db, Some(app));
        if let Some(w) = self.wrapper.as_ref().filter(|w| w.covers(&scope)) { w.forget(&scope).await.map_err(|e| anyhow!("{}: {e:#}", w.id()))?; }
        Ok(n)
    }

    /// Deletes the secrets of `app` whose names start with `prefix` (a store database's logins, with the database).
    pub async fn forget_prefix(&self, db: &crate::db::Db, app: Option<&str>, prefix: &str) -> Result<u64> {
        Ok(sqlx::query("DELETE FROM secrets WHERE scope = $1 AND starts_with(name, $2)").bind(app.unwrap_or("")).bind(prefix).execute(db).await?.rows_affected())
    }

    /// Makes `app`'s key ready with the key wrapper, when it covers the app: the key's name, for `app.created`.
    pub async fn prepare_app(&self, db: &crate::db::Db, app: &str) -> Result<Option<String>> {
        let scope = SecretScope::of(db, Some(app));
        match self.wrapper.as_ref().filter(|w| w.covers(&scope)) { Some(w) => w.prepare(&scope).await.map_err(|e| anyhow!("{}: {e:#}", w.id())), None => Ok(None) }
    }

    /// Re-wraps, under the newest version of their keys, the data keys the key wrapper wraps (and those the master key wraps
    /// in a scope the wrapper now covers): the leading replica runs it now and then, so that the org may retire old key
    /// versions. A row that does not move now (its key withdrawn from Cori, the key service refusing) stays as it is, said
    /// in the logs, and the others move on. Returns how many rows changed.
    pub async fn rewrap(&self, db: &crate::db::Db) -> Result<usize> {
        let Some(w) = self.wrapper.clone() else { return Ok(0) };
        let rows = sqlx::query("SELECT id, scope, name, key_by, dek_nonce, dek_wrapped FROM secrets WHERE revoked_at IS NULL").fetch_all(db).await?;
        let (mut changed, mut stuck) = (0, Vec::new());
        for r in rows {
            let (id, scope_col, name, key_by): (String, String, String, String) = (r.get("id"), r.get("scope"), r.get("name"), r.get("key_by"));
            let app = (!scope_col.is_empty()).then_some(scope_col.as_str());
            let scope = SecretScope::of(db, app);
            if !w.covers(&scope) { continue; }
            let aad = crate::names::secret_aad_in(db.realm(), app, &name);
            let next = async {
                if key_by == "master" {
                    // a scope the wrapper took over since: its master-wrapped keys move under the wrapper's key
                    let dek = self.unwrap_key(&scope, &key_by, &r.get::<String, _>("dek_nonce"), &r.get::<String, _>("dek_wrapped"), &aad).await?;
                    Ok::<_, anyhow::Error>(Some(w.wrap(&scope, &dek, &aad).await?))
                } else {
                    w.rewrap(&scope, &key_by, &r.get::<String, _>("dek_wrapped"), &aad).await
                }
            };
            match next.await {
                Ok(Some(x)) => {
                    sqlx::query("UPDATE secrets SET key_by = $1, dek_nonce = '', dek_wrapped = $2 WHERE id = $3").bind(&x.key_by).bind(&x.wrapped).bind(&id).execute(db).await?;
                    changed += 1;
                }
                Ok(None) => {}
                Err(e) => stuck.push(format!("`{name}`{}: {}: {e:#}", app.map(|a| format!(" of `{a}`")).unwrap_or_default(), w.id())),
            }
        }
        if let Some(first) = stuck.first() { tracing::warn!(rows = stuck.len(), first = %first, "data keys not re-wrapped: the next rewrap tries them again"); }
        Ok(changed)
    }
}

/// What a value sealed for a native-activity connector's worker starts with (`Broker::seal_for_connector`).
pub const SEALED_PREFIX: &str = "cori-sealed.v1.";

impl Broker {
    /// The key a native-activity connector's own worker holds (`CORI_CONNECTOR_KEY`; `cori connectors key <name>`): it
    /// opens the run's token in the activity's envelope.
    pub fn connector_key(&self, connector: &str) -> [u8; 32] { self.derive(&format!("native-token:{connector}")) }

    /// A run's token sealed for `connector`'s worker: the envelope of a native activity, which carries it, is written into
    /// the run's Temporal history (`seal_native`).
    pub fn seal_for_connector(&self, connector: &str, run_id: &str, token: &str) -> Result<String> { seal_native(&self.connector_key(connector), run_id, token, rand::random()) }
}

/// `SEALED_PREFIX` + base64url (no padding) of nonce (12 bytes) ‖ ciphertext ‖ tag: AES-256-GCM under `key`, the run id
/// as associated data, so that a sealed token opens for its own run only. The SDKs open it (sdks/go `OpenToken`,
/// sdks/dotnet `SealedToken.Open`; sdks/testdata holds a sealed token and its key).
pub fn seal_native(key: &[u8; 32], run_id: &str, token: &str, nonce: [u8; 12]) -> Result<String> {
    let ct = Aes256Gcm::new(key.into()).encrypt(&aes_gcm::Nonce::from(nonce), aes_gcm::aead::Payload { msg: token.as_bytes(), aad: run_id.as_bytes() }).map_err(|_| anyhow!("encrypt"))?;
    let mut sealed = nonce.to_vec();
    sealed.extend(ct);
    Ok(format!("{SEALED_PREFIX}{}", base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sealed)))
}

/// The token `seal_native` sealed for the run `run_id`.
pub fn open_native(key: &[u8; 32], run_id: &str, sealed: &str) -> Result<String> {
    let b = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(sealed.strip_prefix(SEALED_PREFIX).ok_or_else(|| anyhow!("not a sealed token"))?)?;
    if b.len() < 12 + 16 { return Err(anyhow!("sealed token too short")); }
    let (nonce, ct) = b.split_at(12);
    let nonce: [u8; 12] = nonce.try_into().map_err(|_| anyhow!("bad nonce"))?;
    let plain = Aes256Gcm::new(key.into()).decrypt(&aes_gcm::Nonce::from(nonce), aes_gcm::aead::Payload { msg: ct, aad: run_id.as_bytes() }).map_err(|_| anyhow!("the token does not open with this key for this run"))?;
    Ok(String::from_utf8(plain)?)
}

impl Broker {
    /// Encrypts an arbitrary value under the master key (used for per-run principal tokens).
    pub fn seal_value(&self, plain: &str, aad: &str) -> Result<String> { let (n, c) = Self::seal(&self.master, plain.as_bytes(), aad.as_bytes())?; Ok(format!("{n}.{c}")) }
    pub fn open_value(&self, sealed: &str, aad: &str) -> Result<String> { let (n, c) = sealed.split_once('.').ok_or_else(|| anyhow!("bad sealed value"))?; Ok(String::from_utf8(Self::open_sealed(&self.master, n, c, aad.as_bytes())?)?) }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A key wrapper standing in for an external service: one key per app (derived from the app's name and a version it
    /// counts), app scopes only; `forget` remembers what it was told; the apps in `withdrawn` no longer move.
    #[derive(Default)]
    struct PerApp { version: std::sync::atomic::AtomicU32, forgotten: std::sync::Mutex<Vec<String>>, withdrawn: std::sync::Mutex<Vec<String>> }
    impl PerApp {
        fn key(app: &str, v: u32) -> [u8; 32] { use sha2::Digest; sha2::Sha256::digest(format!("{app}:{v}").as_bytes()).into() }
        fn label(app: &str, v: u32) -> String { format!("per-app:{app}:{v}") }
    }
    impl KeyWrapper for PerApp {
        fn id(&self) -> &'static str { "per-app" }
        fn covers(&self, scope: &SecretScope) -> bool { scope.app.is_some() }
        fn prepare<'a>(&'a self, scope: &'a SecretScope) -> KeyFuture<'a, Option<String>> { Box::pin(async move { Ok(scope.app.as_ref().map(|a| format!("key-{a}"))) }) }
        fn wrap<'a>(&'a self, scope: &'a SecretScope, key: &'a [u8; 32], aad: &'a str) -> KeyFuture<'a, Wrapped> {
            Box::pin(async move {
                let (app, v) = (scope.app.clone().unwrap(), self.version.load(std::sync::atomic::Ordering::SeqCst) + 1);
                let (n, c) = Broker::seal(&Self::key(&app, v), key, aad.as_bytes())?;
                Ok(Wrapped { key_by: Self::label(&app, v), wrapped: format!("{n}.{c}") })
            })
        }
        fn unwrap<'a>(&'a self, scope: &'a SecretScope, key_by: &'a str, wrapped: &'a str, aad: &'a str) -> KeyFuture<'a, [u8; 32]> {
            Box::pin(async move {
                let app = scope.app.clone().unwrap();
                let v: u32 = key_by.strip_prefix(&format!("per-app:{app}:")).ok_or_else(|| anyhow!("not this app's key: {key_by}"))?.parse()?;
                let (n, c) = wrapped.split_once('.').ok_or_else(|| anyhow!("bad wrapped key"))?;
                Broker::open_sealed(&Self::key(&app, v), n, c, aad.as_bytes())?.try_into().map_err(|_| anyhow!("bad key"))
            })
        }
        fn rewrap<'a>(&'a self, scope: &'a SecretScope, key_by: &'a str, wrapped: &'a str, aad: &'a str) -> KeyFuture<'a, Option<Wrapped>> {
            Box::pin(async move {
                if self.withdrawn.lock().unwrap().contains(scope.app.as_ref().unwrap()) { return Err(anyhow!("the key of `{}` is withdrawn", scope.app.as_deref().unwrap())); }
                let current = Self::label(scope.app.as_deref().unwrap(), self.version.load(std::sync::atomic::Ordering::SeqCst) + 1);
                if key_by == current { return Ok(None); }
                let key = self.unwrap(scope, key_by, wrapped, aad).await?;
                Ok(Some(self.wrap(scope, &key, aad).await?))
            })
        }
        fn forget<'a>(&'a self, scope: &'a SecretScope) -> KeyFuture<'a, ()> { Box::pin(async move { self.forgotten.lock().unwrap().push(scope.app.clone().unwrap()); Ok(()) }) }
    }

    /// An app's credentials are its own: its runs find them before the deployment's of the same name, another app's runs
    /// never do, and a row moved to another app does not open. Needs CORI_STORE_DATABASE_URL.
    #[tokio::test]
    async fn an_apps_credentials_are_its_own() {
        let Some(db) = crate::db::test_db().await else { eprintln!("CORI_STORE_DATABASE_URL not set: skipped"); return };
        let b = Broker::from_key([3; 32]);
        b.put(&db, "crm.token", "token", "deployment-value", "admin").await.unwrap();
        assert_eq!(b.put_in(&db, Some("desk"), "crm.token", "token", "desk-value", "owner").await.unwrap(), 1, "versions count per scope");
        assert_eq!(b.put_in(&db, Some("desk"), "crm.token", "token", "desk-value-2", "owner").await.unwrap(), 2);
        assert_eq!(b.resolve(&db, "desk", "crm.token").await.unwrap(), ("desk-value-2".to_string(), Some("desk".to_string())));
        assert_eq!(b.resolve(&db, "billing", "crm.token").await.unwrap(), ("deployment-value".to_string(), None), "the deployment's, for an app with none");
        assert_eq!(b.reveal(&db, "crm.token").await.unwrap(), "deployment-value");
        assert!(b.reveal_in(&db, Some("billing"), "crm.token").await.is_err());
        // a row moved to another app does not open there
        sqlx::query("UPDATE secrets SET scope = 'billing' WHERE scope = 'desk' AND revoked_at IS NULL").execute(&db).await.unwrap();
        assert!(b.reveal_in(&db, Some("billing"), "crm.token").await.unwrap_err().to_string().contains("decrypt failed"));
        // the app's credentials go with it
        assert_eq!(b.forget_app(&db, "billing").await.unwrap(), 1);
        assert_eq!(b.forget_app(&db, "desk").await.unwrap(), 1, "the revoked version");
        assert_eq!(b.reveal(&db, "crm.token").await.unwrap(), "deployment-value", "the deployment's stays");
    }

    /// A key wrapper wraps the data keys of the scopes it covers, each row recording which key; the master key keeps the
    /// others; rows of either kind open whatever the setting; a rewrap moves rows to the newest key version, and those the
    /// master key wrapped in a covered scope under the wrapper; without the wrapper, a row it wrapped says so.
    #[tokio::test]
    async fn a_key_wrapper_wraps_the_scopes_it_covers() {
        let Some(db) = crate::db::test_db().await else { eprintln!("CORI_STORE_DATABASE_URL not set: skipped"); return };
        let plain = Broker::from_key([5; 32]);
        plain.put_in(&db, Some("desk"), "old", "token", "before the wrapper", "admin").await.unwrap();
        let w = Arc::new(PerApp::default());
        let b = Broker::from_key([5; 32]).with_wrapper(Some(w.clone()));
        b.put_in(&db, Some("desk"), "crm.token", "token", "desk-value", "owner").await.unwrap();
        b.put(&db, "llm.main", "llm_key", "deployment-value", "admin").await.unwrap();
        let key_by = |name: &'static str| { let db = db.clone(); async move { sqlx::query_scalar::<_, String>("SELECT key_by FROM secrets WHERE name = $1 AND revoked_at IS NULL").bind(name).fetch_one(&db).await.unwrap() } };
        assert_eq!(key_by("crm.token").await, "per-app:desk:1");
        assert_eq!(key_by("llm.main").await, "master", "the deployment's scope is not covered");
        assert_eq!(b.reveal_in(&db, Some("desk"), "crm.token").await.unwrap(), "desk-value");
        assert_eq!(b.reveal_in(&db, Some("desk"), "old").await.unwrap(), "before the wrapper", "a master-wrapped row still opens");
        assert_eq!(b.prepare_app(&db, "desk").await.unwrap().as_deref(), Some("key-desk"));
        assert!(plain.reveal_in(&db, Some("desk"), "crm.token").await.unwrap_err().to_string().contains("no key wrapper"));
        // the key rotates: a rewrap moves every covered row to version 2, the master-wrapped one included
        w.version.store(1, std::sync::atomic::Ordering::SeqCst);
        assert_eq!(b.rewrap(&db).await.unwrap(), 2);
        assert_eq!((key_by("crm.token").await, key_by("old").await, key_by("llm.main").await), ("per-app:desk:2".into(), "per-app:desk:2".into(), "master".into()));
        assert_eq!(b.rewrap(&db).await.unwrap(), 0, "nothing left to move");
        assert_eq!(b.reveal_in(&db, Some("desk"), "old").await.unwrap(), "before the wrapper");
        // an app whose key is withdrawn does not hold the others back: its rows stay, the others move on
        b.put_in(&db, Some("billing"), "erp.token", "token", "billing-value", "owner").await.unwrap();
        w.withdrawn.lock().unwrap().push("desk".into());
        w.version.store(2, std::sync::atomic::Ordering::SeqCst);
        assert_eq!(b.rewrap(&db).await.unwrap(), 1);
        assert_eq!((key_by("crm.token").await, key_by("erp.token").await), ("per-app:desk:2".into(), "per-app:billing:3".into()));
        w.withdrawn.lock().unwrap().clear();
        b.forget_app(&db, "desk").await.unwrap();
        assert_eq!(*w.forgotten.lock().unwrap(), ["desk"]);
    }

    /// The SDKs open the same sealed token: sdks/testdata holds it (the envelope's `principal_token`) and its key.
    #[test]
    fn a_native_token_is_sealed_as_the_sdks_open_it() {
        let testdata = |f: &str| std::fs::read_to_string(format!("{}/../../sdks/testdata/{f}", env!("CARGO_MANIFEST_DIR"))).unwrap();
        let key: [u8; 32] = hex::decode(testdata("connector_key.hex").trim()).unwrap().try_into().unwrap();
        let golden: serde_json::Value = serde_json::from_str(&testdata("native_envelope.json")).unwrap();
        let sealed = seal_native(&key, "refund_request-0001", "eyJhbGciOiJSUzI1NiJ9.e30.c2ln", std::array::from_fn(|i| 0xa0 + i as u8)).unwrap();
        assert_eq!(sealed, golden["cori"]["principal_token"].as_str().unwrap());
        assert_eq!(open_native(&key, "refund_request-0001", &sealed).unwrap(), "eyJhbGciOiJSUzI1NiJ9.e30.c2ln");
        assert!(open_native(&key, "refund_request-0002", &sealed).is_err(), "a sealed token opens for its own run only");
        let mut other = key;
        other[0] ^= 1;
        assert!(open_native(&other, "refund_request-0001", &sealed).is_err(), "nor with another key");
        let b = Broker::from_key([7; 32]);
        assert_ne!(b.connector_key("billing"), b.connector_key("payroll"), "each connector has a key of its own");
        assert_eq!(open_native(&b.connector_key("billing"), "r1", &b.seal_for_connector("billing", "r1", "tok").unwrap()).unwrap(), "tok");
    }
}
