use crate::{assets, state::AppState};
use axum::{
    extract::{Extension, Query},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use oxide_auth::frontends::simple::endpoint::{access_token_flow, authorization_flow};
use oxide_auth::primitives::registrar::{Client, RegisteredUrl};
use oxide_auth_axum::{OAuthRequest, OAuthResponse, WebError};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
use uuid::Uuid;

/// Wrapper for OAuthResponse that ensures HTML content has proper content-type header
pub struct HtmlOAuthResponse(OAuthResponse);

impl IntoResponse for HtmlOAuthResponse {
    fn into_response(self) -> Response {
        let mut response = self.0.into_response();
        
        // Check if the response body might be HTML (heuristic: starts with <!doctype or <html)
        // Since we can't easily inspect the body without consuming it, we'll check the status
        // and assume authorization flow responses that are 200 OK are likely HTML forms
        if response.status() == StatusCode::OK {
            // Set content-type to text/html for successful authorization responses
            response.headers_mut().insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/html; charset=utf-8"),
            );
        }
        
        response
    }
}

pub fn router() -> Router {
    Router::new()
        .route("/auth/login", get(login_page))
        .route("/auth/callback", get(oauth_callback))
        .route("/auth/poll", get(poll_device_code))
        .route("/oidc/authorize", get(oidc_authorize).post(oidc_authorize))
        .route("/oidc/token", post(oidc_token))
        .route("/oidc/jwks", get(jwks_get))
        .route("/mcp/start_login_flow", post(start_login_flow))
}

async fn login_page() -> Result<Html<String>, (StatusCode, String)> {
    assets::raw_login_html()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

/// Oxide-auth authorization endpoint.
pub async fn oidc_authorize(
    Extension(state): Extension<Arc<AppState>>,
    req: OAuthRequest,
) -> Result<HtmlOAuthResponse, WebError> {
    let registrar = state.oauth_clients.lock().await;
    let mut authorizer = state.oauth_authorizer.lock().await;
    let mut solicitor = state
        .oauth_solicitor
        .lock()
        .map_err(|_| WebError::InternalError(Some("solicitor lock poisoned".to_string())))?;

    let mut flow = authorization_flow(&*registrar, &mut *authorizer, &mut *solicitor);
    let resp = flow.execute(req)?;
    Ok(HtmlOAuthResponse(resp))
}

async fn jwks_get() -> impl IntoResponse {
    (StatusCode::NOT_IMPLEMENTED, Json(json!({"error": "jwks_not_implemented"})))
}

/// Oxide-auth token endpoint.
pub async fn oidc_token(
    Extension(state): Extension<Arc<AppState>>,
    req: OAuthRequest,
) -> Result<OAuthResponse, WebError> {
    let registrar = state.oauth_clients.lock().await;
    let mut authorizer = state.oauth_authorizer.lock().await;
    let mut issuer = state.oauth_issuer.lock().await;

    let mut flow = access_token_flow(&*registrar, &mut *authorizer, &mut *issuer);
    let resp = flow.execute(req)?;
    Ok(resp)
}

// -----------------------------
// Device-code-ish login flow for agents (Double-Lock)
// -----------------------------

#[derive(Debug, Deserialize)]
struct StartLoginRequest {
    /// Optional client_id override; default is the agent_id.
    client_id: Option<String>,
}

#[derive(Debug, Serialize)]
struct StartLoginResponse {
    device_code: String,
    verification_uri_complete: String,
}

async fn start_login_flow(
    Extension(state): Extension<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<StartLoginRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    let agent_id = extract_agent_id(&headers).ok_or(StatusCode::UNAUTHORIZED)?;
    let device_code = Uuid::new_v4().to_string();

    {
        let mut pending = state.pending_logins.write().await;
        pending.insert(device_code.clone(), agent_id.clone());
    }

    let client_id = req.client_id.unwrap_or(agent_id.clone());
    let base = server_base_url(&state.cfg);
    let redirect_uri = format!("{base}/auth/callback");

    // Register client dynamically (public client; redirect restricted to localhost callback).
    {
        let url = redirect_uri.parse::<url::Url>().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        if url.host_str() != Some("localhost") {
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
        let reg = RegisteredUrl::Semantic(url);
        let scope = "default".parse().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        let mut clients = state.oauth_clients.lock().await;
        clients.register_client(Client::public(&client_id, reg, scope));
    }

    let verification_uri_complete = format!(
        "{base}/oidc/authorize?response_type=code&client_id={}&redirect_uri={}&scope=default&state={}",
        url_escape(&client_id),
        url_escape(&redirect_uri),
        url_escape(&device_code),
    );

    Ok(Json(StartLoginResponse {
        device_code,
        verification_uri_complete,
    }))
}

#[derive(Debug, Deserialize)]
struct CallbackQuery {
    #[allow(dead_code)]
    code: String, // OAuth code - not currently used, but part of OAuth callback spec
    state: String, // device_code
}

async fn oauth_callback(
    Extension(state): Extension<Arc<AppState>>,
    Query(q): Query<CallbackQuery>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    // Bind the OAuth’d user to a previously-verified agent.
    let agent_id = {
        let pending = state.pending_logins.read().await;
        pending
            .get(&q.state)
            .cloned()
            .ok_or_else(|| (StatusCode::BAD_REQUEST, "unknown device_code".to_string()))?
    };

    // Owner identity should have been captured by the OwnerSolicitor keyed by `state`.
    let user_id = state
        .device_users
        .lock()
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "device user map poisoned".to_string()))?
        .get(&q.state)
        .cloned()
        .ok_or_else(|| (StatusCode::UNAUTHORIZED, "missing user identity".to_string()))?;

    // Internal-only: mint double-lock Biscuit
    let token = state
        .token_factory
        .mint_double_lock_token(&agent_id, &user_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Persist for agent polling for 5 minutes.
    let expires_at = unix_now_seconds()
        .checked_add(300)
        .ok_or_else(|| (StatusCode::INTERNAL_SERVER_ERROR, "time overflow".to_string()))?;

    sqlx::query("INSERT OR REPLACE INTO device_tokens (device_code, agent_id, token, expires_at) VALUES (?, ?, ?, ?)")
        .bind(&q.state)
        .bind(&agent_id)
        .bind(&token)
        .bind(expires_at as i64)
        .execute(&state.auth_db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Html("<h2>Login complete</h2><p>You can close this window.</p>".to_string()))
}

#[derive(Debug, Deserialize)]
struct PollQuery {
    device_code: String,
}

async fn poll_device_code(
    Extension(state): Extension<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<PollQuery>,
) -> Result<axum::response::Response, StatusCode> {
    let agent_id = extract_agent_id(&headers).ok_or(StatusCode::UNAUTHORIZED)?;

    let row = sqlx::query_as::<_, (String, String, i64)>(
        "SELECT agent_id, token, expires_at FROM device_tokens WHERE device_code = ?",
    )
    .bind(&q.device_code)
    .fetch_optional(&state.auth_db)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let Some((token_agent_id, token, expires_at)) = row else {
        return Ok((StatusCode::ACCEPTED, Json(json!({"status":"pending"}))).into_response());
    };

    if token_agent_id != agent_id {
        return Err(StatusCode::FORBIDDEN);
    }

    if unix_now_seconds() as i64 > expires_at {
        return Err(StatusCode::UNAUTHORIZED);
    }

    Ok(Json(json!({"status":"ok","token": token})).into_response())
}

// -----------------------------
// Helpers
// -----------------------------

// (SQLite user verification is handled inside the oxide-auth OwnerSolicitor.)

fn extract_agent_id(headers: &HeaderMap) -> Option<String> {
    if let Some(v) = headers.get("x-agent-id").and_then(|h| h.to_str().ok()) {
        let s = v.trim();
        if !s.is_empty() {
            return Some(s.to_string());
        }
    }
    if let Some(v) = headers.get(axum::http::header::AUTHORIZATION).and_then(|h| h.to_str().ok()) {
        // MVP: accept "Bearer agent:<id>"
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

fn server_base_url(cfg: &crate::config::AppConfig) -> String {
    // MVP: assume http and re-use bind port.
    // Production: configure explicitly + support TLS + reverse proxy headers.
    let hostport = cfg.server.bind.clone();
    format!("http://{}", hostport.replace("0.0.0.0", "localhost"))
}

fn unix_now_seconds() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn url_escape(s: &str) -> String {
    // Minimal URL encoding (safe for this MVP query-string usage).
    urlencoding::encode(s).to_string()
}


