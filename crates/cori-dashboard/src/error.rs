//! Error types for the dashboard crate.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use thiserror::Error;

/// Errors that can occur in the dashboard.
#[derive(Debug, Error)]
pub enum DashboardError {
    /// Failed to start the server.
    #[error("failed to start dashboard: {0}")]
    StartupFailed(String),

    /// Authentication failed.
    #[error("authentication failed")]
    AuthenticationFailed,

    /// Authorization failed.
    #[error("not authorized")]
    NotAuthorized,

    /// Resource not found.
    #[error("not found: {0}")]
    NotFound(String),

    /// Invalid request.
    #[error("invalid request: {0}")]
    InvalidRequest(String),

    /// Database error.
    #[error("database error: {0}")]
    DatabaseError(String),

    /// Token error.
    #[error("token error: {0}")]
    TokenError(String),

    /// Internal error.
    #[error("internal error: {0}")]
    Internal(#[from] anyhow::Error),
}

impl IntoResponse for DashboardError {
    fn into_response(self) -> Response {
        let status = match &self {
            DashboardError::AuthenticationFailed => StatusCode::UNAUTHORIZED,
            DashboardError::NotAuthorized => StatusCode::FORBIDDEN,
            DashboardError::NotFound(_) => StatusCode::NOT_FOUND,
            DashboardError::InvalidRequest(_) => StatusCode::BAD_REQUEST,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };

        (status, self.to_string()).into_response()
    }
}
