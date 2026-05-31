//! HTTP router.
//!
//! Layering:
//!   1. `POST /api/session` — unauthenticated (this is how the SPA
//!      trades the URL token for a cookie).
//!   2. Every other `/api/...` route — behind the cookie middleware.
//!   3. Any unmatched `/api/{rest}` returns a real 404 JSON — never
//!      the SPA fallback. The catch-all 404 lives in the same
//!      protected branch as the read endpoints so cross-site pages
//!      can't probe the surface anonymously.
//!   4. Anything else falls through to `assets::serve`, which serves
//!      the embedded SPA (or `index.html` for deep links).

pub mod runs;
pub mod session;
pub mod status;
pub mod workflows;

use axum::{
    Json,
    Router,
    http::StatusCode,
    middleware,
    response::IntoResponse,
    routing::{any, get, post},
};
use serde_json::json;

use crate::{assets, state::AppState};

pub fn build_router(state: AppState) -> Router {
    let protected = Router::new()
        .route("/api/status", get(status::handler))
        .route("/api/runs", get(runs::list))
        .route("/api/runs/{key}/{filename}", get(runs::trace))
        .route("/api/workflows/recent", get(workflows::recent))
        // Anything else under `/api/` is a 404 JSON — never the SPA shell.
        .route("/api/{*rest}", any(api_not_found))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            crate::auth::require_cookie,
        ));

    let open = Router::new().route("/api/session", post(session::exchange));

    Router::new()
        .merge(open)
        .merge(protected)
        // SPA fallback: everything outside `/api/*` resolves to an
        // embedded asset, falling through to `index.html` for client
        // routing on deep links.
        .fallback(assets::serve)
        .with_state(state)
}

async fn api_not_found() -> impl IntoResponse {
    (
        StatusCode::NOT_FOUND,
        Json(json!({ "error": "no such API route" })),
    )
}
