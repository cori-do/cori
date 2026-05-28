//! Worker identity resolution.
//!
//! A [`WorkerIdentity`](cori_protocol::WorkerIdentity) is fixed at
//! launch and determines the Temporal task queue a worker polls. This
//! module provides the [`IdentitySource`] trait and the v1 [`OsUser`]
//! implementation; an OIDC/SSO source is the enterprise extension
//! point and is deferred to a later phase.

use cori_protocol::{IdentityValidationError, WorkerIdentity};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum IdentityError {
    #[error("could not resolve OS user: {0}")]
    OsUserUnavailable(String),

    #[error("invalid identity: {0}")]
    Invalid(#[from] IdentityValidationError),
}

/// How to obtain the running worker's identity. The local default is
/// [`OsUser`]; enterprises plug an OIDC implementation behind the same
/// trait.
pub trait IdentitySource: Send + Sync {
    fn resolve(&self) -> Result<WorkerIdentity, IdentityError>;
}

/// Resolve identity from the operating-system user.
///
/// Reads `$USER` (Unix) or `$USERNAME` (Windows), normalizes to
/// lowercase, replaces any character that isn't `[a-z0-9_-]` with `_`,
/// and constructs a [`WorkerIdentity::Person`]. Normalization fails
/// loudly if the resulting string is empty.
pub struct OsUser;

impl OsUser {
    pub fn new() -> Self {
        Self
    }
}

impl Default for OsUser {
    fn default() -> Self {
        Self::new()
    }
}

impl IdentitySource for OsUser {
    fn resolve(&self) -> Result<WorkerIdentity, IdentityError> {
        let raw = std::env::var("USER")
            .or_else(|_| std::env::var("LOGNAME"))
            .or_else(|_| std::env::var("USERNAME"))
            .map_err(|_| {
                IdentityError::OsUserUnavailable(
                    "neither $USER, $LOGNAME, nor $USERNAME is set".to_string(),
                )
            })?;
        let normalized = normalize(&raw);
        if normalized.is_empty() {
            return Err(IdentityError::OsUserUnavailable(format!(
                "OS user `{raw}` normalized to empty string"
            )));
        }
        Ok(WorkerIdentity::person(normalized)?)
    }
}

fn normalize(raw: &str) -> String {
    raw.chars()
        .map(|c| c.to_ascii_lowercase())
        .map(|c| {
            if c.is_ascii_digit() || c.is_ascii_lowercase() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_mixed_case_and_specials() {
        assert_eq!(normalize("Jean.Doe"), "jean_doe");
        assert_eq!(normalize("alice"), "alice");
        assert_eq!(normalize("Bob-42"), "bob-42");
    }

    #[test]
    fn os_user_uses_env_var() {
        // SAFETY: tests are single-threaded inside this fn; we restore
        // the env var afterwards.
        let prev = std::env::var("USER").ok();
        unsafe {
            std::env::set_var("USER", "Test.User");
        }
        let id = OsUser.resolve().expect("resolves");
        match id {
            WorkerIdentity::Person { user_id } => assert_eq!(user_id, "test_user"),
            other => panic!("expected Person, got {other:?}"),
        }
        unsafe {
            match prev {
                Some(v) => std::env::set_var("USER", v),
                None => std::env::remove_var("USER"),
            }
        }
    }
}
