//! Token store backends.
//!
//! Two implementations:
//!
//! - [`KeychainStore`] — uses the OS keychain via the [`keyring`] crate
//!   (macOS Keychain, Windows Credential Manager, libsecret on Linux).
//!   This is the default on every supported platform.
//! - [`FileStore`] — JSON at `~/.cori/credentials/secrets.json` with
//!   `0600` mode. Used as a fallback on machines without a usable
//!   keychain (typically headless Linux without libsecret installed).
//!
//! In both cases a non-secret metadata index lives at
//! `~/.cori/credentials/index.json` so `cori status` / `cori check` can
//! answer "is `notion` ready for user `jean`?" without touching the
//! keychain.
//!
//! ## v1 caveats
//!
//! The [`FileStore`] currently writes **plaintext JSON with file mode
//! `0600`**. The migration plan calls for ChaCha20Poly1305/age
//! encryption keyed by an OS-derived secret; that is deferred to a
//! follow-up so this phase can ship the full PKCE flow. The
//! `EncryptedFileStore` rename will land alongside the real encryption.

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::types::{Owner, Token, TokenKey};

const KEYRING_SERVICE: &str = "cori";
const SECRETS_FILE: &str = "secrets.json";
const INDEX_FILE: &str = "index.json";

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("keychain unavailable: {0}")]
    KeychainUnavailable(String),
    #[error("keychain error: {0}")]
    Keychain(String),
    #[error("credential file I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("credential file is malformed: {0}")]
    Malformed(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, StoreError>;

/// Non-secret metadata about a stored token.
///
/// Written to `index.json` so machine-level UIs (`cori status`) can be
/// fast without unlocking the keychain.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenMetadata {
    pub server_id: String,
    pub owner: Owner,
    pub token_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    pub auth_kind: super::types::AuthKind,
    pub stored_at: DateTime<Utc>,
}

impl From<(&TokenKey, &Token)> for TokenMetadata {
    fn from((key, tok): (&TokenKey, &Token)) -> Self {
        Self {
            server_id: key.server_id.clone(),
            owner: key.owner.clone(),
            token_type: tok.token_type.clone(),
            expires_at: tok.expires_at,
            scope: tok.scope.clone(),
            auth_kind: tok.auth_kind,
            stored_at: Utc::now(),
        }
    }
}

/// Trait implemented by every token-store backend.
///
/// Implementations must be safe for concurrent use across threads —
/// `mcp::run` and `cori login` may both touch the store from different
/// activity / CLI threads.
pub trait TokenStore: Send + Sync {
    fn get(&self, key: &TokenKey) -> Result<Option<Token>>;
    fn put(&self, key: &TokenKey, token: &Token) -> Result<()>;
    fn delete(&self, key: &TokenKey) -> Result<()>;
    /// List the metadata of every token this store knows about. Drives
    /// `cori status` and the OAuth-aware `CapabilityReport.authed` bit.
    fn list_metadata(&self) -> Result<Vec<TokenMetadata>>;
}

// ---------------------------------------------------------------------------
// Default-store construction
// ---------------------------------------------------------------------------

/// Build the default store for this machine. Prefers the keychain;
/// falls back to a file store under `credentials_dir` when the keychain
/// cannot be probed (typically headless Linux).
///
/// `credentials_dir` is `~/.cori/credentials/`. It is created on first
/// write; it is also the home of `index.json`.
pub fn default_store(credentials_dir: PathBuf) -> Arc<dyn TokenStore> {
    if keychain_available() {
        Arc::new(KeychainStore::new(credentials_dir))
    } else {
        tracing::info!(
            "OS keychain unavailable; falling back to file token store at {}",
            credentials_dir.display()
        );
        Arc::new(FileStore::new(credentials_dir))
    }
}

fn keychain_available() -> bool {
    // The cheapest probe: try opening an entry. Some platforms (Linux
    // without libsecret) will fail at entry construction.
    keyring::Entry::new(KEYRING_SERVICE, "__probe__").is_ok()
}

// ---------------------------------------------------------------------------
// KeychainStore
// ---------------------------------------------------------------------------

pub struct KeychainStore {
    credentials_dir: PathBuf,
}

impl KeychainStore {
    pub fn new(credentials_dir: PathBuf) -> Self {
        Self { credentials_dir }
    }

    fn entry(key: &TokenKey) -> Result<keyring::Entry> {
        keyring::Entry::new(KEYRING_SERVICE, &key.as_storage_key())
            .map_err(|e| StoreError::Keychain(format!("{e}")))
    }
}

impl TokenStore for KeychainStore {
    fn get(&self, key: &TokenKey) -> Result<Option<Token>> {
        let entry = Self::entry(key)?;
        match entry.get_password() {
            Ok(s) => Ok(Some(serde_json::from_str(&s)?)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(StoreError::Keychain(format!("{e}"))),
        }
    }

    fn put(&self, key: &TokenKey, token: &Token) -> Result<()> {
        let entry = Self::entry(key)?;
        let blob = serde_json::to_string(token)?;
        entry
            .set_password(&blob)
            .map_err(|e| StoreError::Keychain(format!("{e}")))?;
        update_index(&self.credentials_dir, key, Some(token))?;
        Ok(())
    }

    fn delete(&self, key: &TokenKey) -> Result<()> {
        let entry = Self::entry(key)?;
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => {}
            Err(e) => return Err(StoreError::Keychain(format!("{e}"))),
        }
        update_index(&self.credentials_dir, key, None)?;
        Ok(())
    }

    fn list_metadata(&self) -> Result<Vec<TokenMetadata>> {
        read_index(&self.credentials_dir)
    }
}

// ---------------------------------------------------------------------------
// FileStore (fallback)
// ---------------------------------------------------------------------------

pub struct FileStore {
    credentials_dir: PathBuf,
}

impl FileStore {
    pub fn new(credentials_dir: PathBuf) -> Self {
        Self { credentials_dir }
    }

    fn path(&self) -> PathBuf {
        self.credentials_dir.join(SECRETS_FILE)
    }

    fn read(&self) -> Result<BTreeMap<String, Token>> {
        let p = self.path();
        if !p.exists() {
            return Ok(BTreeMap::new());
        }
        let raw = fs::read_to_string(&p)?;
        if raw.trim().is_empty() {
            return Ok(BTreeMap::new());
        }
        Ok(serde_json::from_str(&raw)?)
    }

    fn write(&self, map: &BTreeMap<String, Token>) -> Result<()> {
        ensure_credentials_dir(&self.credentials_dir)?;
        let p = self.path();
        let bytes = serde_json::to_vec_pretty(map)?;
        write_secret_file(&p, &bytes)?;
        Ok(())
    }
}

impl TokenStore for FileStore {
    fn get(&self, key: &TokenKey) -> Result<Option<Token>> {
        let map = self.read()?;
        Ok(map.get(&key.as_storage_key()).cloned())
    }

    fn put(&self, key: &TokenKey, token: &Token) -> Result<()> {
        let mut map = self.read()?;
        map.insert(key.as_storage_key(), token.clone());
        self.write(&map)?;
        update_index(&self.credentials_dir, key, Some(token))?;
        Ok(())
    }

    fn delete(&self, key: &TokenKey) -> Result<()> {
        let mut map = self.read()?;
        map.remove(&key.as_storage_key());
        self.write(&map)?;
        update_index(&self.credentials_dir, key, None)?;
        Ok(())
    }

    fn list_metadata(&self) -> Result<Vec<TokenMetadata>> {
        read_index(&self.credentials_dir)
    }
}

// ---------------------------------------------------------------------------
// Index (non-secret metadata)
// ---------------------------------------------------------------------------

fn ensure_credentials_dir(dir: &Path) -> Result<()> {
    if !dir.exists() {
        fs::create_dir_all(dir)?;
    }
    Ok(())
}

fn index_path(dir: &Path) -> PathBuf {
    dir.join(INDEX_FILE)
}

fn read_index(dir: &Path) -> Result<Vec<TokenMetadata>> {
    let p = index_path(dir);
    if !p.exists() {
        return Ok(Vec::new());
    }
    let raw = fs::read_to_string(&p)?;
    if raw.trim().is_empty() {
        return Ok(Vec::new());
    }
    Ok(serde_json::from_str(&raw)?)
}

fn write_index(dir: &Path, entries: &[TokenMetadata]) -> Result<()> {
    ensure_credentials_dir(dir)?;
    let p = index_path(dir);
    let bytes = serde_json::to_vec_pretty(entries)?;
    write_secret_file(&p, &bytes)?;
    Ok(())
}

fn update_index(dir: &Path, key: &TokenKey, token: Option<&Token>) -> Result<()> {
    let mut entries = read_index(dir)?;
    entries.retain(|e| !(e.server_id == key.server_id && e.owner == key.owner));
    if let Some(t) = token {
        entries.push(TokenMetadata::from((key, t)));
    }
    write_index(dir, &entries)
}

/// Write a file atomically with `0600` permissions on Unix.
fn write_secret_file(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    {
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts.open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use tempfile::TempDir;

    use crate::oauth::types::AuthKind;

    fn sample_token() -> Token {
        Token {
            access_token: "secret-abc".to_string(),
            refresh_token: Some("refresh-xyz".to_string()),
            token_type: "Bearer".to_string(),
            expires_at: Some(Utc::now() + Duration::hours(1)),
            scope: Some("read write".to_string()),
            auth_kind: AuthKind::Pkce,
        }
    }

    #[test]
    fn file_store_round_trip() {
        let dir = TempDir::new().unwrap();
        let store = FileStore::new(dir.path().to_path_buf());
        let key = TokenKey::new("notion", Owner::User("jean".into()));

        assert!(store.get(&key).unwrap().is_none());
        store.put(&key, &sample_token()).unwrap();
        let got = store.get(&key).unwrap().unwrap();
        assert_eq!(got.access_token, "secret-abc");

        let meta = store.list_metadata().unwrap();
        assert_eq!(meta.len(), 1);
        assert_eq!(meta[0].server_id, "notion");

        store.delete(&key).unwrap();
        assert!(store.get(&key).unwrap().is_none());
        assert!(store.list_metadata().unwrap().is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn file_store_writes_0600() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        let store = FileStore::new(dir.path().to_path_buf());
        let key = TokenKey::new("notion", Owner::User("jean".into()));
        store.put(&key, &sample_token()).unwrap();
        let perms = fs::metadata(dir.path().join(SECRETS_FILE))
            .unwrap()
            .permissions();
        assert_eq!(perms.mode() & 0o777, 0o600);
    }
}
