//! `POST /api/session` — exchange the URL master token for an HttpOnly cookie.

use axum::{
    Json,
    extract::State,
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::IntoResponse,
};
use serde::Deserialize;
use serde_json::json;

use crate::{state::AppState, token::generate_session_value};

#[derive(Deserialize)]
pub struct Body {
    pub token: String,
}

pub async fn exchange(
    State(state): State<AppState>,
    Json(body): Json<Body>,
) -> impl IntoResponse {
    if !constant_time_eq(body.token.as_bytes(), state.master_token.as_bytes()) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "invalid token" })),
        )
            .into_response();
    }

    let value = generate_session_value();
    *state.session_value.write().await = Some(value.clone());

    // Loopback HTTP: Secure can't be set. HttpOnly + SameSite=Strict are the
    // real protections; loopback excludes the open internet.
    let cookie = format!("cori_session={value}; HttpOnly; SameSite=Strict; Path=/");

    let mut headers = HeaderMap::new();
    headers.insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&cookie).expect("session cookie value is ascii"),
    );

    (StatusCode::OK, headers, Json(json!({ "ok": true }))).into_response()
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}
