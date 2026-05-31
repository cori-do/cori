//! Cookie auth middleware. Phase 1 only protects GET endpoints; state-
//! changing endpoints in Phase 3 will additionally require the bearer
//! header (the master token) on top of this cookie.

use axum::{
    extract::{Request, State},
    http::header,
    middleware::Next,
    response::Response,
};

use crate::{error::ApiError, state::AppState};

/// Reject the request unless `Cookie: cori_session=<session_value>`
/// matches the value the server issued at exchange time.
pub async fn require_cookie(
    State(state): State<AppState>,
    req: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let cookie_header = req
        .headers()
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    let presented = cookie_value(cookie_header, "cori_session");
    let expected = state.session_value.read().await;

    let ok = match (expected.as_deref(), presented.as_deref()) {
        (Some(want), Some(got)) => constant_time_eq(want.as_bytes(), got.as_bytes()),
        _ => false,
    };
    drop(expected);

    if !ok {
        return Err(ApiError::Unauthorized);
    }
    Ok(next.run(req).await)
}

/// Reject the request unless `Authorization: Bearer <master_token>`
/// matches. Layered on top of [`require_cookie`] on state-changing
/// endpoints to defeat CSRF — a malicious page in the same browser
/// has the cookie automatically but cannot read the bearer secret.
pub async fn require_bearer(
    State(state): State<AppState>,
    req: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let presented = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .unwrap_or("");

    if !constant_time_eq(presented.as_bytes(), state.master_token.as_bytes()) {
        return Err(ApiError::Unauthorized);
    }
    Ok(next.run(req).await)
}

/// Extract `name=value` from a `Cookie` header. Returns the first match.
pub fn cookie_value(header: &str, name: &str) -> Option<String> {
    for part in header.split(';') {
        let part = part.trim();
        if let Some((k, v)) = part.split_once('=')
            && k.trim() == name
        {
            return Some(v.trim().to_string());
        }
    }
    None
}

/// Constant-time byte comparison so a wrong-but-similar value doesn't
/// leak any prefix info via timing.
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
