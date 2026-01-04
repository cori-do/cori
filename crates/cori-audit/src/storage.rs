//! Audit storage backends.

use cori_core::config::audit::{AuditConfig, StorageBackend};
use crate::error::AuditError;
use crate::event::AuditEvent;
use crate::logger::AuditFilter;
use async_trait::async_trait;
use std::sync::RwLock;
use uuid::Uuid;

/// Trait for audit storage backends.
#[async_trait]
pub trait AuditStorage: Send + Sync {
    /// Store an audit event.
    async fn store(&self, event: AuditEvent) -> Result<(), AuditError>;

    /// Query audit events with filters.
    async fn query(&self, filter: AuditFilter) -> Result<Vec<AuditEvent>, AuditError>;

    /// Get an audit event by ID.
    async fn get(&self, event_id: Uuid) -> Result<Option<AuditEvent>, AuditError>;
}

/// Create a storage backend based on configuration.
pub fn create_storage(config: &AuditConfig) -> Result<Box<dyn AuditStorage>, AuditError> {
    match config.storage.backend {
        StorageBackend::Console => Ok(Box::new(ConsoleStorage)),
        StorageBackend::File => {
            let path = config
                .storage
                .file_path
                .as_deref()
                .unwrap_or("audit.log");
            Ok(Box::new(FileStorage::new(path)?))
        }
        StorageBackend::Database => {
            // TODO: Implement database storage
            tracing::warn!("Database storage not yet implemented, falling back to console");
            Ok(Box::new(ConsoleStorage))
        }
    }
}

/// Console storage (logs to stdout).
pub struct ConsoleStorage;

#[async_trait]
impl AuditStorage for ConsoleStorage {
    async fn store(&self, event: AuditEvent) -> Result<(), AuditError> {
        let json = serde_json::to_string(&event)?;
        println!("{}", json);
        Ok(())
    }

    async fn query(&self, _filter: AuditFilter) -> Result<Vec<AuditEvent>, AuditError> {
        // Console storage doesn't support querying
        Ok(vec![])
    }

    async fn get(&self, _event_id: Uuid) -> Result<Option<AuditEvent>, AuditError> {
        // Console storage doesn't support retrieval
        Ok(None)
    }
}

/// File storage (appends to a log file).
pub struct FileStorage {
    path: String,
    // In-memory cache for querying (in production, you'd parse the file)
    events: RwLock<Vec<AuditEvent>>,
}

impl FileStorage {
    /// Create a new file storage.
    pub fn new(path: &str) -> Result<Self, AuditError> {
        Ok(Self {
            path: path.to_string(),
            events: RwLock::new(Vec::new()),
        })
    }
}

#[async_trait]
impl AuditStorage for FileStorage {
    async fn store(&self, event: AuditEvent) -> Result<(), AuditError> {
        let json = serde_json::to_string(&event)?;
        
        // Append to file
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        writeln!(file, "{}", json)?;

        // Also store in memory for querying
        if let Ok(mut events) = self.events.write() {
            events.push(event);
        }

        Ok(())
    }

    async fn query(&self, filter: AuditFilter) -> Result<Vec<AuditEvent>, AuditError> {
        let events = self.events.read().map_err(|e| {
            AuditError::StorageError(format!("Failed to acquire read lock: {}", e))
        })?;

        let mut results: Vec<_> = events
            .iter()
            .filter(|e| {
                if let Some(ref tenant) = filter.tenant_id {
                    if e.tenant_id.as_ref() != Some(tenant) {
                        return false;
                    }
                }
                if let Some(ref role) = filter.role {
                    if e.role.as_ref() != Some(role) {
                        return false;
                    }
                }
                if let Some(event_type) = filter.event_type {
                    if e.event_type != event_type {
                        return false;
                    }
                }
                if let Some(start) = filter.start_time {
                    if e.occurred_at < start {
                        return false;
                    }
                }
                if let Some(end) = filter.end_time {
                    if e.occurred_at > end {
                        return false;
                    }
                }
                true
            })
            .cloned()
            .collect();

        // Apply offset and limit
        if let Some(offset) = filter.offset {
            results = results.into_iter().skip(offset).collect();
        }
        if let Some(limit) = filter.limit {
            results.truncate(limit);
        }

        Ok(results)
    }

    async fn get(&self, event_id: Uuid) -> Result<Option<AuditEvent>, AuditError> {
        let events = self.events.read().map_err(|e| {
            AuditError::StorageError(format!("Failed to acquire read lock: {}", e))
        })?;

        Ok(events.iter().find(|e| e.event_id == event_id).cloned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::AuditEventType;

    #[tokio::test]
    async fn test_console_storage() {
        let storage = ConsoleStorage;
        let event = AuditEvent::new(AuditEventType::QueryExecuted);
        
        // Should not error
        storage.store(event).await.unwrap();
    }

    #[tokio::test]
    async fn test_file_storage_query() {
        let storage = FileStorage::new("/tmp/test_audit.log").unwrap();
        
        let event1 = AuditEvent::builder(AuditEventType::QueryExecuted)
            .tenant("client_a")
            .build();
        let event2 = AuditEvent::builder(AuditEventType::QueryExecuted)
            .tenant("client_b")
            .build();

        storage.store(event1).await.unwrap();
        storage.store(event2).await.unwrap();

        let filter = AuditFilter {
            tenant_id: Some("client_a".to_string()),
            ..Default::default()
        };
        let results = storage.query(filter).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].tenant_id, Some("client_a".to_string()));
    }
}

