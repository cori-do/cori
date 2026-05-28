//! `gws` (Google Workspace CLI) auth adapter.
//!
//! The probe is `gws auth list --format=value(account)` (or whichever
//! equivalent is present). A non-zero exit or empty stdout means no
//! active account; we then point the user at `gws auth login`.

use std::process::{Command, Stdio};

use super::{AuthState, CliAuthAdapter};

pub struct GwsAdapter;

impl CliAuthAdapter for GwsAdapter {
    fn binary(&self) -> &'static str {
        "gws"
    }

    fn display_name(&self) -> &'static str {
        "Google Workspace"
    }

    fn login_hint(&self) -> String {
        "run: gws auth login".to_string()
    }

    fn check(&self) -> AuthState {
        let out = match Command::new("gws")
            .args(["auth", "list", "--format=value(account)"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
        {
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
        AuthState::Ok
    }
}
