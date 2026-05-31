//! HTTP router.
//!
//! Auth layering:
//!   1. `POST /api/session` — unauthenticated (token-for-cookie exchange).
//!   2. Read endpoints — `require_cookie`.
//!   3. State-changing endpoints (`POST /api/runs`, `POST /api/trust`)
//!      — `require_cookie` **and** `require_bearer` (Authorization
//!      header carries the master token). The bearer layer defeats
//!      CSRF: a cross-site page in the same browser has the cookie
//!      but cannot read the bearer secret.
//!   4. Any unmatched `/api/{rest}` returns a real 404 JSON — never
//!      the SPA shell.
//!   5. Anything else falls through to `assets::serve`, which serves
//!      the embedded SPA (or `index.html` for deep links).

pub mod runs;
pub mod schedules;
pub mod session;
pub mod status;
pub mod stream;
pub mod trigger;
pub mod trust;
pub mod workers;
pub mod workflow;
pub mod workflows;

use axum::{
    Json,
    Router,
    http::StatusCode,
    middleware,
    response::IntoResponse,
    routing::{any, delete, get, patch, post},
};
use serde_json::json;

use crate::{assets, state::AppState};

pub fn build_router(state: AppState) -> Router {
    // Cookie + bearer: state-changing endpoints.
    let mutations = Router::new()
        .route("/api/runs", post(trigger::handler))
        .route("/api/trust", post(trust::handler))
        .route("/api/schedules", post(schedules::create))
        .route("/api/schedules/{id}", patch(schedules::patch))
        .route("/api/schedules/{id}", delete(schedules::delete))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            crate::auth::require_bearer,
        ));

    // Cookie only: read endpoints + the SSE stream.
    let reads = Router::new()
        .route("/api/status", get(status::handler))
        .route("/api/runs", get(runs::list))
        .route("/api/runs/{key}/{filename}", get(runs::trace))
        .route("/api/runs/{run_id}/stream", get(stream::handler))
        .route("/api/workflow", get(workflow::handler))
        .route("/api/workflows/recent", get(workflows::recent))
        .route("/api/workers", get(workers::handler))
        .route("/api/schedules", get(schedules::list));

    let protected = reads
        .merge(mutations)
        // /api/* catch-all 404 so the SPA fallback never swallows API typos.
        .route("/api/{*rest}", any(api_not_found))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            crate::auth::require_cookie,
        ));

    let open = Router::new().route("/api/session", post(session::exchange));

    Router::new()
        .merge(open)
        .merge(protected)
        .fallback(assets::serve)
        .with_state(state)
}

async fn api_not_found() -> impl IntoResponse {
    (
        StatusCode::NOT_FOUND,
        Json(json!({ "error": "no such API route" })),
    )
}
