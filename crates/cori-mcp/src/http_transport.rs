//! HTTP transport for MCP server.
//!
//! This module provides an HTTP/SSE transport for the MCP server,
//! allowing remote AI agents and API integrations to connect.

use crate::error::McpError;
use crate::protocol::{JsonRpcRequest, JsonRpcResponse};
use axum::{
    extract::{State, Query},
    http::StatusCode,
    response::{IntoResponse, Sse},
    routing::{get, post},
    Json, Router,
};
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
}

impl HttpTransportState {
    /// Create a new HTTP transport state.
    pub fn new(
        request_tx: mpsc::Sender<(JsonRpcRequest, mpsc::Sender<JsonRpcResponse>)>,
    ) -> Self {
        Self {
            request_tx,
            sse_connections: RwLock::new(HashMap::new()),
        }
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
    Json(request): Json<JsonRpcRequest>,
) -> impl IntoResponse {
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
        );
    }

    // Wait for response
    match response_rx.recv().await {
        Some(response) => (StatusCode::OK, Json(response)),
        None => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(JsonRpcResponse::error(None, -32603, "No response from MCP server")),
        ),
    }
}

/// Handle GET requests to /mcp (SSE streaming).
async fn handle_mcp_sse(
    State(state): State<Arc<HttpTransportState>>,
    Query(query): Query<McpQuery>,
) -> impl IntoResponse {
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
    )
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

    /// Run the HTTP server.
    pub async fn run(self) -> Result<(), McpError> {
        let app = create_router(self.state);

        let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", self.port))
            .await
            .map_err(|e| McpError::StartupFailed(format!("Failed to bind to port {}: {}", self.port, e)))?;

        tracing::info!(port = self.port, "MCP HTTP server listening");

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
}
