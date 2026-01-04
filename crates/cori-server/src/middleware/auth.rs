use crate::state::AppState;
use axum::{
    extract::Request,
    http::{HeaderMap, StatusCode},
    middleware::Next,
    response::Response,
};
use biscuit_auth::builder::AuthorizerBuilder;
use biscuit_auth::{Authorizer, Biscuit};
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct VerifiedSession {
    pub agent_id: String,
    pub user_id: String,
    pub session_id: String,
    pub expiry_unix: u64,
}

/// Axum middleware enforcing the "Double-Lock" protocol:
/// - verify Biscuit signature (human + agent binding)
/// - re-verify agent identity (prevents token theft)
/// - run a simple Biscuit policy for the operation
pub async fn enforce_double_lock(
    mut req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let state = req
        .extensions()
        .get::<Arc<AppState>>()
        .cloned()
        .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;

    let headers = req.headers();
    let agent_id = extract_agent_id(headers).ok_or(StatusCode::UNAUTHORIZED)?;
    let token_str = extract_biscuit(headers).ok_or(StatusCode::UNAUTHORIZED)?;

    let biscuit = Biscuit::from_base64(&token_str, state.token_factory.public_key())
        .map_err(|_| StatusCode::FORBIDDEN)?;

    // Operation context injection (MVP).
    let operation = req
        .uri()
        .path()
        .rsplit('/')
        .next()
        .unwrap_or("unknown");

    // Build authorizer with time, operation fact, and policy using AuthorizerBuilder
    let now = unix_now_seconds();
    let mut authorizer = AuthorizerBuilder::new()
        .code(format!(
            r#"
            time({now});
            operation("{operation}");
            allow if user("admin"), operation("update_record");
            "#
        ))
        .map_err(|_| StatusCode::FORBIDDEN)?
        .build(&biscuit)
        .map_err(|_| StatusCode::FORBIDDEN)?;

    // Re-verify agent binding.
    let token_agent = query_first_string(&mut authorizer, "data($a) <- agent($a)")?
        .ok_or(StatusCode::FORBIDDEN)?;
    if token_agent != agent_id {
        return Err(StatusCode::FORBIDDEN);
    }

    // Extract user/session/expiry
    let user_id = query_first_string(&mut authorizer, "data($u) <- user($u)")?
        .ok_or(StatusCode::FORBIDDEN)?;
    let session_id = query_first_string(&mut authorizer, "data($s) <- session_id($s)")?
        .unwrap_or_else(|| "unknown".to_string());
    let expiry_unix = query_first_i64(&mut authorizer, "data($t) <- expiry($t)")?
        .ok_or(StatusCode::FORBIDDEN)?
        .max(0) as u64;

    if unix_now_seconds() > expiry_unix {
        return Err(StatusCode::UNAUTHORIZED);
    }

    authorizer
        .authorize()
        .map_err(|_| StatusCode::FORBIDDEN)?;

    req.extensions_mut().insert(VerifiedSession {
        agent_id,
        user_id,
        session_id,
        expiry_unix,
    });

    Ok(next.run(req).await)
}

fn extract_biscuit(headers: &HeaderMap) -> Option<String> {
    if let Some(v) = headers.get("x-session-token").and_then(|h| h.to_str().ok()) {
        let s = v.trim();
        if !s.is_empty() {
            return Some(s.to_string());
        }
    }
    if let Some(v) = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
    {
        if let Some(rest) = v.strip_prefix("Bearer ") {
            let rest = rest.trim();
            if !rest.is_empty() {
                return Some(rest.to_string());
            }
        }
    }
    None
}

fn extract_agent_id(headers: &HeaderMap) -> Option<String> {
    if let Some(v) = headers.get("x-agent-id").and_then(|h| h.to_str().ok()) {
        let s = v.trim();
        if !s.is_empty() {
            return Some(s.to_string());
        }
    }
    // MVP fallback: allow "Authorization: Bearer agent:<id>" when not using Biscuit.
    if let Some(v) = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
    {
        if let Some(rest) = v.strip_prefix("Bearer ") {
            if let Some(id) = rest.strip_prefix("agent:") {
                let id = id.trim();
                if !id.is_empty() {
                    return Some(id.to_string());
                }
            }
        }
    }
    None
}

fn unix_now_seconds() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn query_first_string(authorizer: &mut Authorizer, rule: &str) -> Result<Option<String>, StatusCode> {
    let res: Vec<(String,)> = authorizer.query(rule).map_err(|_| StatusCode::FORBIDDEN)?;
    Ok(res.into_iter().next().map(|t| t.0))
}

fn query_first_i64(authorizer: &mut Authorizer, rule: &str) -> Result<Option<i64>, StatusCode> {
    let res: Vec<(i64,)> = authorizer.query(rule).map_err(|_| StatusCode::FORBIDDEN)?;
    Ok(res.into_iter().next().map(|t| t.0))
}


