//! Per-CLI authentication adapters.
//!
//! Many CLIs Cori dispatches (`gws`, `gh`, `aws`, …) carry their own
//! login state. When the broker is about to spawn one of these, the
//! adapter probes whether the CLI is currently authed — if not, the
//! step fails fast with a clean `NeedsReauth` instead of executing and
//! emitting a confusing 401.
//!
//! Adapters are intentionally tiny: each one knows how to run a
//! "whoami"-style probe and how to suggest a re-auth command. v1 ships
//! `gws`; unknown CLIs are passed through without an auth check (the
//! whitelist in [`crate::capabilities`] still gates them).

pub mod gws;

use std::sync::OnceLock;

/// Result of an adapter probe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthState {
    Ok,
    /// Adapter is confident the CLI is not authenticated. `hint` is a
    /// one-line command the user can run to fix it.
    NeedsReauth {
        hint: String,
    },
    /// The probe itself failed (e.g. binary not found, timed out). We
    /// treat this as `Ok` to avoid false positives — the actual spawn
    /// will surface any real problem with its own error.
    Unknown,
}

pub trait CliAuthAdapter: Send + Sync {
    /// The binary this adapter probes (`"gws"`, `"gh"`, …).
    fn binary(&self) -> &'static str;
    /// Probe the CLI's current auth state.
    fn check(&self) -> AuthState;
    /// Human-readable name of the underlying account / service.
    fn display_name(&self) -> &'static str {
        self.binary()
    }
    /// One-line `cori login`-style hint shown by `cori login <bin>`.
    fn login_hint(&self) -> String {
        format!("see `{} --help`", self.binary())
    }
}

static ADAPTERS: OnceLock<Vec<Box<dyn CliAuthAdapter>>> = OnceLock::new();

fn registry() -> &'static [Box<dyn CliAuthAdapter>] {
    ADAPTERS.get_or_init(|| {
        let v: Vec<Box<dyn CliAuthAdapter>> = vec![Box::new(gws::GwsAdapter)];
        v
    })
}

/// Look up an adapter by CLI binary name.
pub fn for_binary(name: &str) -> Option<&'static dyn CliAuthAdapter> {
    registry()
        .iter()
        .find(|a| a.binary() == name)
        .map(|a| a.as_ref())
}

/// Convenience: probe a binary; returns `AuthState::Ok` when no
/// adapter is registered for that name (unknown CLI is best-effort).
pub fn check_known(name: &str) -> AuthState {
    match for_binary(name) {
        Some(a) => a.check(),
        None => AuthState::Ok,
    }
}
