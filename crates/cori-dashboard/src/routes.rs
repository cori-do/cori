//! Route definitions for the dashboard.

use crate::handlers;
use axum::{routing::get, Router};

/// Create the dashboard router.
pub fn create_router() -> Router {
    Router::new()
        .route("/", get(handlers::home))
        .route("/schema", get(handlers::schema_browser))
        .route("/roles", get(handlers::roles))
        .route("/tokens", get(handlers::tokens))
        .route("/audit", get(handlers::audit_logs))
        .route("/approvals", get(handlers::approvals))
        .route("/settings", get(handlers::settings))
}

