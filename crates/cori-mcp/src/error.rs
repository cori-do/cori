//! Error types for the MCP crate.

use thiserror::Error;

/// Errors that can occur in the MCP server.
#[derive(Debug, Error)]
pub enum McpError {
    /// Failed to start the server.
    #[error("failed to start MCP server: {0}")]
    StartupFailed(String),

    /// Invalid request format.
    #[error("invalid request: {0}")]
    InvalidRequest(String),

    /// Tool not found.
    #[error("tool not found: {name}")]
    ToolNotFound { name: String },

    /// Invalid arguments for tool.
    #[error("invalid arguments for tool {tool}: {reason}")]
    InvalidArguments { tool: String, reason: String },

    /// Authentication failed.
    #[error("authentication failed: {0}")]
    AuthenticationFailed(String),

    /// Authorization failed.
    #[error("not authorized to call tool {tool}")]
    NotAuthorized { tool: String },

    /// Action requires approval.
    #[error("action {action} requires approval (id: {approval_id})")]
    ApprovalRequired { action: String, approval_id: String },

    /// Execution failed.
    #[error("execution failed: {0}")]
    ExecutionFailed(String),

    /// Transport error.
    #[error("transport error: {0}")]
    TransportError(String),

    /// Serialization error.
    #[error("serialization error: {0}")]
    SerializationError(#[from] serde_json::Error),

    /// IO error.
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    /// Internal error.
    #[error("internal error: {0}")]
    Internal(#[from] anyhow::Error),
}
