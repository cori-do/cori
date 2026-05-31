//! Tests for the SPA fallback wiring.
//!
//!  * `/` → embedded `index.html`
//!  * deep link like `/runs/x/2026-...Z` → also `index.html`
//!    (RR7 handles client-side)
//!  * `/api/no-such-route` → real 404 JSON, never the SPA shell
//!  * a session that hasn't been exchanged → 401 on protected routes

use axum::body::Body;
use axum::http::{Request, StatusCode};
use cori_console::AppState;
use tower::ServiceExt; // for `oneshot`

fn app() -> axum::Router {
    let state = AppState::new("test-token".to_string(), std::env::temp_dir());
    cori_console::api::build_router(state)
}

async fn body_string(resp: axum::response::Response) -> String {
    let bytes = axum::body::to_bytes(resp.into_body(), 4 * 1024 * 1024)
        .await
        .expect("body");
    String::from_utf8_lossy(&bytes).into_owned()
}

#[tokio::test]
async fn root_returns_spa_index_html() {
    let resp = app()
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let ct = resp
        .headers()
        .get("content-type")
        .map(|v| v.to_str().unwrap().to_string())
        .unwrap_or_default();
    assert!(ct.starts_with("text/html"), "got content-type {ct}");
    let body = body_string(resp).await;
    assert!(
        body.contains("<html") && body.contains("</html>"),
        "expected HTML, got: {}",
        &body.chars().take(200).collect::<String>()
    );
}

#[tokio::test]
async fn deep_link_falls_back_to_index() {
    let resp = app()
        .oneshot(
            Request::builder()
                .uri("/runs/translate-abcd1234/2026-01-01T00-00-00Z")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(
        body.contains("<html"),
        "expected SPA index.html for deep link"
    );
}

#[tokio::test]
async fn api_unknown_route_returns_404_json_not_spa() {
    let resp = app()
        .oneshot(
            Request::builder()
                .uri("/api/no-such-route")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    // 401 first (no cookie) — that's also acceptable; the critical
    // invariant is "not 200 HTML".
    assert!(
        resp.status() == StatusCode::NOT_FOUND || resp.status() == StatusCode::UNAUTHORIZED,
        "unexpected status {} for /api/no-such-route",
        resp.status()
    );
    let ct = resp
        .headers()
        .get("content-type")
        .map(|v| v.to_str().unwrap().to_string())
        .unwrap_or_default();
    assert!(
        ct.contains("application/json"),
        "expected JSON content-type for /api/* 404 / 401, got `{ct}`"
    );
}

#[tokio::test]
async fn protected_api_without_cookie_is_401() {
    let resp = app()
        .oneshot(
            Request::builder()
                .uri("/api/status")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}
