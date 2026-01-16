//! Route definitions for the dashboard.

use crate::handlers;
use crate::state::AppState;
use axum::{
    routing::{delete, get, post, put},
    Router,
};

/// Create the dashboard router without state (for backward compatibility).
pub fn create_router() -> Router {
    Router::new()
}

/// Create the router with application state.
pub fn create_router_with_state(state: AppState) -> Router {
    Router::new()
        // Page routes (HTML)
        .route("/", get(handlers::home))
        .route("/schema", get(handlers::schema_browser))
        .route("/roles", get(handlers::roles_list))
        .route("/roles/new", get(handlers::role_new))
        .route("/roles/{name}", get(handlers::role_detail))
        .route("/roles/{name}/edit", get(handlers::role_edit))
        .route("/tokens", get(handlers::tokens))
        .route("/audit", get(handlers::audit_logs))
        .route("/approvals", get(handlers::approvals))
        .route("/settings", get(handlers::settings))
        // API routes (JSON/HTMX)
        .nest("/api", api_routes())
        .with_state(state)
}

/// API routes for HTMX interactions and JSON responses.
fn api_routes() -> Router<AppState> {
    Router::new()
        // Schema API
        .route("/schema", get(handlers::api::schema_get))
        .route("/schema/refresh", post(handlers::api::schema_refresh))
        // Roles API
        .route("/roles", get(handlers::api::roles_list))
        .route("/roles", post(handlers::api::role_create))
        .route("/roles/{name}", get(handlers::api::role_get))
        .route("/roles/{name}", put(handlers::api::role_update))
        .route("/roles/{name}", delete(handlers::api::role_delete))
        .route("/roles/{name}/mcp-preview", get(handlers::api::role_mcp_preview))
        // Tokens API
        .route("/tokens/mint-role", post(handlers::api::token_mint_role))
        .route("/tokens/mint-agent", post(handlers::api::token_mint_agent))
        .route("/tokens/attenuate", post(handlers::api::token_attenuate))
        .route("/tokens/inspect", post(handlers::api::token_inspect))
        // Audit API
        .route("/audit", get(handlers::api::audit_list))
        .route("/audit/{id}", get(handlers::api::audit_get))
        .route("/audit/{id}/tree", get(handlers::api::audit_get_tree))
        .route("/audit/{id}/children", get(handlers::api::audit_get_children))
        .route("/audit/{id}/children-rows", get(handlers::api::audit_get_children_rows))
        // Approvals API
        .route("/approvals", get(handlers::api::approvals_list))
        .route("/approvals/{id}", get(handlers::api::approval_get))
        .route("/approvals/{id}/approve", post(handlers::api::approval_approve))
        .route("/approvals/{id}/reject", post(handlers::api::approval_reject))
        // Settings API
        .route("/settings", get(handlers::api::settings_get))
        .route("/settings/guardrails", put(handlers::api::settings_update_guardrails))
        .route("/settings/audit", put(handlers::api::settings_update_audit))
}

