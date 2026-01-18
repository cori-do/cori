//! File-based approval storage.
//!
//! This module provides persistent storage for approval requests using three JSON Lines files:
//! - `pending.log` - requests awaiting decision
//! - `approved.log` - approved requests (audit trail with execution results)
//! - `denied.log` - rejected requests (audit trail with denial reasons)
//!
//! Each file uses JSON Lines format (one JSON object per line) for easy parsing
//! and compatibility with standard tools (jq, grep, etc.).

use crate::approval::{ApprovalError, ApprovalRequest, ApprovalStatus};
use chrono::{Duration, Utc};
use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::RwLock;

/// File-based approval storage with three-file architecture.
pub struct ApprovalFileStorage {
    /// Directory containing the approval files.
    directory: PathBuf,
    /// In-memory cache of pending requests (keyed by ID).
    pending: RwLock<HashMap<String, ApprovalRequest>>,
    /// In-memory cache of approved requests (keyed by ID).
    approved: RwLock<HashMap<String, ApprovalRequest>>,
    /// In-memory cache of denied requests (keyed by ID).
    denied: RwLock<HashMap<String, ApprovalRequest>>,
    /// Maximum number of decided requests to keep in memory cache.
    max_cache_size: usize,
}

impl ApprovalFileStorage {
    /// Create a new file storage with the given directory.
    ///
    /// This will create the directory if it doesn't exist and load
    /// any existing approvals from the files.
    pub fn new(directory: impl AsRef<Path>) -> Result<Self, ApprovalStorageError> {
        let directory = directory.as_ref().to_path_buf();

        // Ensure directory exists
        if !directory.exists() {
            fs::create_dir_all(&directory)?;
        }

        let storage = Self {
            directory,
            pending: RwLock::new(HashMap::new()),
            approved: RwLock::new(HashMap::new()),
            denied: RwLock::new(HashMap::new()),
            max_cache_size: 1000,
        };

        // Load existing data from files
        storage.load_all()?;

        Ok(storage)
    }

    /// Get the path to the pending.log file.
    fn pending_path(&self) -> PathBuf {
        self.directory.join("pending.log")
    }

    /// Get the path to the approved.log file.
    fn approved_path(&self) -> PathBuf {
        self.directory.join("approved.log")
    }

    /// Get the path to the denied.log file.
    fn denied_path(&self) -> PathBuf {
        self.directory.join("denied.log")
    }

    /// Load all approvals from files into memory.
    fn load_all(&self) -> Result<(), ApprovalStorageError> {
        // Load pending
        let pending_requests = Self::load_from_file(&self.pending_path())?;
        let mut pending = self
            .pending
            .write()
            .map_err(|_| ApprovalStorageError::LockError)?;
        for request in pending_requests {
            pending.insert(request.id.clone(), request);
        }
        tracing::info!("Loaded {} pending approval requests", pending.len());

        // Load approved
        let approved_requests = Self::load_from_file(&self.approved_path())?;
        let mut approved = self
            .approved
            .write()
            .map_err(|_| ApprovalStorageError::LockError)?;
        for request in approved_requests {
            approved.insert(request.id.clone(), request);
        }
        tracing::info!("Loaded {} approved approval requests", approved.len());

        // Load denied
        let denied_requests = Self::load_from_file(&self.denied_path())?;
        let mut denied = self
            .denied
            .write()
            .map_err(|_| ApprovalStorageError::LockError)?;
        for request in denied_requests {
            denied.insert(request.id.clone(), request);
        }
        tracing::info!("Loaded {} denied approval requests", denied.len());

        Ok(())
    }

    /// Load approval requests from a JSON Lines file.
    fn load_from_file(path: &Path) -> Result<Vec<ApprovalRequest>, ApprovalStorageError> {
        if !path.exists() {
            return Ok(Vec::new());
        }

        let file = fs::File::open(path)?;
        let reader = BufReader::new(file);
        let mut requests = Vec::new();

        for (line_num, line) in reader.lines().enumerate() {
            let line = line?;
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            match serde_json::from_str::<ApprovalRequest>(line) {
                Ok(request) => requests.push(request),
                Err(e) => {
                    tracing::warn!(
                        "Failed to parse approval request on line {} of {}: {}",
                        line_num + 1,
                        path.display(),
                        e
                    );
                }
            }
        }

        Ok(requests)
    }

    /// Append a request to a file.
    fn append_to_file(path: &Path, request: &ApprovalRequest) -> Result<(), ApprovalStorageError> {
        let json = serde_json::to_string(request)?;
        let mut file = OpenOptions::new().create(true).append(true).open(path)?;
        writeln!(file, "{}", json)?;
        Ok(())
    }

    /// Rewrite a file with the given requests (used when removing from pending).
    fn rewrite_file(
        path: &Path,
        requests: &HashMap<String, ApprovalRequest>,
    ) -> Result<(), ApprovalStorageError> {
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(path)?;

        for request in requests.values() {
            let json = serde_json::to_string(request)?;
            writeln!(file, "{}", json)?;
        }

        Ok(())
    }

    /// Store a new pending approval request.
    pub fn store_pending(&self, request: ApprovalRequest) -> Result<(), ApprovalStorageError> {
        // Add to in-memory cache
        {
            let mut pending = self
                .pending
                .write()
                .map_err(|_| ApprovalStorageError::LockError)?;
            pending.insert(request.id.clone(), request.clone());
        }

        // Append to pending.log
        Self::append_to_file(&self.pending_path(), &request)?;

        tracing::debug!("Stored pending approval request: {}", request.id);
        Ok(())
    }

    /// Get an approval request by ID, searching all files.
    pub fn get(&self, id: &str) -> Result<Option<ApprovalRequest>, ApprovalStorageError> {
        // Check pending first
        {
            let mut pending = self
                .pending
                .write()
                .map_err(|_| ApprovalStorageError::LockError)?;
            if let Some(request) = pending.get_mut(id) {
                // Check for expiration
                if request.status == ApprovalStatus::Pending && request.is_expired() {
                    request.expire();
                    // Move to denied (expired)
                    let request = request.clone();
                    drop(pending);
                    self.move_to_denied(request.clone())?;
                    return Ok(Some(request));
                }
                return Ok(Some(request.clone()));
            }
        }

        // Check approved
        {
            let approved = self
                .approved
                .read()
                .map_err(|_| ApprovalStorageError::LockError)?;
            if let Some(request) = approved.get(id) {
                return Ok(Some(request.clone()));
            }
        }

        // Check denied
        {
            let denied = self
                .denied
                .read()
                .map_err(|_| ApprovalStorageError::LockError)?;
            if let Some(request) = denied.get(id) {
                return Ok(Some(request.clone()));
            }
        }

        Ok(None)
    }

    /// List all pending approval requests.
    pub fn list_pending(
        &self,
        tenant_id: Option<&str>,
    ) -> Result<Vec<ApprovalRequest>, ApprovalStorageError> {
        let mut pending = self
            .pending
            .write()
            .map_err(|_| ApprovalStorageError::LockError)?;
        let mut expired_ids = Vec::new();

        // Check for expirations
        for (id, request) in pending.iter_mut() {
            if request.status == ApprovalStatus::Pending && request.is_expired() {
                request.expire();
                expired_ids.push(id.clone());
            }
        }

        // Move expired to denied
        let expired_requests: Vec<_> = expired_ids
            .iter()
            .filter_map(|id| pending.remove(id))
            .collect();

        drop(pending);

        for request in expired_requests {
            self.move_to_denied(request)?;
        }

        // Re-acquire and return filtered list
        let pending = self
            .pending
            .read()
            .map_err(|_| ApprovalStorageError::LockError)?;
        Ok(pending
            .values()
            .filter(|r| {
                r.status == ApprovalStatus::Pending && tenant_id.is_none_or(|t| r.tenant_id == t)
            })
            .cloned()
            .collect())
    }

    /// Approve a request and move it from pending to approved.
    pub fn approve(
        &self,
        id: &str,
        by: impl Into<String>,
        reason: Option<String>,
    ) -> Result<ApprovalRequest, ApprovalError> {
        let mut pending = self
            .pending
            .write()
            .map_err(|_| ApprovalError::NotFound(id.to_string()))?;

        let request = pending
            .get_mut(id)
            .ok_or_else(|| ApprovalError::NotFound(id.to_string()))?;

        if request.is_expired() {
            request.expire();
            let request = request.clone();
            pending.remove(id);
            drop(pending);
            let _ = self.move_to_denied(request);
            return Err(ApprovalError::Expired(id.to_string()));
        }

        if request.status != ApprovalStatus::Pending {
            return Err(ApprovalError::AlreadyDecided(id.to_string()));
        }

        request.approve(by, reason);
        let request = request.clone();
        pending.remove(id);
        drop(pending);

        // Move to approved file
        self.move_to_approved(request.clone())
            .map_err(|_| ApprovalError::NotFound(id.to_string()))?;

        Ok(request)
    }

    /// Reject a request and move it from pending to denied.
    pub fn reject(
        &self,
        id: &str,
        by: impl Into<String>,
        reason: Option<String>,
    ) -> Result<ApprovalRequest, ApprovalError> {
        let mut pending = self
            .pending
            .write()
            .map_err(|_| ApprovalError::NotFound(id.to_string()))?;

        let request = pending
            .get_mut(id)
            .ok_or_else(|| ApprovalError::NotFound(id.to_string()))?;

        if request.is_expired() {
            request.expire();
            let request = request.clone();
            pending.remove(id);
            drop(pending);
            let _ = self.move_to_denied(request);
            return Err(ApprovalError::Expired(id.to_string()));
        }

        if request.status != ApprovalStatus::Pending {
            return Err(ApprovalError::AlreadyDecided(id.to_string()));
        }

        request.reject(by, reason);
        let request = request.clone();
        pending.remove(id);
        drop(pending);

        // Move to denied file
        self.move_to_denied(request.clone())
            .map_err(|_| ApprovalError::NotFound(id.to_string()))?;

        Ok(request)
    }

    /// Cancel a request.
    pub fn cancel(&self, id: &str) -> Result<ApprovalRequest, ApprovalError> {
        let mut pending = self
            .pending
            .write()
            .map_err(|_| ApprovalError::NotFound(id.to_string()))?;

        let request = pending
            .get_mut(id)
            .ok_or_else(|| ApprovalError::NotFound(id.to_string()))?;

        if request.status != ApprovalStatus::Pending {
            return Err(ApprovalError::AlreadyDecided(id.to_string()));
        }

        request.cancel();
        let request = request.clone();
        pending.remove(id);
        drop(pending);

        // Move to denied file
        self.move_to_denied(request.clone())
            .map_err(|_| ApprovalError::NotFound(id.to_string()))?;

        Ok(request)
    }

    /// Update an approved request with execution result.
    pub fn update_with_result(
        &self,
        id: &str,
        result: serde_json::Value,
    ) -> Result<(), ApprovalStorageError> {
        let mut approved = self
            .approved
            .write()
            .map_err(|_| ApprovalStorageError::LockError)?;

        if let Some(request) = approved.get_mut(id) {
            request.execution_result = Some(result);
            // Rewrite the approved file
            Self::rewrite_file(&self.approved_path(), &approved)?;
            Ok(())
        } else {
            Err(ApprovalStorageError::NotFound(id.to_string()))
        }
    }

    /// Update an approval request with audit event ID and correlation ID.
    pub fn update_audit_ids(
        &self,
        id: &str,
        event_id: uuid::Uuid,
        correlation_id: String,
    ) -> Result<(), ApprovalStorageError> {
        // Try pending first
        {
            let mut pending = self
                .pending
                .write()
                .map_err(|_| ApprovalStorageError::LockError)?;
            if let Some(request) = pending.get_mut(id) {
                request.set_audit_ids(event_id, correlation_id);
                // Rewrite the pending file
                Self::rewrite_file(&self.pending_path(), &pending)?;
                return Ok(());
            }
        }

        // If not in pending, try approved
        {
            let mut approved = self
                .approved
                .write()
                .map_err(|_| ApprovalStorageError::LockError)?;
            if let Some(request) = approved.get_mut(id) {
                request.set_audit_ids(event_id, correlation_id);
                // Rewrite the approved file
                Self::rewrite_file(&self.approved_path(), &approved)?;
                return Ok(());
            }
        }

        Err(ApprovalStorageError::NotFound(id.to_string()))
    }

    /// Move a request from pending to approved.
    fn move_to_approved(&self, request: ApprovalRequest) -> Result<(), ApprovalStorageError> {
        // Add to approved cache
        {
            let mut approved = self
                .approved
                .write()
                .map_err(|_| ApprovalStorageError::LockError)?;
            approved.insert(request.id.clone(), request.clone());

            // Trim cache if too large
            if approved.len() > self.max_cache_size {
                // Remove oldest entries (by decided_at)
                let mut entries: Vec<_> = approved.iter().collect();
                entries.sort_by(|a, b| a.1.decided_at.cmp(&b.1.decided_at));
                let to_remove: Vec<_> = entries
                    .iter()
                    .take(approved.len() - self.max_cache_size)
                    .map(|(k, _)| (*k).clone())
                    .collect();
                for key in to_remove {
                    approved.remove(&key);
                }
            }
        }

        // Append to approved.log
        Self::append_to_file(&self.approved_path(), &request)?;

        // Rewrite pending.log (remove this request)
        {
            let pending = self
                .pending
                .read()
                .map_err(|_| ApprovalStorageError::LockError)?;
            Self::rewrite_file(&self.pending_path(), &pending)?;
        }

        tracing::debug!("Moved approval request {} to approved", request.id);
        Ok(())
    }

    /// Move a request from pending to denied.
    fn move_to_denied(&self, request: ApprovalRequest) -> Result<(), ApprovalStorageError> {
        // Add to denied cache
        {
            let mut denied = self
                .denied
                .write()
                .map_err(|_| ApprovalStorageError::LockError)?;
            denied.insert(request.id.clone(), request.clone());

            // Trim cache if too large
            if denied.len() > self.max_cache_size {
                let mut entries: Vec<_> = denied.iter().collect();
                entries.sort_by(|a, b| a.1.decided_at.cmp(&b.1.decided_at));
                let to_remove: Vec<_> = entries
                    .iter()
                    .take(denied.len() - self.max_cache_size)
                    .map(|(k, _)| (*k).clone())
                    .collect();
                for key in to_remove {
                    denied.remove(&key);
                }
            }
        }

        // Append to denied.log
        Self::append_to_file(&self.denied_path(), &request)?;

        // Rewrite pending.log (remove this request)
        {
            let pending = self
                .pending
                .read()
                .map_err(|_| ApprovalStorageError::LockError)?;
            Self::rewrite_file(&self.pending_path(), &pending)?;
        }

        tracing::debug!("Moved approval request {} to denied", request.id);
        Ok(())
    }

    /// Clean up old decided requests from files.
    pub fn cleanup(&self, max_age: Duration) -> Result<(), ApprovalStorageError> {
        let cutoff = Utc::now() - max_age;

        // Clean approved cache (keep file intact for audit trail)
        {
            let mut approved = self
                .approved
                .write()
                .map_err(|_| ApprovalStorageError::LockError)?;
            approved.retain(|_, r| r.decided_at.is_none_or(|t| t > cutoff));
        }

        // Clean denied cache (keep file intact for audit trail)
        {
            let mut denied = self
                .denied
                .write()
                .map_err(|_| ApprovalStorageError::LockError)?;
            denied.retain(|_, r| r.decided_at.is_none_or(|t| t > cutoff));
        }

        Ok(())
    }

    /// Get the directory path.
    pub fn directory(&self) -> &Path {
        &self.directory
    }
}

/// Errors that can occur with approval storage.
#[derive(Debug, thiserror::Error)]
pub enum ApprovalStorageError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Lock error")]
    LockError,

    #[error("Approval request not found: {0}")]
    NotFound(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_store_and_get_pending() {
        let temp_dir = TempDir::new().unwrap();
        let storage = ApprovalFileStorage::new(temp_dir.path()).unwrap();

        let request = ApprovalRequest::new(
            "updateTicket",
            serde_json::json!({"id": 1}),
            vec!["priority".to_string()],
            "tenant_a",
            "support_agent",
            Duration::hours(1),
        );
        let id = request.id.clone();

        storage.store_pending(request).unwrap();

        let retrieved = storage.get(&id).unwrap().unwrap();
        assert_eq!(retrieved.status, ApprovalStatus::Pending);
        assert_eq!(retrieved.tool_name, "updateTicket");
    }

    #[test]
    fn test_approve_moves_to_approved_file() {
        let temp_dir = TempDir::new().unwrap();
        let storage = ApprovalFileStorage::new(temp_dir.path()).unwrap();

        let request = ApprovalRequest::new(
            "updateTicket",
            serde_json::json!({}),
            vec![],
            "tenant_a",
            "agent",
            Duration::hours(1),
        );
        let id = request.id.clone();

        storage.store_pending(request).unwrap();
        let approved = storage
            .approve(&id, "admin", Some("OK".to_string()))
            .unwrap();

        assert_eq!(approved.status, ApprovalStatus::Approved);
        assert_eq!(approved.decided_by, Some("admin".to_string()));

        // Should be in approved, not pending
        let pending = storage.list_pending(None).unwrap();
        assert!(pending.is_empty());

        // Should still be retrievable
        let retrieved = storage.get(&id).unwrap().unwrap();
        assert_eq!(retrieved.status, ApprovalStatus::Approved);
    }

    #[test]
    fn test_reject_moves_to_denied_file() {
        let temp_dir = TempDir::new().unwrap();
        let storage = ApprovalFileStorage::new(temp_dir.path()).unwrap();

        let request = ApprovalRequest::new(
            "deleteRecord",
            serde_json::json!({}),
            vec![],
            "tenant_a",
            "agent",
            Duration::hours(1),
        );
        let id = request.id.clone();

        storage.store_pending(request).unwrap();
        let rejected = storage
            .reject(&id, "admin", Some("Not allowed".to_string()))
            .unwrap();

        assert_eq!(rejected.status, ApprovalStatus::Rejected);

        // Should still be retrievable from denied
        let retrieved = storage.get(&id).unwrap().unwrap();
        assert_eq!(retrieved.status, ApprovalStatus::Rejected);
    }

    #[test]
    fn test_persistence_across_restarts() {
        let temp_dir = TempDir::new().unwrap();
        let id;

        // Create and store a request
        {
            let storage = ApprovalFileStorage::new(temp_dir.path()).unwrap();
            let request = ApprovalRequest::new(
                "testAction",
                serde_json::json!({}),
                vec![],
                "tenant",
                "role",
                Duration::hours(1),
            );
            id = request.id.clone();
            storage.store_pending(request).unwrap();
        }

        // "Restart" by creating a new storage instance
        {
            let storage = ApprovalFileStorage::new(temp_dir.path()).unwrap();
            let retrieved = storage.get(&id).unwrap().unwrap();
            assert_eq!(retrieved.status, ApprovalStatus::Pending);
        }
    }

    #[test]
    fn test_update_with_result() {
        let temp_dir = TempDir::new().unwrap();
        let storage = ApprovalFileStorage::new(temp_dir.path()).unwrap();

        let request = ApprovalRequest::new(
            "updateTicket",
            serde_json::json!({}),
            vec![],
            "tenant_a",
            "agent",
            Duration::hours(1),
        );
        let id = request.id.clone();

        storage.store_pending(request).unwrap();
        storage.approve(&id, "admin", None).unwrap();

        // Update with result
        let result = serde_json::json!({"updated": true, "rows_affected": 1});
        storage.update_with_result(&id, result.clone()).unwrap();

        // Verify result is stored
        let retrieved = storage.get(&id).unwrap().unwrap();
        assert_eq!(retrieved.execution_result, Some(result));
    }
}
