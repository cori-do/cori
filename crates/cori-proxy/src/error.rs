//! Error types for the proxy crate.

use thiserror::Error;

/// Errors that can occur in the Postgres proxy.
#[derive(Debug, Error)]
pub enum ProxyError {
    /// Failed to bind to the listen address.
    #[error("failed to bind to {address}: {source}")]
    BindFailed {
        address: String,
        source: std::io::Error,
    },

    /// Failed to accept a connection.
    #[error("failed to accept connection: {0}")]
    AcceptFailed(#[source] std::io::Error),

    /// Authentication failed.
    #[error("authentication failed: {0}")]
    AuthenticationFailed(String),

    /// Invalid Biscuit token.
    #[error("invalid biscuit token: {0}")]
    InvalidToken(String),

    /// Token has expired.
    #[error("token has expired")]
    TokenExpired,

    /// Token is missing required tenant claim.
    #[error("token missing tenant claim")]
    MissingTenantClaim,

    /// Failed to connect to upstream Postgres.
    #[error("failed to connect to upstream: {0}")]
    UpstreamConnectionFailed(String),

    /// Query was rejected by RLS policy.
    #[error("query rejected: {0}")]
    QueryRejected(String),

    /// SQL parsing error.
    #[error("SQL parse error: {0}")]
    SqlParseError(String),

    /// Protocol error.
    #[error("protocol error: {0}")]
    ProtocolError(String),

    /// Internal error.
    #[error("internal error: {0}")]
    Internal(#[from] anyhow::Error),
}

