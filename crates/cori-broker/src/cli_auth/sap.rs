//! Owner- and SAP-target-bound credential support for `cori-sap`.
//!
//! SAP access tokens are Cori-managed secrets. They are keyed by the
//! requesting user plus a digest of the machine's canonical SAP origin and
//! `sap-client`. Data-plane dispatch carries the validated profile and token
//! together into the linked `cori_sap` library; neither value crosses a child
//! process boundary. Using the same profile snapshot for account lookup and
//! request construction closes the config-change race completely.

use std::path::{Path, PathBuf};

use sha2::{Digest as _, Sha256};
use thiserror::Error;

use super::{AuthState, CliAuthAdapter, WorkflowPolicy};

pub use crate::process::SAP_ACCESS_TOKEN_ENV as ACCESS_TOKEN_ENV;
pub const ALLOW_FILE_STORE_ENV: &str = "CORI_SAP_ALLOW_INSECURE_FILE_STORE";

const LOGIN_HINT: &str = "run: cori login cori-sap";
const ACCOUNT_PREFIX: &str = "sap/access-token/v1";

pub struct SapAdapter;

impl CliAuthAdapter for SapAdapter {
    fn binary(&self) -> &'static str {
        "cori-sap"
    }

    fn display_name(&self) -> &'static str {
        "SAP"
    }

    fn login_hint(&self) -> String {
        LOGIN_HINT.to_string()
    }

    fn check(&self) -> AuthState {
        // SAP readiness is owner- and target-specific. A context-free probe
        // must never turn a process-global environment variable into "ready".
        needs_reauth()
    }

    fn check_for_owner(&self, owner_id: &str, credentials_dir: &Path) -> AuthState {
        let target = match credential_target_for_owner(owner_id, credentials_dir) {
            Ok(target) => target,
            Err(SapCredentialError::Target(_) | SapCredentialError::InvalidCredentialsDir) => {
                return AuthState::NeedsReauth {
                    hint: "configure $CORI_HOME/sap.toml (or ~/.cori/sap.toml), then run: cori login cori-sap"
                        .to_string(),
                };
            }
            Err(_) => return needs_reauth(),
        };
        let store = match secure_store(credentials_dir) {
            Ok(store) => store,
            Err(_) => return credential_store_unavailable(),
        };
        // Human-frequency preflight intentionally unlocks the keychain. The
        // non-secret index can be stale, so readiness requires retrieving and
        // validating the actual token just as dispatch does.
        match store.get(&target.account) {
            Ok(Some(token)) if validate_access_token(&token).is_ok() => AuthState::Ok,
            Ok(_) => needs_reauth(),
            Err(_) => credential_store_unavailable(),
        }
    }

    fn workflow_policy(&self) -> WorkflowPolicy {
        WorkflowPolicy {
            allow_step_env: false,
            forced_env: &[],
        }
    }
}

#[derive(Debug, Error)]
pub enum SapCredentialError {
    #[error("SAP credentials directory must be an absolute path beneath Cori home")]
    InvalidCredentialsDir,
    #[error("SAP target configuration is unavailable: {0}")]
    Target(#[from] cori_sap::AdapterError),
    #[error("SAP credentials require an OS keychain; the plaintext file fallback is disabled")]
    InsecureStore,
    #[error("SAP credential store is unavailable: {0}")]
    Store(#[from] cori_secrets::SecretError),
    #[error("no SAP access token is stored for this user and SAP target")]
    MissingToken,
    #[error("the SAP access token is empty or invalid")]
    InvalidToken,
    #[error("SAP target configuration changed while credentials were being stored")]
    TargetChanged,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SapCredentialTarget {
    account: String,
    profile_name: String,
    credential_scope: String,
    scope_sha256: String,
}

impl SapCredentialTarget {
    pub fn profile_name(&self) -> &str {
        &self.profile_name
    }

    /// Canonical, non-secret target description suitable for confirmation.
    pub fn credential_scope(&self) -> &str {
        &self.credential_scope
    }

    pub fn scope_sha256(&self) -> &str {
        &self.scope_sha256
    }
}

pub struct SapCredential {
    pub(crate) access_token: String,
    pub(crate) profile: cori_sap::MachineProfile,
}

pub fn credential_target_for_owner(
    owner_id: &str,
    credentials_dir: &Path,
) -> Result<SapCredentialTarget, SapCredentialError> {
    credential_target_and_profile_for_owner(owner_id, credentials_dir).map(|(target, _)| target)
}

fn credential_target_and_profile_for_owner(
    owner_id: &str,
    credentials_dir: &Path,
) -> Result<(SapCredentialTarget, cori_sap::MachineProfile), SapCredentialError> {
    let cori_home = cori_home_for(credentials_dir)?;
    let profile = cori_sap::load_default_machine_profile_from_cori_home(&cori_home)?;
    let credential_scope = profile.credential_scope();
    let scope_sha256 = sha256_hex(credential_scope.as_bytes());
    let owner_prefix = owner_account_prefix(owner_id);
    let account = format!("{owner_prefix}/target-{scope_sha256}");
    Ok((
        SapCredentialTarget {
            account,
            profile_name: profile.name().to_string(),
            credential_scope,
            scope_sha256,
        },
        profile,
    ))
}

pub fn access_token_for_owner(
    owner_id: &str,
    credentials_dir: &Path,
) -> Result<SapCredential, SapCredentialError> {
    let (target, profile) = credential_target_and_profile_for_owner(owner_id, credentials_dir)?;
    let store = secure_store(credentials_dir)?;
    let token = store
        .get(&target.account)?
        .ok_or(SapCredentialError::MissingToken)?;
    let access_token = validate_access_token(&token)?;
    Ok(SapCredential {
        access_token,
        profile,
    })
}

/// Store a token only if the target the human confirmed is still current.
pub fn store_access_token(
    owner_id: &str,
    credentials_dir: &Path,
    confirmed_target: &SapCredentialTarget,
    token: &str,
) -> Result<(), SapCredentialError> {
    let current = credential_target_for_owner(owner_id, credentials_dir)?;
    if current != *confirmed_target {
        return Err(SapCredentialError::TargetChanged);
    }
    let token = validate_access_token(token)?;
    secure_store(credentials_dir)?.set(&confirmed_target.account, &token)?;
    Ok(())
}

/// Remove every target-bound SAP token for this owner, including credentials
/// orphaned by an intentional `sap.toml` tenant change.
pub fn delete_access_tokens_for_owner(
    owner_id: &str,
    credentials_dir: &Path,
) -> Result<usize, SapCredentialError> {
    let store = secure_store(credentials_dir)?;
    let owner_prefix = owner_account_prefix(owner_id);
    let accounts: Vec<String> = store
        .configured()?
        .into_iter()
        .filter(|metadata| metadata.account.starts_with(&owner_prefix))
        .map(|metadata| metadata.account)
        .collect();
    for account in &accounts {
        store.delete(account)?;
    }
    Ok(accounts.len())
}

fn owner_account_prefix(owner_id: &str) -> String {
    let owner_sha256 = sha256_hex(format!("user\0{owner_id}").as_bytes());
    format!("{ACCOUNT_PREFIX}/user-{owner_sha256}")
}

fn cori_home_for(credentials_dir: &Path) -> Result<PathBuf, SapCredentialError> {
    if !credentials_dir.is_absolute()
        || credentials_dir.file_name().and_then(|name| name.to_str()) != Some("credentials")
    {
        return Err(SapCredentialError::InvalidCredentialsDir);
    }
    credentials_dir
        .parent()
        .map(Path::to_path_buf)
        .ok_or(SapCredentialError::InvalidCredentialsDir)
}

fn secure_store(credentials_dir: &Path) -> Result<cori_secrets::SecretStore, SapCredentialError> {
    let store = cori_secrets::SecretStore::open_at(credentials_dir.to_path_buf());
    if store.uses_keychain() || insecure_file_store_allowed() {
        Ok(store)
    } else {
        Err(SapCredentialError::InsecureStore)
    }
}

fn insecure_file_store_allowed() -> bool {
    cfg!(debug_assertions) && std::env::var(ALLOW_FILE_STORE_ENV).as_deref() == Ok("1")
}

fn validate_access_token(value: &str) -> Result<String, SapCredentialError> {
    let value = value.trim();
    if value.is_empty() || value.len() > 64 * 1024 || value.contains(['\r', '\n']) {
        return Err(SapCredentialError::InvalidToken);
    }
    Ok(value.to_string())
}

pub fn scope_sha256(scope: &str) -> String {
    sha256_hex(scope.as_bytes())
}

fn sha256_hex(value: &[u8]) -> String {
    hex::encode(Sha256::digest(value))
}

fn needs_reauth() -> AuthState {
    AuthState::NeedsReauth {
        hint: LOGIN_HINT.to_string(),
    }
}

fn credential_store_unavailable() -> AuthState {
    AuthState::NeedsReauth {
        hint: "unlock the OS keychain, then run: cori login cori-sap".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adapter_is_owner_scoped_and_workflow_env_is_closed() {
        let adapter = SapAdapter;
        assert_eq!(adapter.display_name(), "SAP");
        assert_eq!(adapter.login_hint(), "run: cori login cori-sap");
        assert!(matches!(adapter.check(), AuthState::NeedsReauth { .. }));
        let policy = adapter.workflow_policy();
        assert!(!policy.allow_step_env);
        assert!(policy.forced_env.is_empty());
    }

    #[test]
    fn account_is_bound_to_owner_and_target_without_exposing_either() {
        let owner_a = sha256_hex(b"user\0alice");
        let owner_b = sha256_hex(b"user\0bob");
        let target_a = scope_sha256("sap-odata-v1|origin=https://a.example|sap_client=100");
        let target_b = scope_sha256("sap-odata-v1|origin=https://b.example|sap_client=100");
        let account_a = format!("{ACCOUNT_PREFIX}/user-{owner_a}/target-{target_a}");
        let account_other_owner = format!("{ACCOUNT_PREFIX}/user-{owner_b}/target-{target_a}");
        let account_other_target = format!("{ACCOUNT_PREFIX}/user-{owner_a}/target-{target_b}");

        assert_ne!(account_a, account_other_owner);
        assert_ne!(account_a, account_other_target);
        assert!(!account_a.contains("alice"));
        assert!(!account_a.contains("a.example"));
    }

    #[test]
    fn tokens_are_trimmed_and_never_accept_header_injection() {
        assert_eq!(
            validate_access_token("  token  ")
                .expect("valid token")
                .as_str(),
            "token"
        );
        assert!(validate_access_token("").is_err());
        assert!(validate_access_token("token\r\nX-Test: value").is_err());
    }

    #[test]
    fn adapter_does_not_claim_a_managed_installer() {
        assert!(crate::install::spec_for("cori-sap").is_none());
    }

    #[test]
    fn owner_probe_explains_missing_machine_configuration() {
        let temp = tempfile::tempdir().expect("temporary Cori home");
        let credentials_dir = temp.path().join("credentials");
        let state = SapAdapter.check_for_owner("alice", &credentials_dir);
        assert!(matches!(
            state,
            AuthState::NeedsReauth { hint }
                if hint.contains("sap.toml") && hint.contains("cori login cori-sap")
        ));
    }

    #[cfg(debug_assertions)]
    #[test]
    fn owner_probe_rejects_a_stale_index_when_the_secret_is_missing() {
        static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _lock = ENV_LOCK.lock().expect("SAP test environment lock");
        let previous_backend = std::env::var_os("CORI_SECRETS_BACKEND");
        let previous_override = std::env::var_os(ALLOW_FILE_STORE_ENV);
        struct RestoreEnv {
            backend: Option<std::ffi::OsString>,
            override_value: Option<std::ffi::OsString>,
        }
        impl Drop for RestoreEnv {
            fn drop(&mut self) {
                // SAFETY: this test serializes every mutation of these
                // SAP-specific test variables and restores their prior values.
                unsafe {
                    match self.backend.take() {
                        Some(value) => std::env::set_var("CORI_SECRETS_BACKEND", value),
                        None => std::env::remove_var("CORI_SECRETS_BACKEND"),
                    }
                    match self.override_value.take() {
                        Some(value) => std::env::set_var(ALLOW_FILE_STORE_ENV, value),
                        None => std::env::remove_var(ALLOW_FILE_STORE_ENV),
                    }
                }
            }
        }
        let _restore = RestoreEnv {
            backend: previous_backend,
            override_value: previous_override,
        };
        // SAFETY: guarded and restored above; these variables are scoped to
        // SAP's explicit debug-only file-store test path.
        unsafe {
            std::env::set_var("CORI_SECRETS_BACKEND", "file");
            std::env::set_var(ALLOW_FILE_STORE_ENV, "1");
        }

        let temp = tempfile::tempdir().expect("temporary Cori home");
        std::fs::write(
            temp.path().join("sap.toml"),
            r#"
default_profile = "production"
[profiles.production]
base_url = "https://tenant.example.com"
sap_client = "100"
"#,
        )
        .expect("SAP config");
        let credentials_dir = temp.path().join("credentials");
        let target =
            credential_target_for_owner("alice", &credentials_dir).expect("SAP credential target");
        store_access_token("alice", &credentials_dir, &target, "valid-token")
            .expect("store test token");
        assert_eq!(
            SapAdapter.check_for_owner("alice", &credentials_dir),
            AuthState::Ok
        );

        // Leave secrets-index.json untouched while removing the actual
        // secret. Index-only readiness would incorrectly remain green.
        std::fs::write(credentials_dir.join("llm-secrets.json"), b"{}")
            .expect("remove actual secret only");
        let state = SapAdapter.check_for_owner("alice", &credentials_dir);
        assert!(matches!(state, AuthState::NeedsReauth { .. }));
        assert!(credentials_dir.join("secrets-index.json").is_file());
    }
}
