// SPDX-License-Identifier: Apache-2.0
//! The Transit engine (`CORI_VAULT_ENGINE=transit`, the default): one Transit key per app, which wraps and unwraps the
//! app's data keys in Vault and never leaves it.
use super::*;

/// What a row's `key_by` starts with: `vault-transit:<key>:<version>`.
pub(crate) const KEY_BY: &str = "vault-transit:";

/// The key wrapper over Vault Transit.
pub struct Transit {
    s: Settings,
    c: Arc<Client>,
    /// Unwrapped data keys: (key name, wrapped key) → (key, when it was unwrapped).
    cache: Cache<(String, String)>,
    /// The newest version of each key, learnt from what `encrypt` answered, and when: trusted as long as an unwrapped
    /// key is (`Settings::cache`, at most a minute), so that a rotation in Vault is seen by the next rewrap.
    latest: Mutex<HashMap<String, (u32, Instant)>>,
}

/// The version a Transit ciphertext names (`vault:v3:…` → 3).
pub(crate) fn version_of(ciphertext: &str) -> Result<u32> {
    ciphertext.strip_prefix("vault:v").and_then(|r| r.split_once(':')).and_then(|(v, _)| v.parse().ok()).ok_or_else(|| anyhow!("not a Transit ciphertext"))
}

impl Transit {
    /// The engine with a login of its own.
    pub fn new(s: Settings) -> Self { let c = Arc::new(Client::new(s.clone())); Self::with_client(s, c) }

    pub fn with_client(s: Settings, c: Arc<Client>) -> Self { Self { s, c, cache: Mutex::new(HashMap::new()), latest: Mutex::new(HashMap::new()) } }

    /// The key that wraps `scope`'s data keys: the app's own, or the deployment's (when configured).
    pub fn key_name(&self, scope: &SecretScope) -> Result<String> {
        match (&scope.app, &scope.realm) {
            (Some(app), None) => Ok(format!("{}-app-{app}", self.s.prefix)),
            (Some(app), Some(realm)) => Ok(format!("{}-realm-{}-app-{app}", self.s.prefix, realm.as_str())),
            (None, _) => self.s.deployment_key.clone().ok_or_else(|| anyhow!("the deployment's secrets are not under a Vault key (CORI_VAULT_DEPLOYMENT_KEY)")),
        }
    }

    /// A refusal of a call on key `name` in words that say what to fix.
    fn explain(&self, name: &str, e: anyhow::Error) -> anyhow::Error {
        match e.downcast_ref::<Refused>() {
            Some(r) if r.status == 403 && self.s.create_keys => anyhow!("{e} (Transit key `{name}`: Cori's policy needs create and update on {0}/encrypt/{name} and update on {0}/decrypt/{name})", self.s.mount),
            Some(r) if r.status == 403 => anyhow!("{e} (Transit key `{name}` may not exist yet: Cori does not create keys (CORI_VAULT_CREATE_KEYS=false), so the organization creates it when it hears `app.created`; otherwise Cori's policy needs update on {0}/encrypt/{name} and {0}/decrypt/{name})", self.s.mount),
            Some(r) if r.status == 400 && r.message.contains("not found") => anyhow!("{e} (Transit key `{name}` does not exist: create it in Vault)"),
            _ => e,
        }
    }

    /// Wraps `plain` with key `name`, which Transit creates first when it does not exist and Cori's policy grants `create`
    /// on its encrypt path (Transit's upsert).
    async fn encrypt(&self, name: &str, plain: &[u8], aad: &str) -> Result<String> {
        let body = json!({ "plaintext": b64(plain), "associated_data": b64(aad.as_bytes()), "type": "aes256-gcm96" });
        let v = self.c.call(reqwest::Method::POST, &format!("{}/encrypt/{name}", self.s.mount), Some(&body)).await.map_err(|e| self.explain(name, e))?;
        let ciphertext = v["data"]["ciphertext"].as_str().ok_or_else(|| anyhow!("Transit's encrypt answer has no ciphertext"))?.to_string();
        if let Ok(version) = version_of(&ciphertext) { self.latest.lock().unwrap().insert(name.to_string(), (version, Instant::now())); }
        Ok(ciphertext)
    }

    async fn decrypt(&self, name: &str, ciphertext: &str, aad: &str) -> Result<Vec<u8>> {
        let v = self.c.call(reqwest::Method::POST, &format!("{}/decrypt/{name}", self.s.mount), Some(&json!({ "ciphertext": ciphertext, "associated_data": b64(aad.as_bytes()) }))).await.map_err(|e| self.explain(name, e))?;
        unb64(v["data"]["plaintext"].as_str().ok_or_else(|| anyhow!("Transit's decrypt answer has no plaintext"))?)
    }

    /// The key named in a row's `key_by` (`vault-transit:<key>:<version>`), refused unless it is `scope`'s own.
    fn recorded_key(&self, scope: &SecretScope, key_by: &str) -> Result<String> {
        let name = key_by.strip_prefix(KEY_BY).and_then(|r| r.rsplit_once(':')).map(|(n, _)| n.to_string()).ok_or_else(|| anyhow!("`{key_by}` is not a Transit key"))?;
        let own = self.key_name(scope)?;
        if name != own { bail!("the data key is wrapped by `{name}`, not by `{own}`, the key of its scope"); }
        Ok(name)
    }

    fn cached(&self, name: &str, wrapped: &str) -> Option<[u8; 32]> {
        if self.s.cache.is_zero() { return None; }
        let mut c = self.cache.lock().unwrap();
        let now = Instant::now();
        c.retain(|_, (_, at)| now.duration_since(*at) < self.s.cache);
        c.get(&(name.to_string(), wrapped.to_string())).map(|(k, _)| *k)
    }

    fn remember(&self, name: &str, wrapped: &str, key: [u8; 32]) {
        if self.s.cache.is_zero() { return; }
        self.cache.lock().unwrap().insert((name.to_string(), wrapped.to_string()), (key, Instant::now()));
    }

    /// The newest version of `name`: what `encrypt` last answered, else one encryption of nothing to find out.
    async fn latest_version(&self, name: &str) -> Result<u32> {
        let fresh_for = self.s.cache.min(Duration::from_secs(60));
        if let Some((v, at)) = self.latest.lock().unwrap().get(name).copied() && at.elapsed() < fresh_for { return Ok(v); }
        version_of(&self.encrypt(name, &[0], "cori:latest-version").await?)
    }

    /// `cori preflight`, once logged in: a wrap and an unwrap with the preflight key (`<prefix>-preflight`).
    pub async fn preflight(&self) -> Check {
        let name = format!("{}-preflight", self.s.prefix);
        let key: [u8; 32] = rand::random();
        let round = async {
            let wrapped = self.encrypt(&name, &key, "cori:preflight").await?;
            let back = self.decrypt(&name, &wrapped, "cori:preflight").await?;
            if back != key { bail!("the key Transit gave back is not the one it wrapped"); }
            Ok::<_, anyhow::Error>(wrapped)
        };
        match round.await {
            Ok(w) => Check::ok("vault transit", format!("key `{name}` wraps and unwraps (version {})", version_of(&w).unwrap_or(0))),
            Err(e) => Check::failed("vault transit", format!("{e:#}"), format!("Cori's policy needs create and update on {0}/encrypt/{1}-* (or the keys made beforehand, and update) and update on {0}/decrypt/{1}-*", self.s.mount, self.s.prefix)),
        }
    }
}

impl KeyWrapper for Transit {
    fn id(&self) -> &'static str { "vault-transit" }

    fn covers(&self, scope: &SecretScope) -> bool { scope.app.is_some() || self.s.deployment_key.is_some() }

    fn prepare<'a>(&'a self, scope: &'a SecretScope) -> KeyFuture<'a, Option<String>> {
        Box::pin(async move {
            // the key exists once something was wrapped with it (Transit's upsert); without the right to create keys,
            // the organization creates it on `app.created`, which names it
            let name = self.key_name(scope)?;
            if self.s.create_keys { self.encrypt(&name, &[0], "cori:prepare").await?; }
            Ok(Some(name))
        })
    }

    fn wrap<'a>(&'a self, scope: &'a SecretScope, key: &'a [u8; 32], aad: &'a str) -> KeyFuture<'a, Wrapped> {
        Box::pin(async move {
            let name = self.key_name(scope)?;
            let wrapped = self.encrypt(&name, key, aad).await?;
            let version = version_of(&wrapped)?;
            self.remember(&name, &wrapped, *key);
            Ok(Wrapped { key_by: format!("{KEY_BY}{name}:{version}"), wrapped })
        })
    }

    fn unwrap<'a>(&'a self, scope: &'a SecretScope, key_by: &'a str, wrapped: &'a str, aad: &'a str) -> KeyFuture<'a, [u8; 32]> {
        Box::pin(async move {
            let name = self.recorded_key(scope, key_by)?;
            if let Some(k) = self.cached(&name, wrapped) { return Ok(k); }
            let key: [u8; 32] = self.decrypt(&name, wrapped, aad).await?.try_into().map_err(|_| anyhow!("Transit gave back a key of the wrong size"))?;
            self.remember(&name, wrapped, key);
            Ok(key)
        })
    }

    fn rewrap<'a>(&'a self, scope: &'a SecretScope, key_by: &'a str, wrapped: &'a str, aad: &'a str) -> KeyFuture<'a, Option<Wrapped>> {
        Box::pin(async move {
            let name = self.recorded_key(scope, key_by)?;
            if version_of(wrapped)? >= self.latest_version(&name).await? { return Ok(None); }
            // decrypt and encrypt again rather than Transit's rewrap, which takes no associated data
            let key: [u8; 32] = self.decrypt(&name, wrapped, aad).await?.try_into().map_err(|_| anyhow!("Transit gave back a key of the wrong size"))?;
            let fresh = self.encrypt(&name, &key, aad).await?;
            Ok(Some(Wrapped { key_by: format!("{KEY_BY}{name}:{}", version_of(&fresh)?), wrapped: fresh }))
        })
    }

    fn forget<'a>(&'a self, scope: &'a SecretScope) -> KeyFuture<'a, ()> {
        Box::pin(async move {
            let name = self.key_name(scope)?;
            self.cache.lock().unwrap().retain(|(n, _), _| n != &name);
            if !self.s.delete_keys { tracing::info!(key = %name, "the app is deleted, its Transit key stays (CORI_VAULT_DELETE_KEYS=false)"); return Ok(()); }
            self.c.call(reqwest::Method::POST, &format!("{}/keys/{name}/config", self.s.mount), Some(&json!({ "deletion_allowed": true }))).await.map_err(|e| anyhow!("allowing the deletion of `{name}`: {e:#}"))?;
            self.c.call(reqwest::Method::DELETE, &format!("{}/keys/{name}", self.s.mount), None).await.map_err(|e| anyhow!("deleting `{name}`: {e:#}"))?;
            Ok(())
        })
    }
}
