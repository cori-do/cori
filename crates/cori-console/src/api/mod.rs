//! HTTP router.
//!
//! `POST /api/session` is the only route open to unauthenticated
//! callers — that's how the SPA trades the URL token for a cookie.
//! Every other route is behind the cookie middleware.

pub mod runs;
pub mod session;
pub mod status;
pub mod workflows;

use axum::{
    Router,
    middleware,
    routing::{get, post},
};

use crate::state::AppState;

pub fn build_router(state: AppState) -> Router {
    let protected = Router::new()
        .route("/api/status", get(status::handler))
        .route("/api/runs", get(runs::list))
        .route("/api/runs/{key}/{filename}", get(runs::trace))
        .route("/api/workflows/recent", get(workflows::recent))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            crate::auth::require_cookie,
        ));

    let open = Router::new().route("/api/session", post(session::exchange));

    Router::new()
        .merge(open)
        .merge(protected)
        .with_state(state)
}
