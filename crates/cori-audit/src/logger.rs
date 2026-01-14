//! Audit logger implementation.
//!
//! Provides the main `AuditLogger` type with helper methods for logging
//! tool calls, query execution, and approval workflow events.

use cori_core::AuditConfig;
use std::path::PathBuf;
use std::sync::Arc;

use crate::error::AuditError;
use crate::event::{AuditEvent, AuditEventType};
use crate::storage::{AuditStorage, ConsoleStorage, DualStorage, FileStorage, NullStorage};

/// The main audit logger.
///
/// Provides convenient methods for logging different event types
/// following the [role - tenant - action - sql] format.
pub struct AuditLogger {
    config: AuditConfig,
    storage: Arc<dyn AuditStorage>,
}

impl AuditLogger {
    /// Create a new audit logger with the given configuration.
    pub fn new(config: AuditConfig) -> Result<Self, AuditError> {
        let storage: Arc<dyn AuditStorage> = if !config.enabled {
            Arc::new(NullStorage::new())
        } else {
            // Determine file path
            let file_path = Self::resolve_log_path(&config);

            if config.stdout {
                // Dual output: file + console
                Arc::new(DualStorage::new(&file_path)?)
            } else {
                // File only
                Arc::new(FileStorage::new(&file_path)?)
            }
        };

        Ok(Self { config, storage })
    }

    /// Create a logger with a custom storage backend.
    pub fn with_storage(config: AuditConfig, storage: Arc<dyn AuditStorage>) -> Self {
        Self { config, storage }
    }

    /// Create a disabled (no-op) logger.
    pub fn disabled() -> Self {
        Self {
            config: AuditConfig {
                enabled: false,
                ..Default::default()
            },
            storage: Arc::new(NullStorage::new()),
        }
    }

    /// Create a console-only logger (useful for development).
    pub fn console_only() -> Self {
        Self {
            config: AuditConfig {
                enabled: true,
                stdout: true,
                ..Default::default()
            },
            storage: Arc::new(ConsoleStorage::new()),
        }
    }

    /// Resolve the log file path from configuration.
    fn resolve_log_path(config: &AuditConfig) -> PathBuf {
        let directory = &config.directory;
        let mut path = PathBuf::from(directory);
        path.push("audit.log");
        path
    }

    /// Check if logging is enabled.
    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    /// Log an audit event.
    pub async fn log(&self, event: AuditEvent) -> Result<(), AuditError> {
        if !self.config.enabled {
            return Ok(());
        }

        // Also log to tracing for structured logging integration
        tracing::debug!(
            event_id = %event.event_id,
            event_type = %event.event_type,
            tenant = %event.tenant_id,
            role = %event.role,
            action = %event.action,
            "Audit event"
        );

        self.storage.store(event).await
    }

    /// Log a tool call event.
    ///
    /// Use this when an MCP tool is invoked.
    pub async fn log_tool_call(
        &self,
        role: &str,
        tenant_id: &str,
        action: &str,
        sql: Option<&str>,
        dry_run: bool,
    ) -> Result<(), AuditError> {
        let mut builder = AuditEvent::builder(AuditEventType::ToolCalled, role, tenant_id, action);

        if let Some(sql) = sql {
            builder = builder.sql(sql);
        }
        if dry_run {
            builder = builder.dry_run(true);
        }

        self.log(builder.build()).await
    }

    /// Log a query execution event.
    ///
    /// Use this after a query has been executed.
    pub async fn log_query_executed(
        &self,
        role: &str,
        tenant_id: &str,
        action: &str,
        sql: &str,
        row_count: u64,
        duration_ms: u64,
    ) -> Result<(), AuditError> {
        let event = AuditEvent::builder(AuditEventType::QueryExecuted, role, tenant_id, action)
            .sql(sql)
            .row_count(row_count)
            .duration_ms(duration_ms)
            .build();

        self.log(event).await
    }

    /// Log a query failure event.
    pub async fn log_query_failed(
        &self,
        role: &str,
        tenant_id: &str,
        action: &str,
        sql: Option<&str>,
        error: &str,
    ) -> Result<(), AuditError> {
        let mut builder = AuditEvent::builder(AuditEventType::QueryFailed, role, tenant_id, action)
            .error(error);

        if let Some(sql) = sql {
            builder = builder.sql(sql);
        }

        self.log(builder.build()).await
    }

    /// Log an approval request event.
    ///
    /// Use this when an action requires human approval.
    pub async fn log_approval_requested(
        &self,
        role: &str,
        tenant_id: &str,
        action: &str,
        approval_id: &str,
    ) -> Result<(), AuditError> {
        let event = AuditEvent::builder(
            AuditEventType::ApprovalRequested,
            role,
            tenant_id,
            action,
        )
        .approval_id(approval_id)
        .build();

        self.log(event).await
    }

    /// Log an approval granted event.
    pub async fn log_approved(
        &self,
        role: &str,
        tenant_id: &str,
        action: &str,
        approval_id: &str,
        approver: &str,
    ) -> Result<(), AuditError> {
        let event = AuditEvent::builder(AuditEventType::Approved, role, tenant_id, action)
            .approval_id(approval_id)
            .approver(approver)
            .build();

        self.log(event).await
    }

    /// Log an approval denied event.
    pub async fn log_denied(
        &self,
        role: &str,
        tenant_id: &str,
        action: &str,
        approval_id: &str,
        approver: &str,
        reason: Option<&str>,
    ) -> Result<(), AuditError> {
        let mut builder = AuditEvent::builder(AuditEventType::Denied, role, tenant_id, action)
            .approval_id(approval_id)
            .approver(approver);

        if let Some(reason) = reason {
            builder = builder.error(reason);
        }

        self.log(builder.build()).await
    }

    /// Log an authorization denied event.
    pub async fn log_authorization_denied(
        &self,
        role: &str,
        tenant_id: &str,
        action: &str,
        reason: &str,
    ) -> Result<(), AuditError> {
        let event = AuditEvent::builder(
            AuditEventType::AuthorizationDenied,
            role,
            tenant_id,
            action,
        )
        .error(reason)
        .build();

        self.log(event).await
    }

    /// Log an authentication failure event.
    pub async fn log_authentication_failed(
        &self,
        reason: &str,
        client_ip: Option<&str>,
    ) -> Result<(), AuditError> {
        let mut builder = AuditEvent::builder(
            AuditEventType::AuthenticationFailed,
            "unknown",
            "unknown",
            "authenticate",
        )
        .error(reason);

        if let Some(ip) = client_ip {
            builder = builder.client_ip(ip);
        }

        self.log(builder.build()).await
    }

    /// Query audit events with filters.
    pub async fn query(&self, filter: AuditFilter) -> Result<Vec<AuditEvent>, AuditError> {
        self.storage.query(filter).await
    }

    /// Count audit events matching a filter (ignores limit/offset).
    pub async fn count(&self, filter: AuditFilter) -> Result<usize, AuditError> {
        self.storage.count(filter).await
    }

    /// Get an audit event by ID.
    pub async fn get(&self, event_id: uuid::Uuid) -> Result<Option<AuditEvent>, AuditError> {
        self.storage.get(event_id).await
    }

    /// Get recent events for a tenant.
    pub async fn recent_for_tenant(
        &self,
        tenant_id: &str,
        limit: usize,
    ) -> Result<Vec<AuditEvent>, AuditError> {
        self.query(AuditFilter {
            tenant_id: Some(tenant_id.to_string()),
            limit: Some(limit),
            ..Default::default()
        })
        .await
    }

    /// Get recent approval-related events.
    pub async fn recent_approvals(&self, limit: usize) -> Result<Vec<AuditEvent>, AuditError> {
        self.query(AuditFilter {
            event_type: Some(AuditEventType::ApprovalRequested),
            limit: Some(limit),
            ..Default::default()
        })
        .await
    }
}

/// Filter for querying audit events.
#[derive(Debug, Clone, Default)]
pub struct AuditFilter {
    /// Filter by tenant ID.
    pub tenant_id: Option<String>,
    /// Filter by role.
    pub role: Option<String>,
    /// Filter by action/tool name.
    pub action: Option<String>,
    /// Filter by event type.
    pub event_type: Option<AuditEventType>,
    /// Filter by start time.
    pub start_time: Option<chrono::DateTime<chrono::Utc>>,
    /// Filter by end time.
    pub end_time: Option<chrono::DateTime<chrono::Utc>>,
    /// Maximum number of results.
    pub limit: Option<usize>,
    /// Offset for pagination.
    pub offset: Option<usize>,
    /// Sort by field (occurred_at, role, tenant_id, action, duration_ms).
    pub sort_by: Option<String>,
    /// Sort descending (default: true for newest first).
    pub sort_desc: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_disabled_logger() {
        let logger = AuditLogger::disabled();
        assert!(!logger.is_enabled());

        // Should not error even when logging
        logger
            .log_tool_call("admin", "acme", "test", None, false)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_console_only_logger() {
        let logger = AuditLogger::console_only();
        assert!(logger.is_enabled());

        // Should print to console
        logger
            .log_tool_call("support_agent", "client_a", "listOrders", None, false)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_log_tool_call() {
        let logger = AuditLogger::disabled();

        logger
            .log_tool_call(
                "admin",
                "acme",
                "getCustomer",
                Some("SELECT * FROM customers WHERE id = 1"),
                false,
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_log_query_executed() {
        let logger = AuditLogger::disabled();

        logger
            .log_query_executed(
                "support_agent",
                "client_a",
                "listOrders",
                "SELECT * FROM orders WHERE tenant_id = 'client_a'",
                42,
                15,
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_log_approval_workflow() {
        let logger = AuditLogger::disabled();

        // Request approval
        logger
            .log_approval_requested("support_agent", "acme", "deleteOrder", "apr_123")
            .await
            .unwrap();

        // Approve
        logger
            .log_approved(
                "support_agent",
                "acme",
                "deleteOrder",
                "apr_123",
                "admin@example.com",
            )
            .await
            .unwrap();

        // Or deny
        logger
            .log_denied(
                "support_agent",
                "acme",
                "deleteOrder",
                "apr_456",
                "admin@example.com",
                Some("Not authorized for bulk deletions"),
            )
            .await
            .unwrap();
    }
}
