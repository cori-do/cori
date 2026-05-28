//! OAuth / token-store subsystem (Phase 5).
//!
//! Public surface:
//!
//! ```ignore
//! use cori_broker::oauth::{TokenStore, TokenKey, Owner, default_store, token_for};
//! ```
//!
//! - [`store::TokenStore`] is the swappable backend trait. The default
//!   implementation is [`store::KeychainStore`] (OS keychain); a
//!   [`store::FileStore`] fallback is used on machines without a usable
//!   keychain.
//! - [`pkce`] runs the user-interactive PKCE flow.
//! - [`metadata::McpOAuthConfig`] is the per-MCP-server OAuth
//!   configuration read from `~/.cori/mcp-servers.json`.
//!
//! Higher-level entry point: [`token_for`] resolves the access token
//! the broker should use for a given `(server_id, owner)` pair,
//! refreshing transparently if the stored token is near expiry. If no
//! token is available (or it expired and cannot be refreshed silently)
//! it returns a [`TokenForError::NeedsReauth`] that the calling broker
//! converts into [`crate::BrokerError::NeedsReauth`].

pub mod metadata;
pub mod pkce;
pub mod store;
pub mod types;

use std::sync::Arc;

use thiserror::Error;

pub use metadata::McpOAuthConfig;
pub use store::{TokenStore, default_store};
pub use types::{AuthKind, Owner, Token, TokenKey};

/// Margin before token expiry at which we proactively try to refresh.
pub const REFRESH_MARGIN_SECS: i64 = 60;

#[derive(Debug, Error)]
pub enum TokenForError {
    #[error("no token stored for {key}")]
    NeedsReauth {
        key: TokenKey,
        auth_kind: AuthKind,
        hint: String,
    },
    #[error("token-store error: {0}")]
    Store(#[from] store::StoreError),
}

/// Resolve a usable access token for `(server_id, owner)`.
///
/// v1 implementation: returns the stored token if present and not
/// expiring. **Silent refresh is not yet implemented** — when the
/// stored token is within [`REFRESH_MARGIN_SECS`] of expiry we surface
/// it as `NeedsReauth` so the user reruns `cori login <server>`.
/// Implementing the OAuth refresh-grant call lives in the same
/// follow-up that adds DCR + device + client-credentials.
pub fn token_for(store: &Arc<dyn TokenStore>, key: &TokenKey) -> Result<Token, TokenForError> {
    let Some(tok) = store.get(key)? else {
        return Err(TokenForError::NeedsReauth {
            key: key.clone(),
            auth_kind: AuthKind::Pkce,
            hint: format!("run: cori login {}", key.server_id),
        });
    };
    if tok.is_expiring(REFRESH_MARGIN_SECS) {
        return Err(TokenForError::NeedsReauth {
            key: key.clone(),
            auth_kind: tok.auth_kind,
            hint: format!(
                "token is expired or expiring soon — run: cori login {}",
                key.server_id
            ),
        });
    }
    Ok(tok)
}
