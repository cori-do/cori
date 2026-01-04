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
        }
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

/// Manager for approval requests.
pub struct ApprovalManager {
    /// In-memory store of requests (in production, use persistent storage).
    requests: Arc<RwLock<HashMap<String, ApprovalRequest>>>,
    /// Default TTL for approval requests.
    default_ttl: Duration,
}

impl ApprovalManager {
    /// Create a new approval manager.
    pub fn new(default_ttl: Duration) -> Self {
        Self {
            requests: Arc::new(RwLock::new(HashMap::new())),
            default_ttl,
        }
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

        let id = request.id.clone();
        self.requests
            .write()
            .unwrap()
            .insert(id, request.clone());

        request
    }

    /// Get an approval request by ID.
    pub fn get(&self, id: &str) -> Option<ApprovalRequest> {
        let mut requests = self.requests.write().unwrap();

        if let Some(request) = requests.get_mut(id) {
            // Check for expiration
            if request.status == ApprovalStatus::Pending && request.is_expired() {
                request.expire();
            }
            return Some(request.clone());
        }

        None
    }

    /// List pending approval requests.
    pub fn list_pending(&self, tenant_id: Option<&str>) -> Vec<ApprovalRequest> {
        let mut requests = self.requests.write().unwrap();

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

    /// Approve a request.
    pub fn approve(
        &self,
        id: &str,
        by: impl Into<String>,
        reason: Option<String>,
    ) -> Result<ApprovalRequest, ApprovalError> {
        let mut requests = self.requests.write().unwrap();

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

    /// Reject a request.
    pub fn reject(
        &self,
        id: &str,
        by: impl Into<String>,
        reason: Option<String>,
    ) -> Result<ApprovalRequest, ApprovalError> {
        let mut requests = self.requests.write().unwrap();

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

    /// Cancel a request.
    pub fn cancel(&self, id: &str) -> Result<ApprovalRequest, ApprovalError> {
        let mut requests = self.requests.write().unwrap();

        let request = requests
            .get_mut(id)
            .ok_or_else(|| ApprovalError::NotFound(id.to_string()))?;

        if request.status != ApprovalStatus::Pending {
            return Err(ApprovalError::AlreadyDecided(id.to_string()));
        }

        request.cancel();
        Ok(request.clone())
    }

    /// Clean up expired and old requests.
    pub fn cleanup(&self, max_age: Duration) {
        let cutoff = Utc::now() - max_age;
        let mut requests = self.requests.write().unwrap();

        requests.retain(|_, r| {
            // Keep if created recently OR if pending
            r.created_at > cutoff || r.status == ApprovalStatus::Pending
        });
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
