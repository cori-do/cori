//! Audit event types.
//!
//! Provides structured audit events for logging database operations.
//! Format follows: [role - tenant - action - sql] with approval workflow tracking.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Type of audit event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditEventType {
    // ===== Tool/Query events =====
    /// MCP tool was called.
    ToolCalled,
    /// Query was executed successfully.
    QueryExecuted,
    /// Query execution failed.
    QueryFailed,

    // ===== Approval workflow (approval_request / approved / denied) =====
    /// Action requires approval (approval_request).
    ApprovalRequested,
    /// Action was approved.
    Approved,
    /// Action was denied/rejected.
    Denied,

    // ===== Auth events =====
    /// Token verification failed.
    AuthenticationFailed,
    /// Query was blocked by policy.
    AuthorizationDenied,
}

impl std::fmt::Display for AuditEventType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ToolCalled => write!(f, "TOOL_CALLED"),
            Self::QueryExecuted => write!(f, "QUERY_EXECUTED"),
            Self::QueryFailed => write!(f, "QUERY_FAILED"),
            Self::ApprovalRequested => write!(f, "APPROVAL_REQUESTED"),
            Self::Approved => write!(f, "APPROVED"),
            Self::Denied => write!(f, "DENIED"),
            Self::AuthenticationFailed => write!(f, "AUTH_FAILED"),
            Self::AuthorizationDenied => write!(f, "AUTHZ_DENIED"),
        }
    }
}

/// An audit event.
///
/// Core fields follow the format: [role - tenant - action - sql]
/// with additional approval workflow fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEvent {
    /// Unique event ID.
    pub event_id: Uuid,

    /// When the event occurred.
    pub occurred_at: DateTime<Utc>,

    /// Event type.
    pub event_type: AuditEventType,

    // ===== Core fields: [role - tenant - action - sql] =====
    /// Role name (required for meaningful audit).
    pub role: String,

    /// Tenant ID (required for multi-tenant isolation).
    pub tenant_id: String,

    /// Action/tool name (e.g., "listCustomers", "updateTicket").
    pub action: String,

    /// Generated SQL query (if applicable).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sql: Option<String>,

    // ===== Approval workflow fields =====
    /// Approval ID (for approval-related events).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approval_id: Option<String>,

    /// Who approved/denied (for Approved/Denied events).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approver: Option<String>,

    // ===== Execution details =====
    /// Number of rows affected/returned.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub row_count: Option<u64>,

    /// Duration in milliseconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,

    /// Error message (if event_type indicates failure).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,

    /// Whether this was a dry-run/preview.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dry_run: Option<bool>,

    // ===== Context =====
    /// Tables accessed by the query.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tables: Option<Vec<String>>,

    /// Connection ID (for correlation).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connection_id: Option<String>,

    /// Client IP address.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_ip: Option<String>,

    /// Additional metadata.
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub meta: serde_json::Value,

    // ===== Mutation audit fields (for tracking changes) =====
    /// Tool arguments provided in the request.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<serde_json::Value>,

    /// State before mutation (for update/delete operations).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before_state: Option<serde_json::Value>,

    /// State after mutation (result of the operation).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after_state: Option<serde_json::Value>,

    /// Diff showing what changed (computed from before_state and after_state).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diff: Option<serde_json::Value>,

    // ===== Hierarchical linking fields =====
    /// Parent event ID for hierarchical linking.
    /// - For Approved/Denied events: points to the ApprovalRequested event
    /// - For QueryExecuted (post-approval): points to the Approved event
    /// - For QueryExecuted (direct): points to the ToolCalled event
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_event_id: Option<Uuid>,

    /// Correlation ID to group all events in a workflow.
    /// All events in an approval workflow share the same correlation_id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
}

impl AuditEvent {
    /// Create a new audit event with the given type and core fields.
    pub fn new(
        event_type: AuditEventType,
        role: impl Into<String>,
        tenant_id: impl Into<String>,
        action: impl Into<String>,
    ) -> Self {
        Self {
            event_id: Uuid::new_v4(),
            occurred_at: Utc::now(),
            event_type,
            role: role.into(),
            tenant_id: tenant_id.into(),
            action: action.into(),
            sql: None,
            approval_id: None,
            approver: None,
            row_count: None,
            duration_ms: None,
            error: None,
            dry_run: None,
            tables: None,
            connection_id: None,
            client_ip: None,
            meta: serde_json::Value::Null,
            arguments: None,
            before_state: None,
            after_state: None,
            diff: None,
            parent_event_id: None,
            correlation_id: None,
        }
    }

    /// Create a builder for an audit event.
    pub fn builder(
        event_type: AuditEventType,
        role: impl Into<String>,
        tenant_id: impl Into<String>,
        action: impl Into<String>,
    ) -> AuditEventBuilder {
        AuditEventBuilder::new(event_type, role, tenant_id, action)
    }

    /// Format the event as a human-readable log line.
    ///
    /// Format: `[timestamp] EVENT_TYPE role=... tenant=... action=... [sql=...]`
    pub fn to_log_line(&self) -> String {
        let mut line = format!(
            "[{}] {} role={} tenant={} action={}",
            self.occurred_at.format("%Y-%m-%dT%H:%M:%S%.3fZ"),
            self.event_type,
            self.role,
            self.tenant_id,
            self.action,
        );

        if let Some(ref sql) = self.sql {
            // Truncate long SQL for console output
            let sql_preview = if sql.len() > 100 {
                format!("{}...", &sql[..100])
            } else {
                sql.clone()
            };
            line.push_str(&format!(" sql=\"{}\"", sql_preview.replace('\n', " ")));
        }

        if let Some(ref approval_id) = self.approval_id {
            line.push_str(&format!(" approval_id={}", approval_id));
        }

        if let Some(ref approver) = self.approver {
            line.push_str(&format!(" approver={}", approver));
        }

        if let Some(row_count) = self.row_count {
            line.push_str(&format!(" rows={}", row_count));
        }

        if let Some(duration) = self.duration_ms {
            line.push_str(&format!(" duration_ms={}", duration));
        }

        if let Some(ref error) = self.error {
            line.push_str(&format!(" error=\"{}\"", error.replace('"', "'")));
        }

        if self.dry_run == Some(true) {
            line.push_str(" dry_run=true");
        }

        // Include diff summary if present
        if let Some(ref diff) = self.diff {
            if let Some(obj) = diff.as_object() {
                let changed_fields: Vec<&str> = obj.keys().map(|k| k.as_str()).collect();
                if !changed_fields.is_empty() {
                    line.push_str(&format!(" changed_fields=[{}]", changed_fields.join(",")));
                }
            }
        }

        line
    }
}

/// Builder for creating audit events.
#[derive(Debug)]
pub struct AuditEventBuilder {
    event: AuditEvent,
}

impl AuditEventBuilder {
    /// Create a new builder with required fields.
    pub fn new(
        event_type: AuditEventType,
        role: impl Into<String>,
        tenant_id: impl Into<String>,
        action: impl Into<String>,
    ) -> Self {
        Self {
            event: AuditEvent::new(event_type, role, tenant_id, action),
        }
    }

    /// Set the SQL query.
    pub fn sql(mut self, sql: impl Into<String>) -> Self {
        self.event.sql = Some(sql.into());
        self
    }

    /// Set the approval ID.
    pub fn approval_id(mut self, id: impl Into<String>) -> Self {
        self.event.approval_id = Some(id.into());
        self
    }

    /// Set the approver.
    pub fn approver(mut self, approver: impl Into<String>) -> Self {
        self.event.approver = Some(approver.into());
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

    /// Set the dry-run flag.
    pub fn dry_run(mut self, is_dry_run: bool) -> Self {
        self.event.dry_run = Some(is_dry_run);
        self
    }

    /// Set the tables accessed.
    pub fn tables(mut self, tables: Vec<String>) -> Self {
        self.event.tables = Some(tables);
        self
    }

    /// Set the connection ID.
    pub fn connection_id(mut self, id: impl Into<String>) -> Self {
        self.event.connection_id = Some(id.into());
        self
    }

    /// Set the client IP.
    pub fn client_ip(mut self, ip: impl Into<String>) -> Self {
        self.event.client_ip = Some(ip.into());
        self
    }

    /// Set additional metadata.
    pub fn meta(mut self, meta: serde_json::Value) -> Self {
        self.event.meta = meta;
        self
    }

    /// Set the tool arguments.
    pub fn arguments(mut self, args: serde_json::Value) -> Self {
        self.event.arguments = Some(args);
        self
    }

    /// Set the state before mutation.
    pub fn before_state(mut self, state: serde_json::Value) -> Self {
        self.event.before_state = Some(state);
        self
    }

    /// Set the state after mutation.
    pub fn after_state(mut self, state: serde_json::Value) -> Self {
        self.event.after_state = Some(state);
        self
    }

    /// Set the diff showing what changed.
    pub fn diff(mut self, diff: serde_json::Value) -> Self {
        self.event.diff = Some(diff);
        self
    }

    /// Compute and set the diff from before_state and after_state.
    /// The diff shows fields that changed, with old and new values.
    pub fn compute_diff(mut self) -> Self {
        if let (Some(before), Some(after)) = (&self.event.before_state, &self.event.after_state) {
            let diff = compute_json_diff(before, after);
            if !diff.is_null() {
                self.event.diff = Some(diff);
            }
        }
        self
    }

    /// Set the parent event ID for hierarchical linking.
    pub fn parent_event_id(mut self, parent_id: Uuid) -> Self {
        self.event.parent_event_id = Some(parent_id);
        self
    }

    /// Set the correlation ID for workflow grouping.
    pub fn correlation_id(mut self, correlation_id: impl Into<String>) -> Self {
        self.event.correlation_id = Some(correlation_id.into());
        self
    }

    /// Build the audit event.
    pub fn build(self) -> AuditEvent {
        self.event
    }
}

/// Compute a diff between two JSON values.
/// Returns an object with changed fields showing { "old": ..., "new": ... }
pub fn compute_json_diff(before: &serde_json::Value, after: &serde_json::Value) -> serde_json::Value {
    use serde_json::{json, Map, Value};

    match (before, after) {
        (Value::Object(before_obj), Value::Object(after_obj)) => {
            let mut diff = Map::new();

            // Check all keys in before
            for (key, before_val) in before_obj {
                match after_obj.get(key) {
                    Some(after_val) if before_val != after_val => {
                        diff.insert(
                            key.clone(),
                            json!({
                                "old": before_val,
                                "new": after_val
                            }),
                        );
                    }
                    None => {
                        diff.insert(
                            key.clone(),
                            json!({
                                "old": before_val,
                                "new": null
                            }),
                        );
                    }
                    _ => {} // No change
                }
            }

            // Check for new keys in after
            for (key, after_val) in after_obj {
                if !before_obj.contains_key(key) {
                    diff.insert(
                        key.clone(),
                        json!({
                            "old": null,
                            "new": after_val
                        }),
                    );
                }
            }

            if diff.is_empty() {
                Value::Null
            } else {
                Value::Object(diff)
            }
        }
        _ => {
            // For non-object values, return simple diff if different
            if before != after {
                json!({
                    "old": before,
                    "new": after
                })
            } else {
                Value::Null
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_builder() {
        let event = AuditEvent::builder(
            AuditEventType::QueryExecuted,
            "support_agent",
            "client_a",
            "listOrders",
        )
        .sql("SELECT * FROM orders WHERE tenant_id = 'client_a'")
        .row_count(42)
        .duration_ms(15)
        .build();

        assert_eq!(event.event_type, AuditEventType::QueryExecuted);
        assert_eq!(event.tenant_id, "client_a");
        assert_eq!(event.role, "support_agent");
        assert_eq!(event.action, "listOrders");
        assert_eq!(event.row_count, Some(42));
    }

    #[test]
    fn test_to_log_line() {
        let event = AuditEvent::builder(
            AuditEventType::ToolCalled,
            "admin",
            "acme",
            "updateCustomer",
        )
        .sql("UPDATE customers SET name = 'New Name' WHERE id = 1")
        .row_count(1)
        .build();

        let log_line = event.to_log_line();
        assert!(log_line.contains("TOOL_CALLED"));
        assert!(log_line.contains("role=admin"));
        assert!(log_line.contains("tenant=acme"));
        assert!(log_line.contains("action=updateCustomer"));
        assert!(log_line.contains("sql="));
    }

    #[test]
    fn test_approval_event() {
        let event = AuditEvent::builder(
            AuditEventType::ApprovalRequested,
            "support_agent",
            "client_a",
            "deleteOrder",
        )
        .approval_id("apr_123")
        .build();

        assert_eq!(event.event_type, AuditEventType::ApprovalRequested);
        assert_eq!(event.approval_id, Some("apr_123".to_string()));

        // Simulate approval
        let approved_event = AuditEvent::builder(
            AuditEventType::Approved,
            "support_agent",
            "client_a",
            "deleteOrder",
        )
        .approval_id("apr_123")
        .approver("admin@example.com")
        .build();

        assert_eq!(approved_event.event_type, AuditEventType::Approved);
        assert_eq!(approved_event.approver, Some("admin@example.com".to_string()));
    }

    #[test]
    fn test_event_type_display() {
        assert_eq!(format!("{}", AuditEventType::ToolCalled), "TOOL_CALLED");
        assert_eq!(format!("{}", AuditEventType::ApprovalRequested), "APPROVAL_REQUESTED");
        assert_eq!(format!("{}", AuditEventType::Approved), "APPROVED");
        assert_eq!(format!("{}", AuditEventType::Denied), "DENIED");
    }
}

