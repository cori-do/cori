//! `gws` (Google Workspace CLI) auth adapter.
//!
//! Probe: `gws auth status` (exit 0 + non-empty stdout == signed in).
//!
//! Managed login: `gws auth login` normally requires the user to have
//! run `gws auth setup` first — create a GCP project, enable the
//! Workspace APIs, mint an OAuth client, download `client_secret.json`.
//! That is the step non-technical users never get past. Cori removes it
//! by provisioning `~/.config/gws/client_secret.json` with a
//! **Cori-owned** OAuth client, then delegating to the vendor's own
//! `gws auth login`, which opens the browser and manages tokens/refresh
//! itself. A `client_secret.json` the user already has is never
//! overwritten.

use std::path::PathBuf;
use std::process::{Command, Stdio};

use serde_json::json;

use super::{AuthState, CliAuthAdapter, ManagedLogin, OAuthClient};
use crate::install::resolve_binary;

/// Default Workspace services requested at sign-in. Narrow on purpose:
/// full-access Gmail/Drive scopes trigger Google's restricted-scope
/// review and are not needed for typical workflows. Users can widen via
/// `cori config set capability.gws.services <list>`.
pub const DEFAULT_SERVICES: &[&str] = &["drive", "docs", "sheets", "gmail", "calendar"];

pub struct GwsAdapter;

impl CliAuthAdapter for GwsAdapter {
    fn binary(&self) -> &'static str {
        "gws"
    }

    fn display_name(&self) -> &'static str {
        "Google Workspace"
    }

    fn login_hint(&self) -> String {
        "run: cori login gws".to_string()
    }

    fn check(&self) -> AuthState {
        let bin = match resolve_binary("gws") {
            Some(p) => p,
            None => return AuthState::Unknown,
        };
        let mut cmd = Command::new(bin);
        cmd.args(["auth", "status"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        super::apply_spawn_env(&mut cmd, self);
        let out = match cmd.output() {
            Ok(o) => o,
            Err(_) => return AuthState::Unknown,
        };
        if !out.status.success() {
            return AuthState::NeedsReauth {
                hint: self.login_hint(),
            };
        }
        let stdout = String::from_utf8_lossy(&out.stdout);
        if stdout.trim().is_empty() {
            return AuthState::NeedsReauth {
                hint: self.login_hint(),
            };
        }
        // `gws auth status` exits 0 even when signed out; the JSON body
        // is the truth. Signed out == `"auth_method": "none"`, OR
        // credentials that exist but can't be decrypted
        // (`"encryption_valid": false` — e.g. they were encrypted under
        // a different keyring backend or on another machine).
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(stdout.trim()) {
            let signed_out = v.get("auth_method").and_then(|m| m.as_str()) == Some("none");
            let unreadable =
                v.get("encryption_valid").and_then(|b| b.as_bool()) == Some(false);
            if signed_out || unreadable {
                return AuthState::NeedsReauth {
                    hint: self.login_hint(),
                };
            }
        }
        AuthState::Ok
    }

    fn baked_client(&self) -> Option<OAuthClient> {
        // Baked in by release CI; absent in dev builds. See the CBC
        // contract (§7, provisioned clients) for the release procedure.
        match (
            option_env!("CORI_GWS_OAUTH_CLIENT_ID"),
            option_env!("CORI_GWS_OAUTH_CLIENT_SECRET"),
        ) {
            (Some(id), Some(secret)) if !id.is_empty() => Some(OAuthClient {
                client_id: id.to_string(),
                client_secret: secret.to_string(),
            }),
            _ => None,
        }
    }

    /// File-based credential encryption key instead of the OS keychain.
    ///
    /// The release binary is ad-hoc signed, so macOS keychain ACLs
    /// re-prompt on every process (and "Always allow" breaks on each
    /// gws update) — a prompt storm for Console probes and workflow
    /// steps. `file` is gws's supported headless mode: the AES key
    /// lives in `~/.config/gws/.encryption_key` (same trust boundary as
    /// `client_secret.json` next to it). A user-set value always wins.
    fn spawn_env(&self) -> &'static [(&'static str, &'static str)] {
        &[("GOOGLE_WORKSPACE_CLI_KEYRING_BACKEND", "file")]
    }

    fn managed_login(&self, client: &OAuthClient, services: &[String]) -> Option<ManagedLogin> {
        let path = gws_config_dir()?.join("client_secret.json");
        let config = serde_json::to_string_pretty(&json!({
            "installed": {
                "client_id": client.client_id,
                "client_secret": client.client_secret,
                "auth_uri": "https://accounts.google.com/o/oauth2/auth",
                "token_uri": "https://oauth2.googleapis.com/token",
                "auth_provider_x509_cert_url": "https://www.googleapis.com/oauth2/v1/certs",
                "redirect_uris": ["http://localhost"]
            }
        }))
        .ok()?;

        let mut argv = vec![
            "gws".to_string(),
            "auth".to_string(),
            "login".to_string(),
        ];
        let services: Vec<&str> = if services.is_empty() {
            DEFAULT_SERVICES.to_vec()
        } else {
            services.iter().map(String::as_str).collect()
        };
        argv.push("--services".to_string());
        argv.push(services.join(","));

        Some(ManagedLogin {
            client_config_path: path,
            client_config: config,
            login_argv: argv,
        })
    }
}

/// `gws` keeps its config in `$XDG_CONFIG_HOME/gws` (`~/.config/gws`)
/// on unix; on Windows we fall back to the platform config dir.
fn gws_config_dir() -> Option<PathBuf> {
    #[cfg(unix)]
    {
        if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME")
            && !xdg.is_empty()
        {
            return Some(PathBuf::from(xdg).join("gws"));
        }
        dirs::home_dir().map(|h| h.join(".config").join("gws"))
    }
    #[cfg(not(unix))]
    {
        dirs::config_dir().map(|d| d.join("gws"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn managed_login_provisions_installed_app_client() {
        let client = OAuthClient {
            client_id: "id-123".into(),
            client_secret: "secret-456".into(),
        };
        let plan = GwsAdapter
            .managed_login(&client, &[])
            .expect("plan on a machine with a home dir");
        assert!(plan.client_config_path.ends_with("gws/client_secret.json"));
        let v: serde_json::Value = serde_json::from_str(&plan.client_config).unwrap();
        assert_eq!(v["installed"]["client_id"], "id-123");
        assert_eq!(
            v["installed"]["token_uri"],
            "https://oauth2.googleapis.com/token"
        );
        assert_eq!(plan.login_argv[..3], ["gws", "auth", "login"]);
        // Narrow default scopes, not --full.
        assert!(plan.login_argv.contains(&"--services".to_string()));
        assert!(!plan.login_argv.contains(&"--full".to_string()));
    }

    #[test]
    fn custom_services_override_defaults() {
        let client = OAuthClient {
            client_id: "x".into(),
            client_secret: "y".into(),
        };
        let plan = GwsAdapter
            .managed_login(&client, &["sheets".to_string()])
            .unwrap();
        let idx = plan
            .login_argv
            .iter()
            .position(|a| a == "--services")
            .unwrap();
        assert_eq!(plan.login_argv[idx + 1], "sheets");
    }
}
