// SPDX-License-Identifier: Apache-2.0
//! The KV engine (`CORI_VAULT_ENGINE=kv`), for a Vault without Transit: one master key per app, 32 random bytes in Vault's
//! KV engine (version 2) at `<mount>/<path>/apps/<app>`, field `key` (base64, or hex). Cori reads it with its Vault login
//! and wraps the app's data keys with it itself (AES-256-GCM, the secret's scope and name as associated data): Vault never
//! sees a data key, and Cori keeps an app's key in memory `CORI_VAULT_CACHE_SECS` at most before reading it again, so that
//! a read withdrawn from its policy cuts the app off.
//!
//! - Cori makes an app's key with the app (`CORI_VAULT_CREATE_KEYS`), with check-and-set 0: it never overwrites a key, and
//!   its policy needs `create` and `read`, never `update`. Otherwise the organization makes it on `app.created`.
//! - A new version of the secret is a rotation: rows record the version that wrapped them (`vault-kv:<mount>/<path>:<v>`),
//!   the leading replica's rewrap moves them to the newest, and the organization may then delete or destroy the old ones.
use super::*;
use aes_gcm::{Aes256Gcm, KeyInit, aead::Aead};

/// What a row's `key_by` starts with: `vault-kv:<mount>/<path>:<version>`.
pub(crate) const KEY_BY: &str = "vault-kv:";

/// The key wrapper over Vault's KV engine.
pub struct Kv {
    s: Settings,
    c: Arc<Client>,
    /// The apps' keys read: (path, version) → (key, when it was read).
    keys: Cache<(String, u32)>,
    /// The newest version of each key, and when it was read: trusted as long as a key is (`Settings::cache`, at most a
    /// minute), so that a rotation in Vault is seen by the next wrap and rewrap.
    latest: Mutex<HashMap<String, (u32, Instant)>>,
}

/// An app's key as its secret holds it: 32 bytes, in base64 (`openssl rand -base64 32`) or hex (`openssl rand -hex 32`).
pub fn parse_key(v: &str) -> Result<[u8; 32]> {
    let v = v.trim();
    let bytes = if v.len() == 64 && v.chars().all(|c| c.is_ascii_hexdigit()) { hex::decode(v)? } else { unb64(v)? };
    bytes.try_into().map_err(|b: Vec<u8>| anyhow!("{} bytes, not 32", b.len()))
}

/// `key` wraps `plain` (a data key): base64 of nonce ‖ ciphertext ‖ tag, `aad` bound to it.
fn seal(key: &[u8; 32], plain: &[u8], aad: &str) -> Result<String> {
    let nonce: [u8; 12] = rand::random();
    let ct = Aes256Gcm::new(key.into()).encrypt(&aes_gcm::Nonce::from(nonce), aes_gcm::aead::Payload { msg: plain, aad: aad.as_bytes() }).map_err(|_| anyhow!("encrypt"))?;
    let mut out = nonce.to_vec();
    out.extend(ct);
    Ok(b64(&out))
}

/// What `seal` wrapped, with the same key and associated data.
fn open(key: &[u8; 32], wrapped: &str, aad: &str) -> Result<[u8; 32]> {
    let raw = unb64(wrapped)?;
    if raw.len() < 12 + 16 { bail!("a wrapped data key too short"); }
    let (nonce, ct) = raw.split_at(12);
    let nonce: [u8; 12] = nonce.try_into().map_err(|_| anyhow!("bad nonce"))?;
    let plain = Aes256Gcm::new(key.into()).decrypt(&aes_gcm::Nonce::from(nonce), aes_gcm::aead::Payload { msg: ct, aad: aad.as_bytes() })
        .map_err(|_| anyhow!("the data key does not open with this key (another key at that version, or a row moved to another secret)"))?;
    plain.try_into().map_err(|_| anyhow!("a data key of the wrong size"))
}

impl Kv {
    /// The engine with a login of its own.
    pub fn new(s: Settings) -> Self { let c = Arc::new(Client::new(s.clone())); Self::with_client(s, c) }

    pub fn with_client(s: Settings, c: Arc<Client>) -> Self { Self { s, c, keys: Mutex::new(HashMap::new()), latest: Mutex::new(HashMap::new()) } }

    /// The path under the mount of the key that wraps `scope`'s data keys: the app's own, or the deployment's (when
    /// configured).
    pub fn key_path(&self, scope: &SecretScope) -> Result<String> {
        match (&scope.app, &scope.realm) {
            (Some(app), _) if !plain_path(app) || app.contains('/') => bail!("the app id `{app}` cannot name a path in Vault"),
            (Some(app), None) => Ok(format!("{}/apps/{app}", self.s.kv_path)),
            (Some(app), Some(realm)) => Ok(format!("{}/realms/{}/apps/{app}", self.s.kv_path, realm.as_str())),
            (None, _) => self.s.deployment_key.clone().ok_or_else(|| anyhow!("the deployment's secrets are not under a Vault key (CORI_VAULT_DEPLOYMENT_KEY)")),
        }
    }

    /// How a key is named to the organization (`app.created`'s `secrets_key`, a row's `key_by`): `<mount>/<path>`, as
    /// `vault kv get` takes it.
    pub fn name(&self, path: &str) -> String { format!("{}/{path}", self.s.kv_mount) }

    /// A refusal of a call on the key at `path` in words that say what to fix.
    fn explain(&self, path: &str, e: anyhow::Error) -> anyhow::Error {
        let (name, mount) = (self.name(path), &self.s.kv_mount);
        match status_of(&e) {
            Some(403) if self.s.create_keys => anyhow!("{e} (key `{name}`: Cori's policy needs create and read on {mount}/data/{path})"),
            Some(403) => anyhow!("{e} (key `{name}`: Cori's policy needs read on {mount}/data/{path})"),
            _ => e,
        }
    }

    /// Version `version` of the key at `path` (None: the newest), with its number; None when there is none: never made,
    /// or that version deleted or destroyed.
    async fn read(&self, path: &str, version: Option<u32>) -> Result<Option<(u32, [u8; 32])>> {
        let q = version.map(|v| format!("?version={v}")).unwrap_or_default();
        let v = match self.c.call(reqwest::Method::GET, &format!("{}/data/{path}{q}", self.s.kv_mount), None).await {
            Ok(v) => v,
            Err(e) if status_of(&e) == Some(404) => return Ok(None),
            Err(e) => return Err(self.explain(path, e)),
        };
        let name = self.name(path);
        let found = v["data"]["metadata"]["version"].as_u64().and_then(|n| u32::try_from(n).ok())
            .ok_or_else(|| anyhow!("`{name}` was answered without a version: `{0}` must be a KV engine of version 2 (vault secrets enable -version=2 -path={0} kv)", self.s.kv_mount))?;
        let raw = v["data"]["data"]["key"].as_str().ok_or_else(|| anyhow!("`{name}` (version {found}) has no field `key`"))?;
        let key = parse_key(raw).map_err(|e| anyhow!("the field `key` of `{name}` (version {found}) is not a key of 32 bytes in base64 or hex: {e:#}"))?;
        Ok(Some((found, key)))
    }

    /// Makes the key at `path`: 32 random bytes, written only while the path has none (check-and-set 0), so that two
    /// replicas never make two keys: the one that loses the race reads the other's.
    async fn create(&self, path: &str) -> Result<()> {
        let key: [u8; 32] = rand::random();
        let body = json!({ "options": { "cas": 0 }, "data": { "key": b64(&key) } });
        match self.c.call(reqwest::Method::POST, &format!("{}/data/{path}", self.s.kv_mount), Some(&body)).await {
            Ok(_) => { tracing::info!(key = %self.name(path), "an app's key made in Vault"); Ok(()) }
            // made meanwhile: the check-and-set refuses the write, or the policy does (no `update` once it exists)
            Err(e) if matches!(status_of(&e), Some(400 | 403)) && self.read(path, None).await.ok().flatten().is_some() => Ok(()),
            Err(e) => Err(self.explain(path, e)),
        }
    }

    fn cached(&self, path: &str, version: u32) -> Option<[u8; 32]> {
        if self.s.cache.is_zero() { return None; }
        let mut c = self.keys.lock().unwrap();
        let now = Instant::now();
        c.retain(|_, (_, at)| now.duration_since(*at) < self.s.cache);
        c.get(&(path.to_string(), version)).map(|(k, _)| *k)
    }

    fn remember(&self, path: &str, version: u32, key: [u8; 32]) {
        if self.s.cache.is_zero() { return; }
        self.keys.lock().unwrap().insert((path.to_string(), version), (key, Instant::now()));
    }

    /// The newest version of the key at `path`, and the key: as read within `Settings::cache` (a minute at most), else read
    /// again. With `make`, a key that does not exist yet is made first.
    async fn current(&self, path: &str, make: bool) -> Result<(u32, [u8; 32])> {
        let fresh_for = self.s.cache.min(Duration::from_secs(60));
        let known = self.latest.lock().unwrap().get(path).copied();
        if let Some((v, at)) = known && at.elapsed() < fresh_for && let Some(k) = self.cached(path, v) { return Ok((v, k)); }
        let found = match self.read(path, None).await? {
            Some(f) => f,
            None if make => { self.create(path).await?; self.read(path, None).await?.ok_or_else(|| anyhow!("`{}` not found after its creation", self.name(path)))? }
            None if self.s.create_keys => bail!("the key `{}` is not in Vault any more (deleted)", self.name(path)),
            None => bail!("the key `{0}` does not exist yet: Cori does not create keys (CORI_VAULT_CREATE_KEYS=false), so the organization creates it when it hears `app.created` (vault kv put {0} key=\"$(openssl rand -base64 32)\")", self.name(path)),
        };
        self.latest.lock().unwrap().insert(path.to_string(), (found.0, Instant::now()));
        self.remember(path, found.0, found.1);
        Ok(found)
    }

    /// Version `version` of the key at `path`.
    async fn at(&self, path: &str, version: u32) -> Result<[u8; 32]> {
        if let Some(k) = self.cached(path, version) { return Ok(k); }
        let (_, key) = self.read(path, Some(version)).await?
            .ok_or_else(|| anyhow!("version {version} of `{}` is deleted or destroyed: the data keys it wrapped no longer open", self.name(path)))?;
        self.remember(path, version, key);
        Ok(key)
    }

    /// The key and version a row's `key_by` names (`vault-kv:<mount>/<path>:<version>`), refused unless it is `scope`'s
    /// own: (its path, the version).
    fn recorded(&self, scope: &SecretScope, key_by: &str) -> Result<(String, u32)> {
        let (name, version) = key_by.strip_prefix(KEY_BY).and_then(|r| r.rsplit_once(':')).and_then(|(n, v)| Some((n, v.parse::<u32>().ok()?)))
            .ok_or_else(|| anyhow!("`{key_by}` is not a key of Vault's KV engine"))?;
        let own = self.key_path(scope)?;
        if name != self.name(&own) { bail!("the data key is wrapped by `{name}`, not by `{}`, the key of its scope", self.name(&own)); }
        Ok((own, version))
    }

    /// `cori preflight`, once logged in: the preflight key (`<path>/preflight`) read, made first when Cori makes keys.
    pub async fn preflight(&self) -> Check {
        let path = format!("{}/preflight", self.s.kv_path);
        let round = async {
            let (version, key) = self.current(&path, self.s.create_keys).await?;
            let dek: [u8; 32] = rand::random();
            if open(&key, &seal(&key, &dek, "cori:preflight")?, "cori:preflight")? != dek { bail!("the data key did not come back"); }
            Ok::<_, anyhow::Error>(version)
        };
        match round.await {
            Ok(v) => Check::ok("vault kv", format!("key `{}` (version {v}) read; it wraps and unwraps", self.name(&path))),
            Err(e) => Check::failed("vault kv", format!("{e:#}"), format!("a KV engine of version 2 at `{0}`; Cori's policy needs read (and create, when Cori makes the keys) on {0}/data/{1}/apps/* and {0}/data/{1}/preflight", self.s.kv_mount, self.s.kv_path)),
        }
    }
}

impl KeyWrapper for Kv {
    fn id(&self) -> &'static str { "vault-kv" }

    fn covers(&self, scope: &SecretScope) -> bool { scope.app.is_some() || self.s.deployment_key.is_some() }

    fn prepare<'a>(&'a self, scope: &'a SecretScope) -> KeyFuture<'a, Option<String>> {
        Box::pin(async move {
            // made with the app when Cori makes keys; otherwise the organization makes it on `app.created`, which names it
            let path = self.key_path(scope)?;
            if self.s.create_keys { self.current(&path, true).await?; }
            Ok(Some(self.name(&path)))
        })
    }

    fn wrap<'a>(&'a self, scope: &'a SecretScope, key: &'a [u8; 32], aad: &'a str) -> KeyFuture<'a, Wrapped> {
        Box::pin(async move {
            let path = self.key_path(scope)?;
            let (version, k) = self.current(&path, self.s.create_keys).await?;
            Ok(Wrapped { key_by: format!("{KEY_BY}{}:{version}", self.name(&path)), wrapped: seal(&k, key, aad)? })
        })
    }

    fn unwrap<'a>(&'a self, scope: &'a SecretScope, key_by: &'a str, wrapped: &'a str, aad: &'a str) -> KeyFuture<'a, [u8; 32]> {
        Box::pin(async move {
            let (path, version) = self.recorded(scope, key_by)?;
            open(&self.at(&path, version).await?, wrapped, aad)
        })
    }

    fn rewrap<'a>(&'a self, scope: &'a SecretScope, key_by: &'a str, wrapped: &'a str, aad: &'a str) -> KeyFuture<'a, Option<Wrapped>> {
        Box::pin(async move {
            let (path, version) = self.recorded(scope, key_by)?;
            let (latest, k) = self.current(&path, false).await?;
            if version >= latest { return Ok(None); }
            let dek = open(&self.at(&path, version).await?, wrapped, aad)?;
            Ok(Some(Wrapped { key_by: format!("{KEY_BY}{}:{latest}", self.name(&path)), wrapped: seal(&k, &dek, aad)? }))
        })
    }

    fn forget<'a>(&'a self, scope: &'a SecretScope) -> KeyFuture<'a, ()> {
        Box::pin(async move {
            let path = self.key_path(scope)?;
            self.keys.lock().unwrap().retain(|(p, _), _| p != &path);
            self.latest.lock().unwrap().remove(&path);
            if !self.s.delete_keys { tracing::info!(key = %self.name(&path), "the app is deleted, its key stays in Vault (CORI_VAULT_DELETE_KEYS=false)"); return Ok(()); }
            // every version of the key and its metadata, for good
            self.c.call(reqwest::Method::DELETE, &format!("{}/metadata/{path}", self.s.kv_mount), None).await.map_err(|e| anyhow!("deleting `{}`: {e:#}", self.name(&path)))?;
            Ok(())
        })
    }
}
