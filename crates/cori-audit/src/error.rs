//! Error types for the audit crate.

use thiserror::Error;

/// Errors that can occur during audit operations.
#[derive(Debug, Error)]
pub enum AuditError {
    /// Failed to initialize the audit logger.
    #[error("failed to initialize audit logger: {0}")]
    InitializationFailed(String),

    /// Failed to log an event.
    #[error("failed to log audit event: {0}")]
    LogFailed(String),

    /// Failed to query audit events.
    #[error("failed to query audit events: {0}")]
    QueryFailed(String),

    /// Storage error.
    #[error("storage error: {0}")]
    StorageError(String),

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
