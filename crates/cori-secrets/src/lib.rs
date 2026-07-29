//! Shared secret store for Cori.
//!
//! One store, two writers: the `cori` CLI (`cori login anthropic`) and the
//! desktop app's provider settings both go through this crate, so a key set
//! in either place is immediately visible to the other. Secrets live in the
//! OS keychain (macOS Keychain, Windows Credential Manager, libsecret on
//! Linux) under the service [`KEYRING_SERVICE`], with accounts like
//! `llm/anthropic`. Machines without a usable keychain (headless Linux,
//! CI) fall back to a `0600` JSON file under `~/.cori/credentials/`.
//!
//! A non-secret index (`secrets-index.json`) records which accounts are
//! configured and when, so `cori status` / `cori check` and the desktop
//! settings page can answer "is anthropic ready?" without unlocking the
//! keychain.
//!
//! Backend selection can be forced with `CORI_SECRETS_BACKEND=file|keychain`
//! (tests use `file` together with `CORI_HOME` pointing at a tempdir).

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Keychain service shared by every Cori binary.
pub const KEYRING_SERVICE: &str = "cori";

const SECRETS_FILE: &str = "llm-secrets.json";
const INDEX_FILE: &str = "secrets-index.json";

/// LLM providers Cori knows how to broker.
pub const LLM_PROVIDERS: [&str; 3] = ["openai", "anthropic", "gemini"];

/// Keychain account for a provider's API key (`llm/anthropic`).
pub fn llm_account(provider: &str) -> String {
    format!("llm/{provider}")
}

#[derive(Debug, Error)]
pub enum SecretError {
    #[error("keychain error: {0}")]
    Keychain(String),
    #[error("secret file I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("secret file is malformed: {0}")]
    Malformed(#[from] serde_json::Error),
    #[error("could not resolve home directory ($HOME unset?)")]
    NoHome,
}

pub type Result<T> = std::result::Result<T, SecretError>;

/// Non-secret record of a configured account, kept in the index.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecretMetadata {
    pub account: String,
    pub stored_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Backend {
    Keychain,
    File,
}

pub struct SecretStore {
    credentials_dir: PathBuf,
    backend: Backend,
}

impl SecretStore {
    /// Open the default store for this machine: `~/.cori/credentials`
    /// (honouring `$CORI_HOME`), keychain backend when available.
    pub fn open_default() -> Result<Self> {
        Ok(Self::open_at(default_credentials_dir()?))
    }

    /// Open a store rooted at an explicit credentials directory.
    pub fn open_at(credentials_dir: PathBuf) -> Self {
        let backend = match std::env::var("CORI_SECRETS_BACKEND").as_deref() {
            Ok("file") => Backend::File,
            Ok("keychain") => Backend::Keychain,
            _ if keychain_available() => Backend::Keychain,
            _ => Backend::File,
        };
        Self {
            credentials_dir,
            backend,
        }
    }

    /// True when secrets go to the OS keychain (vs. the file fallback).
    pub fn uses_keychain(&self) -> bool {
        self.backend == Backend::Keychain
    }

    pub fn set(&self, account: &str, value: &str) -> Result<()> {
        match self.backend {
            Backend::Keychain => {
                entry(account)?
                    .set_password(value)
                    .map_err(|e| SecretError::Keychain(e.to_string()))?;
            }
            Backend::File => {
                let mut map = self.read_file()?;
                map.insert(account.to_string(), value.to_string());
                self.write_file(&map)?;
            }
        }
        self.update_index(account, true)
    }

    pub fn get(&self, account: &str) -> Result<Option<String>> {
        match self.backend {
            Backend::Keychain => match entry(account)?.get_password() {
                Ok(v) => Ok(Some(v)),
                Err(keyring::Error::NoEntry) => Ok(None),
                Err(e) => Err(SecretError::Keychain(e.to_string())),
            },
            Backend::File => Ok(self.read_file()?.get(account).cloned()),
        }
    }

    pub fn delete(&self, account: &str) -> Result<()> {
        match self.backend {
            Backend::Keychain => match entry(account)?.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => {}
                Err(e) => return Err(SecretError::Keychain(e.to_string())),
            },
            Backend::File => {
                let mut map = self.read_file()?;
                map.remove(account);
                self.write_file(&map)?;
            }
        }
        self.update_index(account, false)
    }

    /// Configured accounts, from the non-secret index only — never touches
    /// the keychain, so it is safe for status displays and hot paths.
    pub fn configured(&self) -> Result<Vec<SecretMetadata>> {
        self.read_index()
    }

    /// Index-only readiness check for one account.
    pub fn is_configured(&self, account: &str) -> bool {
        self.read_index()
            .map(|v| v.iter().any(|m| m.account == account))
            .unwrap_or(false)
    }

    /// LLM providers with a configured key, per the index.
    pub fn configured_llm_providers(&self) -> Vec<String> {
        let configured = self.read_index().unwrap_or_default();
        LLM_PROVIDERS
            .iter()
            .filter(|p| {
                let account = llm_account(p);
                configured.iter().any(|m| m.account == account)
            })
            .map(|p| p.to_string())
            .collect()
    }

    // -- file fallback ------------------------------------------------------

    fn secrets_path(&self) -> PathBuf {
        self.credentials_dir.join(SECRETS_FILE)
    }

    fn read_file(&self) -> Result<BTreeMap<String, String>> {
        let p = self.secrets_path();
        if !p.exists() {
            return Ok(BTreeMap::new());
        }
        let raw = fs::read_to_string(&p)?;
        if raw.trim().is_empty() {
            return Ok(BTreeMap::new());
        }
        Ok(serde_json::from_str(&raw)?)
    }

    fn write_file(&self, map: &BTreeMap<String, String>) -> Result<()> {
        let bytes = serde_json::to_vec_pretty(map)?;
        write_secret_file(&self.secrets_path(), &bytes)
    }

    // -- index --------------------------------------------------------------

    fn index_path(&self) -> PathBuf {
        self.credentials_dir.join(INDEX_FILE)
    }

    fn read_index(&self) -> Result<Vec<SecretMetadata>> {
        let p = self.index_path();
        if !p.exists() {
            return Ok(Vec::new());
        }
        let raw = fs::read_to_string(&p)?;
        if raw.trim().is_empty() {
            return Ok(Vec::new());
        }
        Ok(serde_json::from_str(&raw)?)
    }

    fn update_index(&self, account: &str, present: bool) -> Result<()> {
        let mut entries = self.read_index()?;
        entries.retain(|e| e.account != account);
        if present {
            entries.push(SecretMetadata {
                account: account.to_string(),
                stored_at: Utc::now(),
            });
        }
        let bytes = serde_json::to_vec_pretty(&entries)?;
        write_secret_file(&self.index_path(), &bytes)
    }
}

/// `~/.cori/credentials`, honouring `$CORI_HOME`.
pub fn default_credentials_dir() -> Result<PathBuf> {
    if let Ok(p) = std::env::var("CORI_HOME")
        && !p.is_empty()
    {
        return Ok(PathBuf::from(p).join("credentials"));
    }
    let home = dirs::home_dir().ok_or(SecretError::NoHome)?;
    Ok(home.join(".cori").join("credentials"))
}

fn entry(account: &str) -> Result<keyring::Entry> {
    keyring::Entry::new(KEYRING_SERVICE, account).map_err(|e| SecretError::Keychain(e.to_string()))
}

fn keychain_available() -> bool {
    keyring::Entry::new(KEYRING_SERVICE, "__probe__").is_ok()
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
    use tempfile::TempDir;

    fn file_store(dir: &TempDir) -> SecretStore {
        SecretStore {
            credentials_dir: dir.path().to_path_buf(),
            backend: Backend::File,
        }
    }

    #[test]
    fn file_round_trip_and_index() {
        let dir = TempDir::new().unwrap();
        let store = file_store(&dir);
        let account = llm_account("anthropic");

        assert!(store.get(&account).unwrap().is_none());
        assert!(!store.is_configured(&account));

        store.set(&account, "sk-ant-test").unwrap();
        assert_eq!(store.get(&account).unwrap().unwrap(), "sk-ant-test");
        assert!(store.is_configured(&account));
        assert_eq!(store.configured_llm_providers(), vec!["anthropic"]);

        store.delete(&account).unwrap();
        assert!(store.get(&account).unwrap().is_none());
        assert!(!store.is_configured(&account));
        assert!(store.configured_llm_providers().is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn file_backend_writes_0600() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        let store = file_store(&dir);
        store.set(&llm_account("openai"), "sk-test").unwrap();
        for f in [SECRETS_FILE, INDEX_FILE] {
            let perms = fs::metadata(dir.path().join(f)).unwrap().permissions();
            assert_eq!(perms.mode() & 0o777, 0o600, "{f} should be 0600");
        }
    }

    #[test]
    fn index_survives_unknown_accounts() {
        let dir = TempDir::new().unwrap();
        let store = file_store(&dir);
        store.set("llm/openai_compatible", "k").unwrap();
        assert!(store.configured_llm_providers().is_empty());
        assert!(store.is_configured("llm/openai_compatible"));
    }
}
