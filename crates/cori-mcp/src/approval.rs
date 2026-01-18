//! Human-in-the-loop approval system.
//!
//! This module provides functionality for actions that require human approval
//! before execution. When an action is flagged as `requires_approval`, it goes
//! through this system.
//!
//! ## Approval Flow
//!
//! 1. Agent calls a tool with `requires_approval: true`
//! 2. Cori returns a "pending approval" response with an approval ID
//! 3. Human reviews and approves/rejects via dashboard or CLI
//! 4. Agent polls or receives callback when approved
//! 5. Action executes

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use uuid::Uuid;

/// Status of an approval request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalStatus {
    /// Waiting for human review.
    Pending,
    /// Approved by human.
    Approved,
    /// Rejected by human.
    Rejected,
    /// Expired before decision.
    Expired,
    /// Cancelled by agent.
    Cancelled,
}

/// An approval request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalRequest {
    /// Unique approval ID.
    pub id: String,
    /// Tool being called.
    pub tool_name: String,
    /// Arguments for the tool.
    pub arguments: serde_json::Value,
    /// Fields requiring approval.
    pub approval_fields: Vec<String>,
    /// Current status.
    pub status: ApprovalStatus,
    /// Tenant ID.
    pub tenant_id: String,
    /// Role name.
    pub role: String,
    /// When the request was created.
    pub created_at: DateTime<Utc>,
    /// When the request expires.
    pub expires_at: DateTime<Utc>,
    /// When the request was decided (if any).
    pub decided_at: Option<DateTime<Utc>>,
    /// Who decided (if any).
    pub decided_by: Option<String>,
    /// Reason for decision (if any).
    pub reason: Option<String>,
    /// Whether this is a dry-run request.
    pub is_dry_run: bool,

    // === New fields for data validation and result storage ===
    /// Snapshot of current DB values at request time (for update validation).
    /// Used to ensure data hasn't changed between approval request and execution.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_values: Option<serde_json::Value>,

    /// Table being modified.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_table: Option<String>,

    /// Primary key of row being modified.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_pk: Option<serde_json::Value>,

    /// Execution result (stored after approval and execution).
    /// Clients can poll for this result using getApprovalResult.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_result: Option<serde_json::Value>,

    // === Audit trail fields for hierarchical event linking ===
    /// Audit event ID of the approval request event.
    /// Used for hierarchical linking in audit logs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audit_event_id: Option<uuid::Uuid>,

    /// Correlation ID for the entire workflow.
    /// Links all events in the approval workflow together.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
}

impl ApprovalRequest {
    /// Create a new approval request.
    pub fn new(
        tool_name: impl Into<String>,
        arguments: serde_json::Value,
        approval_fields: Vec<String>,
        tenant_id: impl Into<String>,
        role: impl Into<String>,
        ttl: Duration,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4().to_string(),
            tool_name: tool_name.into(),
            arguments,
            approval_fields,
            status: ApprovalStatus::Pending,
            tenant_id: tenant_id.into(),
            role: role.into(),
            created_at: now,
            expires_at: now + ttl,
            decided_at: None,
            decided_by: None,
            reason: None,
            is_dry_run: false,
            original_values: None,
            target_table: None,
            target_pk: None,
            execution_result: None,
            audit_event_id: None,
            correlation_id: None,
        }
    }

    /// Create a new approval request with a snapshot of original values.
    /// Used for update operations to validate data hasn't changed.
    pub fn with_snapshot(
        tool_name: impl Into<String>,
        arguments: serde_json::Value,
        approval_fields: Vec<String>,
        tenant_id: impl Into<String>,
        role: impl Into<String>,
        ttl: Duration,
        target_table: impl Into<String>,
        target_pk: serde_json::Value,
        original_values: serde_json::Value,
    ) -> Self {
        let mut request = Self::new(tool_name, arguments, approval_fields, tenant_id, role, ttl);
        request.target_table = Some(target_table.into());
        request.target_pk = Some(target_pk);
        request.original_values = Some(original_values);
        request
    }

    /// Set the execution result after approval.
    pub fn set_execution_result(&mut self, result: serde_json::Value) {
        self.execution_result = Some(result);
    }

    /// Set the audit event ID and correlation ID for hierarchical linking.
    pub fn set_audit_ids(&mut self, event_id: uuid::Uuid, correlation_id: String) {
        self.audit_event_id = Some(event_id);
        self.correlation_id = Some(correlation_id);
    }

    /// Check if the request has expired.
    pub fn is_expired(&self) -> bool {
        Utc::now() > self.expires_at
    }

    /// Check if the request is still pending.
    pub fn is_pending(&self) -> bool {
        self.status == ApprovalStatus::Pending && !self.is_expired()
    }

    /// Approve the request.
    pub fn approve(&mut self, by: impl Into<String>, reason: Option<String>) {
        self.status = ApprovalStatus::Approved;
        self.decided_at = Some(Utc::now());
        self.decided_by = Some(by.into());
        self.reason = reason;
    }

    /// Reject the request.
    pub fn reject(&mut self, by: impl Into<String>, reason: Option<String>) {
        self.status = ApprovalStatus::Rejected;
        self.decided_at = Some(Utc::now());
        self.decided_by = Some(by.into());
        self.reason = reason;
    }

    /// Cancel the request.
    pub fn cancel(&mut self) {
        self.status = ApprovalStatus::Cancelled;
        self.decided_at = Some(Utc::now());
    }

    /// Mark as expired.
    pub fn expire(&mut self) {
        self.status = ApprovalStatus::Expired;
        self.decided_at = Some(Utc::now());
    }
}

use crate::approval_storage::{ApprovalFileStorage, ApprovalStorageError};
use std::path::Path;

/// Storage backend for approval requests.
enum ApprovalStorage {
    /// In-memory storage (for testing or when no directory is configured).
    InMemory(Arc<RwLock<HashMap<String, ApprovalRequest>>>),
    /// File-based storage with persistence.
    File(ApprovalFileStorage),
}

/// Manager for approval requests.
///
/// Supports two storage backends:
/// - In-memory (default, for testing)
/// - File-based (persistent, for production)
pub struct ApprovalManager {
    /// Storage backend.
    storage: ApprovalStorage,
    /// Default TTL for approval requests.
    default_ttl: Duration,
}

impl ApprovalManager {
    /// Create a new approval manager with in-memory storage.
    pub fn new(default_ttl: Duration) -> Self {
        Self {
            storage: ApprovalStorage::InMemory(Arc::new(RwLock::new(HashMap::new()))),
            default_ttl,
        }
    }

    /// Create a new approval manager with file-based storage.
    pub fn with_file_storage(
        directory: impl AsRef<Path>,
        default_ttl: Duration,
    ) -> Result<Self, ApprovalStorageError> {
        let file_storage = ApprovalFileStorage::new(directory)?;
        Ok(Self {
            storage: ApprovalStorage::File(file_storage),
            default_ttl,
        })
    }

    /// Get the default TTL.
    pub fn default_ttl(&self) -> Duration {
        self.default_ttl
    }

    /// Create an approval request for a tool call.
    pub fn create_request(
        &self,
        tool_name: impl Into<String>,
        arguments: serde_json::Value,
        approval_fields: Vec<String>,
        tenant_id: impl Into<String>,
        role: impl Into<String>,
    ) -> ApprovalRequest {
        let request = ApprovalRequest::new(
            tool_name,
            arguments,
            approval_fields,
            tenant_id,
            role,
            self.default_ttl,
        );

        match &self.storage {
            ApprovalStorage::InMemory(requests) => {
                let id = request.id.clone();
                requests.write().unwrap().insert(id, request.clone());
            }
            ApprovalStorage::File(storage) => {
                if let Err(e) = storage.store_pending(request.clone()) {
                    tracing::error!("Failed to store pending approval: {}", e);
                }
            }
        }

        request
    }

    /// Create an approval request with a snapshot of original values.
    /// Used for update operations to validate data hasn't changed.
    pub fn create_request_with_snapshot(
        &self,
        tool_name: impl Into<String>,
        arguments: serde_json::Value,
        approval_fields: Vec<String>,
        tenant_id: impl Into<String>,
        role: impl Into<String>,
        target_table: impl Into<String>,
        target_pk: serde_json::Value,
        original_values: serde_json::Value,
    ) -> ApprovalRequest {
        let request = ApprovalRequest::with_snapshot(
            tool_name,
            arguments,
            approval_fields,
            tenant_id,
            role,
            self.default_ttl,
            target_table,
            target_pk,
            original_values,
        );

        match &self.storage {
            ApprovalStorage::InMemory(requests) => {
                let id = request.id.clone();
                requests.write().unwrap().insert(id, request.clone());
            }
            ApprovalStorage::File(storage) => {
                if let Err(e) = storage.store_pending(request.clone()) {
                    tracing::error!("Failed to store pending approval with snapshot: {}", e);
                }
            }
        }

        request
    }

    /// Get an approval request by ID, searching all storage.
    pub fn get(&self, id: &str) -> Option<ApprovalRequest> {
        match &self.storage {
            ApprovalStorage::InMemory(requests) => {
                let mut requests = requests.write().unwrap();
                if let Some(request) = requests.get_mut(id) {
                    // Check for expiration
                    if request.status == ApprovalStatus::Pending && request.is_expired() {
                        request.expire();
                    }
                    return Some(request.clone());
                }
                None
            }
            ApprovalStorage::File(storage) => storage.get(id).ok().flatten(),
        }
    }

    /// List pending approval requests.
    pub fn list_pending(&self, tenant_id: Option<&str>) -> Vec<ApprovalRequest> {
        match &self.storage {
            ApprovalStorage::InMemory(requests) => {
                let mut requests = requests.write().unwrap();

                // First, expire any that are past their TTL
                for request in requests.values_mut() {
                    if request.status == ApprovalStatus::Pending && request.is_expired() {
                        request.expire();
                    }
                }

                requests
                    .values()
                    .filter(|r| {
                        r.status == ApprovalStatus::Pending
                            && tenant_id.map_or(true, |t| r.tenant_id == t)
                    })
                    .cloned()
                    .collect()
            }
            ApprovalStorage::File(storage) => storage.list_pending(tenant_id).unwrap_or_default(),
        }
    }

    /// Approve a request.
    pub fn approve(
        &self,
        id: &str,
        by: impl Into<String>,
        reason: Option<String>,
    ) -> Result<ApprovalRequest, ApprovalError> {
        match &self.storage {
            ApprovalStorage::InMemory(requests) => {
                let mut requests = requests.write().unwrap();

                let request = requests
                    .get_mut(id)
                    .ok_or_else(|| ApprovalError::NotFound(id.to_string()))?;

                if request.is_expired() {
                    request.expire();
                    return Err(ApprovalError::Expired(id.to_string()));
                }

                if request.status != ApprovalStatus::Pending {
                    return Err(ApprovalError::AlreadyDecided(id.to_string()));
                }

                request.approve(by, reason);
                Ok(request.clone())
            }
            ApprovalStorage::File(storage) => storage.approve(id, by, reason),
        }
    }

    /// Reject a request.
    pub fn reject(
        &self,
        id: &str,
        by: impl Into<String>,
        reason: Option<String>,
    ) -> Result<ApprovalRequest, ApprovalError> {
        match &self.storage {
            ApprovalStorage::InMemory(requests) => {
                let mut requests = requests.write().unwrap();

                let request = requests
                    .get_mut(id)
                    .ok_or_else(|| ApprovalError::NotFound(id.to_string()))?;

                if request.is_expired() {
                    request.expire();
                    return Err(ApprovalError::Expired(id.to_string()));
                }

                if request.status != ApprovalStatus::Pending {
                    return Err(ApprovalError::AlreadyDecided(id.to_string()));
                }

                request.reject(by, reason);
                Ok(request.clone())
            }
            ApprovalStorage::File(storage) => storage.reject(id, by, reason),
        }
    }

    /// Cancel a request.
    pub fn cancel(&self, id: &str) -> Result<ApprovalRequest, ApprovalError> {
        match &self.storage {
            ApprovalStorage::InMemory(requests) => {
                let mut requests = requests.write().unwrap();

                let request = requests
                    .get_mut(id)
                    .ok_or_else(|| ApprovalError::NotFound(id.to_string()))?;

                if request.status != ApprovalStatus::Pending {
                    return Err(ApprovalError::AlreadyDecided(id.to_string()));
                }

                request.cancel();
                Ok(request.clone())
            }
            ApprovalStorage::File(storage) => storage.cancel(id),
        }
    }

    /// Update an approved request with execution result.
    pub fn update_with_result(
        &self,
        id: &str,
        result: serde_json::Value,
    ) -> Result<(), ApprovalError> {
        match &self.storage {
            ApprovalStorage::InMemory(requests) => {
                let mut requests = requests.write().unwrap();
                let request = requests
                    .get_mut(id)
                    .ok_or_else(|| ApprovalError::NotFound(id.to_string()))?;
                request.execution_result = Some(result);
                Ok(())
            }
            ApprovalStorage::File(storage) => storage
                .update_with_result(id, result)
                .map_err(|_| ApprovalError::NotFound(id.to_string())),
        }
    }

    /// Update an approval request with audit event ID and correlation ID.
    pub fn update_audit_ids(
        &self,
        id: &str,
        event_id: uuid::Uuid,
        correlation_id: String,
    ) -> Result<(), ApprovalError> {
        match &self.storage {
            ApprovalStorage::InMemory(requests) => {
                let mut requests = requests.write().unwrap();
                let request = requests
                    .get_mut(id)
                    .ok_or_else(|| ApprovalError::NotFound(id.to_string()))?;
                request.set_audit_ids(event_id, correlation_id);
                Ok(())
            }
            ApprovalStorage::File(storage) => storage
                .update_audit_ids(id, event_id, correlation_id)
                .map_err(|_| ApprovalError::NotFound(id.to_string())),
        }
    }

    /// Clean up expired and old requests.
    pub fn cleanup(&self, max_age: Duration) {
        match &self.storage {
            ApprovalStorage::InMemory(requests) => {
                let cutoff = Utc::now() - max_age;
                let mut requests = requests.write().unwrap();
                requests.retain(|_, r| {
                    // Keep if created recently OR if pending
                    r.created_at > cutoff || r.status == ApprovalStatus::Pending
                });
            }
            ApprovalStorage::File(storage) => {
                let _ = storage.cleanup(max_age);
            }
        }
    }
}

impl Default for ApprovalManager {
    fn default() -> Self {
        Self::new(Duration::hours(24))
    }
}

/// Errors that can occur with approvals.
#[derive(Debug, thiserror::Error)]
pub enum ApprovalError {
    #[error("Approval request not found: {0}")]
    NotFound(String),

    #[error("Approval request expired: {0}")]
    Expired(String),

    #[error("Approval request already decided: {0}")]
    AlreadyDecided(String),
}

/// Response for a pending approval.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalPendingResponse {
    pub status: String,
    #[serde(rename = "approvalId")]
    pub approval_id: String,
    pub message: String,
    #[serde(rename = "expiresAt")]
    pub expires_at: DateTime<Utc>,
    #[serde(rename = "approvalFields")]
    pub approval_fields: Vec<String>,
}

impl From<&ApprovalRequest> for ApprovalPendingResponse {
    fn from(request: &ApprovalRequest) -> Self {
        Self {
            status: "pending_approval".to_string(),
            approval_id: request.id.clone(),
            message: format!(
                "Action '{}' requires human approval. Approval ID: {}",
                request.tool_name, request.id
            ),
            expires_at: request.expires_at,
            approval_fields: request.approval_fields.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_approval_request() {
        let manager = ApprovalManager::new(Duration::hours(1));

        let request = manager.create_request(
            "updateTicket",
            serde_json::json!({"id": 1, "priority": "critical"}),
            vec!["priority".to_string()],
            "tenant_a",
            "support_agent",
        );

        assert_eq!(request.status, ApprovalStatus::Pending);
        assert!(request.is_pending());
        assert!(!request.is_expired());
    }

    #[test]
    fn test_approve_request() {
        let manager = ApprovalManager::new(Duration::hours(1));

        let request = manager.create_request(
            "updateTicket",
            serde_json::json!({}),
            vec![],
            "tenant_a",
            "agent",
        );

        let approved = manager
            .approve(&request.id, "admin", Some("Looks good".to_string()))
            .unwrap();

        assert_eq!(approved.status, ApprovalStatus::Approved);
        assert_eq!(approved.decided_by, Some("admin".to_string()));
    }

    #[test]
    fn test_reject_request() {
        let manager = ApprovalManager::new(Duration::hours(1));

        let request = manager.create_request(
            "updateTicket",
            serde_json::json!({}),
            vec![],
            "tenant_a",
            "agent",
        );

        let rejected = manager
            .reject(&request.id, "admin", Some("Not allowed".to_string()))
            .unwrap();

        assert_eq!(rejected.status, ApprovalStatus::Rejected);
    }

    #[test]
    fn test_expired_request() {
        let manager = ApprovalManager::new(Duration::seconds(-1)); // Already expired

        let request = manager.create_request(
            "updateTicket",
            serde_json::json!({}),
            vec![],
            "tenant_a",
            "agent",
        );

        let result = manager.approve(&request.id, "admin", None);
        assert!(matches!(result, Err(ApprovalError::Expired(_))));
    }
}
