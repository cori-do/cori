//! Phase 3 auth tests for state-changing endpoints.
//!
//!  * `POST /api/runs` without auth → 401
//!  * `POST /api/runs` with cookie but no bearer → 401
//!  * `POST /api/trust` requires both layers too

use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use cori_console::{AppState, generate_session_value};
use tower::ServiceExt;

const TOKEN: &str = "master-token-for-tests";

async fn app_with_session() -> (axum::Router, String) {
    let state = AppState::new(TOKEN.to_string(), std::env::temp_dir());
    let session = generate_session_value();
    *state.session_value.write().await = Some(session.clone());
    (cori_console::api::build_router(state), session)
}

fn cookie_header(value: &str) -> String {
    format!("cori_session={value}")
}

#[tokio::test]
async fn post_runs_without_anything_is_401() {
    let (app, _) = app_with_session().await;
    let resp = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/runs")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"source":"./x","params":{},"dry_run":false}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn post_runs_with_cookie_only_is_401() {
    let (app, session) = app_with_session().await;
    let resp = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/runs")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, cookie_header(&session))
                .body(Body::from(
                    r#"{"source":"./x","params":{},"dry_run":false}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    // Cookie passes; bearer absent → 401 from the bearer layer.
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn post_trust_with_cookie_only_is_401() {
    let (app, session) = app_with_session().await;
    let resp = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/trust")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, cookie_header(&session))
                .body(Body::from(
                    r#"{"host":"github.com","repo":"a/b","sha":"abc","declared_capabilities":[]}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn post_runs_with_wrong_bearer_is_401() {
    let (app, session) = app_with_session().await;
    let resp = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/runs")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, cookie_header(&session))
                .header(header::AUTHORIZATION, "Bearer not-the-master-token")
                .body(Body::from(
                    r#"{"source":"./x","params":{},"dry_run":false}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn get_stream_for_unknown_run_is_404() {
    let (app, session) = app_with_session().await;
    let resp = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/api/runs/00000000-0000-0000-0000-000000000000/stream")
                .header(header::COOKIE, cookie_header(&session))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}
