//! Audit event types.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Type of audit event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditEventType {
    /// A query was received from a client.
    QueryReceived,
    /// RLS predicates were injected into the query.
    QueryRewritten,
    /// Query was executed successfully.
    QueryExecuted,
    /// Query execution failed.
    QueryFailed,
    /// Token verification failed.
    AuthenticationFailed,
    /// Query was blocked by policy.
    AuthorizationDenied,
    /// MCP tool was called.
    ToolCalled,
    /// Action requires approval.
    ApprovalRequired,
    /// Action was approved.
    ActionApproved,
    /// Action was rejected.
    ActionRejected,
    /// Connection established.
    ConnectionOpened,
    /// Connection closed.
    ConnectionClosed,
}

/// An audit event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEvent {
    /// Unique event ID.
    pub event_id: Uuid,

    /// When the event occurred.
    pub occurred_at: DateTime<Utc>,

    /// Sequence number (for ordering within a connection).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sequence: Option<u64>,

    /// Event type.
    pub event_type: AuditEventType,

    /// Tenant ID (if known).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tenant_id: Option<String>,

    /// Role name (if known).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,

    /// The original query (if applicable).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_query: Option<String>,

    /// The rewritten query with RLS predicates (if applicable).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rewritten_query: Option<String>,

    /// Tables accessed by the query.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tables: Option<Vec<String>>,

    /// Number of rows affected/returned.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub row_count: Option<u64>,

    /// Duration in milliseconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,

    /// Error message (if event_type indicates failure).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,

    /// Client IP address.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_ip: Option<String>,

    /// Connection ID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connection_id: Option<String>,

    /// MCP tool name (for ToolCalled events).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,

    /// Approval ID (for approval-related events).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approval_id: Option<String>,

    /// Integrity hash (for tamper-evident logging).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,

    /// Previous event hash (for chaining).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prev_hash: Option<String>,

    /// Additional metadata.
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub meta: serde_json::Value,
}

impl AuditEvent {
    /// Create a new audit event with the given type.
    pub fn new(event_type: AuditEventType) -> Self {
        Self {
            event_id: Uuid::new_v4(),
            occurred_at: Utc::now(),
            sequence: None,
            event_type,
            tenant_id: None,
            role: None,
            original_query: None,
            rewritten_query: None,
            tables: None,
            row_count: None,
            duration_ms: None,
            error: None,
            client_ip: None,
            connection_id: None,
            tool_name: None,
            approval_id: None,
            hash: None,
            prev_hash: None,
            meta: serde_json::Value::Null,
        }
    }

    /// Create a builder for an audit event.
    pub fn builder(event_type: AuditEventType) -> AuditEventBuilder {
        AuditEventBuilder::new(event_type)
    }
}

/// Builder for creating audit events.
#[derive(Debug)]
pub struct AuditEventBuilder {
    event: AuditEvent,
}

impl AuditEventBuilder {
    /// Create a new builder.
    pub fn new(event_type: AuditEventType) -> Self {
        Self {
            event: AuditEvent::new(event_type),
        }
    }

    /// Set the tenant ID.
    pub fn tenant(mut self, tenant_id: impl Into<String>) -> Self {
        self.event.tenant_id = Some(tenant_id.into());
        self
    }

    /// Set the role.
    pub fn role(mut self, role: impl Into<String>) -> Self {
        self.event.role = Some(role.into());
        self
    }

    /// Set the original query.
    pub fn original_query(mut self, query: impl Into<String>) -> Self {
        self.event.original_query = Some(query.into());
        self
    }

    /// Set the rewritten query.
    pub fn rewritten_query(mut self, query: impl Into<String>) -> Self {
        self.event.rewritten_query = Some(query.into());
        self
    }

    /// Set the tables accessed.
    pub fn tables(mut self, tables: Vec<String>) -> Self {
        self.event.tables = Some(tables);
        self
    }

    /// Set the row count.
    pub fn row_count(mut self, count: u64) -> Self {
        self.event.row_count = Some(count);
        self
    }

    /// Set the duration in milliseconds.
    pub fn duration_ms(mut self, duration: u64) -> Self {
        self.event.duration_ms = Some(duration);
        self
    }

    /// Set the error message.
    pub fn error(mut self, error: impl Into<String>) -> Self {
        self.event.error = Some(error.into());
        self
    }

    /// Set the client IP.
    pub fn client_ip(mut self, ip: impl Into<String>) -> Self {
        self.event.client_ip = Some(ip.into());
        self
    }

    /// Set the connection ID.
    pub fn connection_id(mut self, id: impl Into<String>) -> Self {
        self.event.connection_id = Some(id.into());
        self
    }

    /// Set the tool name.
    pub fn tool_name(mut self, name: impl Into<String>) -> Self {
        self.event.tool_name = Some(name.into());
        self
    }

    /// Build the audit event.
    pub fn build(self) -> AuditEvent {
        self.event
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_builder() {
        let event = AuditEvent::builder(AuditEventType::QueryExecuted)
            .tenant("client_a")
            .role("support_agent")
            .original_query("SELECT * FROM orders")
            .rewritten_query("SELECT * FROM orders WHERE tenant_id = 'client_a'")
            .tables(vec!["orders".to_string()])
            .row_count(42)
            .duration_ms(15)
            .build();

        assert_eq!(event.event_type, AuditEventType::QueryExecuted);
        assert_eq!(event.tenant_id, Some("client_a".to_string()));
        assert_eq!(event.role, Some("support_agent".to_string()));
        assert_eq!(event.row_count, Some(42));
    }
}

