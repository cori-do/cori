//! Public types for the OAuth / token-store subsystem.

use std::fmt;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Which security principal a stored token belongs to.
///
/// `User(<user_id>)` — token usable only by runs initiated by that user.
/// `Service(<pool>)` — token usable by any run routed to that service
/// pool's worker (typically org-shared credentials).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "id")]
pub enum Owner {
    User(String),
    Service(String),
}

impl Owner {
    pub fn as_path_segment(&self) -> String {
        match self {
            Owner::User(id) => format!("user.{id}"),
            Owner::Service(pool) => format!("service.{pool}"),
        }
    }
}

impl fmt::Display for Owner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Owner::User(id) => write!(f, "user {id}"),
            Owner::Service(pool) => write!(f, "service {pool}"),
        }
    }
}

/// A token-store lookup key.
///
/// Tokens are scoped per (server, owner). DCR *client registrations* are
/// scoped without owner (one per server, org-wide); a separate dedicated
/// key form will be added when DCR ships.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TokenKey {
    pub server_id: String,
    pub owner: Owner,
}

impl TokenKey {
    pub fn new(server_id: impl Into<String>, owner: Owner) -> Self {
        Self {
            server_id: server_id.into(),
            owner,
        }
    }

    /// Stable string used as the keychain entry name and as a key inside
    /// the encrypted-file fallback / metadata index.
    pub fn as_storage_key(&self) -> String {
        format!("{}::{}", self.server_id, self.owner.as_path_segment())
    }
}

impl fmt::Display for TokenKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} for {}", self.server_id, self.owner)
    }
}

/// The kind of credential exchange that obtained a token. Used by
/// `cori login` to pick the right re-auth flow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthKind {
    /// User-interactive OAuth2 with PKCE.
    Pkce,
    /// Service-identity client credentials. Not implemented in v1.
    ClientCredentials,
    /// Headless OAuth2 device grant. Not implemented in v1.
    Device,
    /// A static API key / personal access token entered by the user.
    StaticToken,
}

/// A bearer token plus the metadata Cori needs to refresh / surface it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Token {
    /// The opaque access token. Treat as a secret.
    pub access_token: String,
    /// Optional refresh token (OAuth refresh-grant). May be absent for
    /// short-lived tokens; `cori login` is then the recovery path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    /// `Bearer` for OAuth flows; `Static` for paste-in API keys.
    #[serde(default = "default_token_type")]
    pub token_type: String,
    /// UTC time the token stops being valid. Absent ⇒ unknown / never.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
    /// The space-separated scope grant string the AS returned (if any).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// How this token was originally obtained — used to drive re-auth.
    pub auth_kind: AuthKind,
}

fn default_token_type() -> String {
    "Bearer".to_string()
}

impl Token {
    /// Returns true when the token has an expiry that has already passed
    /// or is within `margin_secs` of expiring.
    pub fn is_expiring(&self, margin_secs: i64) -> bool {
        match self.expires_at {
            None => false,
            Some(t) => (t - Utc::now()).num_seconds() <= margin_secs,
        }
    }
}
