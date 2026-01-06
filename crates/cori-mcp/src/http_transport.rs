//! HTTP transport for MCP server.
//!
//! This module provides an HTTP/SSE transport for the MCP server,
//! allowing remote AI agents and API integrations to connect.
//!
//! ## Authentication
//!
//! All MCP endpoints (except `/health`) require a valid Biscuit token
//! in the `Authorization` header:
//!
//! ```text
//! Authorization: Bearer <base64-encoded-biscuit-token>
//! ```
//!
//! The token must be:
//! - A valid Biscuit token signed with the server's public key
//! - Attenuated to a specific tenant (agent token, not role token)
//! - Not expired

use crate::error::McpError;
use crate::protocol::{JsonRpcRequest, JsonRpcResponse};
use axum::{
    extract::{State, Query},
    http::{header, StatusCode, HeaderMap},
    response::{IntoResponse, Sse},
    routing::{get, post},
    Json, Router,
};
use cori_biscuit::{TokenVerifier, VerifiedToken, PublicKey};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

/// HTTP transport handler state.
pub struct HttpTransportState {
    /// Channel for sending requests to the MCP server.
    request_tx: mpsc::Sender<(JsonRpcRequest, mpsc::Sender<JsonRpcResponse>)>,
    /// Active SSE connections for streaming.
    sse_connections: RwLock<HashMap<String, mpsc::Sender<SseEvent>>>,
    /// Token verifier for authenticating requests.
    token_verifier: Option<TokenVerifier>,
    /// Whether authentication is required (can be disabled for development).
    require_auth: bool,
}

impl HttpTransportState {
    /// Create a new HTTP transport state.
    pub fn new(
        request_tx: mpsc::Sender<(JsonRpcRequest, mpsc::Sender<JsonRpcResponse>)>,
    ) -> Self {
        Self {
            request_tx,
            sse_connections: RwLock::new(HashMap::new()),
            token_verifier: None,
            require_auth: true, // Authentication required by default
        }
    }

    /// Set the token verifier for authentication.
    pub fn with_token_verifier(mut self, verifier: TokenVerifier) -> Self {
        self.token_verifier = Some(verifier);
        self
    }

    /// Set whether authentication is required.
    /// WARNING: Only disable for local development/testing.
    pub fn with_require_auth(mut self, require: bool) -> Self {
        self.require_auth = require;
        if !require {
            tracing::warn!("MCP HTTP authentication disabled - this should only be used for development!");
        }
        self
    }

    /// Verify the token from the Authorization header.
    fn verify_token(&self, headers: &HeaderMap) -> Result<Option<VerifiedToken>, AuthError> {
        // If auth is not required, return None (no verified token)
        if !self.require_auth {
            return Ok(None);
        }

        // Extract Authorization header
        let auth_header = headers
            .get(header::AUTHORIZATION)
            .ok_or(AuthError::MissingToken)?;

        let auth_str = auth_header
            .to_str()
            .map_err(|_| AuthError::InvalidHeader)?;

        // Parse Bearer token
        if !auth_str.starts_with("Bearer ") {
            return Err(AuthError::InvalidHeader);
        }
        let token = &auth_str[7..]; // Skip "Bearer "

        // Verify the token
        let verifier = self.token_verifier.as_ref().ok_or(AuthError::NoVerifier)?;
        let verified = verifier
            .verify(token)
            .map_err(|e| AuthError::InvalidToken(e.to_string()))?;

        // Require attenuated tokens (with tenant) for MCP access
        if verified.tenant.is_none() {
            return Err(AuthError::NotAttenuated);
        }

        Ok(Some(verified))
    }
}

/// Authentication errors.
#[derive(Debug)]
pub enum AuthError {
    /// No Authorization header provided.
    MissingToken,
    /// Invalid Authorization header format.
    InvalidHeader,
    /// Token verification failed.
    InvalidToken(String),
    /// No token verifier configured.
    NoVerifier,
    /// Token is not attenuated (missing tenant).
    NotAttenuated,
}

impl IntoResponse for AuthError {
    fn into_response(self) -> axum::response::Response {
        let (status, message) = match self {
            AuthError::MissingToken => (
                StatusCode::UNAUTHORIZED,
                "Missing Authorization header. Use: Authorization: Bearer <token>",
            ),
            AuthError::InvalidHeader => (
                StatusCode::UNAUTHORIZED,
                "Invalid Authorization header. Expected: Bearer <base64-token>",
            ),
            AuthError::InvalidToken(ref _e) => (
                StatusCode::UNAUTHORIZED,
                "Token verification failed",
            ),
            AuthError::NoVerifier => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Server not configured for authentication",
            ),
            AuthError::NotAttenuated => (
                StatusCode::FORBIDDEN,
                "Token must be attenuated with a tenant. Use 'cori token attenuate' to create an agent token.",
            ),
        };

        let body = serde_json::json!({
            "error": message,
            "code": status.as_u16(),
            "details": match &self {
                AuthError::InvalidToken(e) => Some(e.clone()),
                _ => None,
            }
        });

        (status, Json(body)).into_response()
    }
}

/// SSE event for streaming.
#[derive(Debug, Clone, Serialize)]
pub struct SseEvent {
    pub event: String,
    pub data: serde_json::Value,
}

/// Query parameters for MCP endpoint.
#[derive(Debug, Deserialize)]
pub struct McpQuery {
    /// Session ID for SSE connections.
    session_id: Option<String>,
}

/// Create the HTTP router for MCP.
pub fn create_router(state: Arc<HttpTransportState>) -> Router {
    Router::new()
        .route("/mcp", post(handle_mcp_post))
        .route("/mcp", get(handle_mcp_sse))
        .route("/health", get(handle_health))
        .with_state(state)
}

/// Handle POST requests to /mcp (JSON-RPC over HTTP).
async fn handle_mcp_post(
    State(state): State<Arc<HttpTransportState>>,
    headers: HeaderMap,
    Json(request): Json<JsonRpcRequest>,
) -> impl IntoResponse {
    // Verify authentication
    let verified_token = match state.verify_token(&headers) {
        Ok(token) => token,
        Err(e) => {
            tracing::warn!(error = ?e, "MCP authentication failed");
            return e.into_response();
        }
    };

    // Log the authenticated request
    if let Some(ref token) = verified_token {
        tracing::debug!(
            role = %token.role,
            tenant = ?token.tenant,
            method = %request.method,
            "Authenticated MCP request"
        );
    }

    let (response_tx, mut response_rx) = mpsc::channel(1);

    // Send request to MCP server
    if state.request_tx.send((request, response_tx)).await.is_err() {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(JsonRpcResponse::error(
                None,
                -32603,
                "MCP server unavailable",
            )),
        ).into_response();
    }

    // Wait for response
    match response_rx.recv().await {
        Some(response) => (StatusCode::OK, Json(response)).into_response(),
        None => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(JsonRpcResponse::error(None, -32603, "No response from MCP server")),
        ).into_response(),
    }
}

/// Handle GET requests to /mcp (SSE streaming).
async fn handle_mcp_sse(
    State(state): State<Arc<HttpTransportState>>,
    headers: HeaderMap,
    Query(query): Query<McpQuery>,
) -> impl IntoResponse {
    // Verify authentication
    let verified_token = match state.verify_token(&headers) {
        Ok(token) => token,
        Err(e) => {
            tracing::warn!(error = ?e, "MCP SSE authentication failed");
            return e.into_response();
        }
    };

    // Log the authenticated SSE connection
    if let Some(ref token) = verified_token {
        tracing::debug!(
            role = %token.role,
            tenant = ?token.tenant,
            "Authenticated MCP SSE connection"
        );
    }

    let session_id = query
        .session_id
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    let (event_tx, event_rx) = mpsc::channel(100);

    // Register SSE connection
    state
        .sse_connections
        .write()
        .await
        .insert(session_id.clone(), event_tx);

    // Create SSE stream
    let stream = async_stream::stream! {
        let mut rx = event_rx;
        while let Some(event) = rx.recv().await {
            let data = serde_json::to_string(&event.data).unwrap_or_default();
            yield Ok::<_, Infallible>(axum::response::sse::Event::default()
                .event(event.event)
                .data(data));
        }
    };

    Sse::new(stream).keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(std::time::Duration::from_secs(30))
            .text("ping"),
    ).into_response()
}

/// Handle health check requests.
async fn handle_health() -> impl IntoResponse {
    Json(serde_json::json!({
        "status": "ok",
        "service": "cori-mcp",
        "version": env!("CARGO_PKG_VERSION")
    }))
}

/// HTTP server for MCP transport.
pub struct HttpServer {
    port: u16,
    state: Arc<HttpTransportState>,
}

impl HttpServer {
    /// Create a new HTTP server.
    pub fn new(
        port: u16,
        request_tx: mpsc::Sender<(JsonRpcRequest, mpsc::Sender<JsonRpcResponse>)>,
    ) -> Self {
        Self {
            port,
            state: Arc::new(HttpTransportState::new(request_tx)),
        }
    }

    /// Create an HTTP server with token authentication.
    pub fn with_auth(
        port: u16,
        request_tx: mpsc::Sender<(JsonRpcRequest, mpsc::Sender<JsonRpcResponse>)>,
        public_key: PublicKey,
    ) -> Self {
        let verifier = TokenVerifier::new(public_key);
        Self {
            port,
            state: Arc::new(
                HttpTransportState::new(request_tx)
                    .with_token_verifier(verifier)
            ),
        }
    }

    /// Create an HTTP server without authentication (development only).
    /// 
    /// # Warning
    /// 
    /// This should only be used for local development and testing.
    /// Production deployments MUST use `with_auth`.
    pub fn without_auth(
        port: u16,
        request_tx: mpsc::Sender<(JsonRpcRequest, mpsc::Sender<JsonRpcResponse>)>,
    ) -> Self {
        tracing::warn!(
            port = port,
            "Creating MCP HTTP server WITHOUT authentication - for development only!"
        );
        Self {
            port,
            state: Arc::new(
                HttpTransportState::new(request_tx)
                    .with_require_auth(false)
            ),
        }
    }

    /// Run the HTTP server.
    pub async fn run(self) -> Result<(), McpError> {
        let require_auth = self.state.require_auth;
        let has_verifier = self.state.token_verifier.is_some();
        
        if require_auth && !has_verifier {
            return Err(McpError::StartupFailed(
                "Token authentication required but no public key configured. \
                 Either configure biscuit.public_key_file/public_key_env in cori.yaml, \
                 or explicitly disable auth for development.".to_string()
            ));
        }

        let app = create_router(self.state);

        let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", self.port))
            .await
            .map_err(|e| McpError::StartupFailed(format!("Failed to bind to port {}: {}", self.port, e)))?;

        tracing::info!(
            port = self.port,
            auth_enabled = require_auth,
            "MCP HTTP server listening"
        );

        axum::serve(listener, app)
            .await
            .map_err(|e| McpError::Internal(e.into()))?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    #[tokio::test]
    async fn test_health_endpoint() {
        let (tx, _rx) = mpsc::channel(1);
        let state = Arc::new(HttpTransportState::new(tx));
        let app = create_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_mcp_endpoint_requires_auth() {
        let (tx, _rx) = mpsc::channel(1);
        // Create state with auth required but no verifier
        let state = Arc::new(HttpTransportState::new(tx));
        let app = create_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/mcp")
                    .header("Content-Type", "application/json")
                    .body(Body::from(r#"{"jsonrpc":"2.0","method":"tools/list","id":1}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        // Should fail because no Authorization header
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_mcp_endpoint_without_auth() {
        let (tx, mut rx) = mpsc::channel(1);
        // Create state with auth disabled
        let state = Arc::new(
            HttpTransportState::new(tx)
                .with_require_auth(false)
        );
        let app = create_router(state);

        // Spawn a handler that will respond to the request
        tokio::spawn(async move {
            if let Some((request, response_tx)) = rx.recv().await {
                let response = JsonRpcResponse::success(
                    request.id,
                    serde_json::json!({"tools": []}),
                );
                let _ = response_tx.send(response).await;
            }
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/mcp")
                    .header("Content-Type", "application/json")
                    .body(Body::from(r#"{"jsonrpc":"2.0","method":"tools/list","id":1}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        // Should succeed because auth is disabled
        assert_eq!(response.status(), StatusCode::OK);
    }
}
