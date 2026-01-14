//! Audit storage backends.
//!
//! Provides different storage options for audit events:
//! - **FileStorage**: JSON Lines format (one JSON object per line) for structured querying
//! - **ConsoleStorage**: Human-readable format for console output
//! - **DualStorage**: Combines both file and console output

use crate::error::AuditError;
use crate::event::AuditEvent;
use crate::logger::AuditFilter;
use async_trait::async_trait;
use std::path::{Path, PathBuf};
use std::sync::RwLock;
use uuid::Uuid;

/// Trait for audit storage backends.
#[async_trait]
pub trait AuditStorage: Send + Sync {
    /// Store an audit event.
    async fn store(&self, event: AuditEvent) -> Result<(), AuditError>;

    /// Query audit events with filters.
    async fn query(&self, filter: AuditFilter) -> Result<Vec<AuditEvent>, AuditError>;

    /// Count audit events matching a filter (excludes limit/offset).
    async fn count(&self, filter: AuditFilter) -> Result<usize, AuditError>;

    /// Get an audit event by ID.
    async fn get(&self, event_id: Uuid) -> Result<Option<AuditEvent>, AuditError>;
}

/// Console storage that outputs human-readable log lines to stdout.
pub struct ConsoleStorage;

impl ConsoleStorage {
    /// Create a new console storage.
    pub fn new() -> Self {
        Self
    }
}

impl Default for ConsoleStorage {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl AuditStorage for ConsoleStorage {
    async fn store(&self, event: AuditEvent) -> Result<(), AuditError> {
        // Output human-readable format to stdout
        println!("{}", event.to_log_line());
        Ok(())
    }

    async fn query(&self, _filter: AuditFilter) -> Result<Vec<AuditEvent>, AuditError> {
        // Console storage doesn't support querying
        Ok(vec![])
    }

    async fn count(&self, _filter: AuditFilter) -> Result<usize, AuditError> {
        // Console storage doesn't support counting
        Ok(0)
    }

    async fn get(&self, _event_id: Uuid) -> Result<Option<AuditEvent>, AuditError> {
        // Console storage doesn't support retrieval
        Ok(None)
    }
}

/// File storage that appends JSON Lines to a log file.
///
/// Each event is serialized as a single-line JSON object, making it easy to:
/// - Parse with standard tools (jq, grep, etc.)
/// - Stream and process incrementally
/// - Load into log aggregation systems
pub struct FileStorage {
    /// Path to the log file.
    path: PathBuf,
    /// In-memory cache for querying (limited to recent events).
    events: RwLock<Vec<AuditEvent>>,
    /// Maximum number of events to cache in memory.
    max_cache_size: usize,
}

impl FileStorage {
    /// Create a new file storage with the given path.
    ///
    /// This will load any existing events from the file into memory.
    pub fn new(path: impl AsRef<Path>) -> Result<Self, AuditError> {
        let path = path.as_ref().to_path_buf();

        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() && !parent.exists() {
                std::fs::create_dir_all(parent)?;
            }
        }

        // Load existing events from file
        let events = Self::load_from_file(&path)?;
        tracing::info!(
            "Loaded {} existing audit events from {}",
            events.len(),
            path.display()
        );

        Ok(Self {
            path,
            events: RwLock::new(events),
            max_cache_size: 10000,
        })
    }

    /// Load events from a JSON Lines file.
    fn load_from_file(path: &Path) -> Result<Vec<AuditEvent>, AuditError> {
        if !path.exists() {
            return Ok(Vec::new());
        }

        let content = std::fs::read_to_string(path)?;
        let mut events = Vec::new();

        for (line_num, line) in content.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            match serde_json::from_str::<AuditEvent>(line) {
                Ok(event) => events.push(event),
                Err(e) => {
                    tracing::warn!(
                        "Failed to parse audit event on line {}: {}",
                        line_num + 1,
                        e
                    );
                    // Continue loading other events
                }
            }
        }

        Ok(events)
    }

    /// Create file storage with a custom cache size.
    pub fn with_cache_size(mut self, size: usize) -> Self {
        self.max_cache_size = size;
        self
    }

    /// Get the path to the log file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Get the total number of events.
    pub fn total_count(&self) -> usize {
        self.events.read().map(|e| e.len()).unwrap_or(0)
    }
}

#[async_trait]
impl AuditStorage for FileStorage {
    async fn store(&self, event: AuditEvent) -> Result<(), AuditError> {
        // Serialize to JSON Lines format (compact, single line)
        let json = serde_json::to_string(&event)?;

        // Append to file
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        writeln!(file, "{}", json)?;

        // Also store in memory for querying (with size limit)
        if let Ok(mut events) = self.events.write() {
            events.push(event);
            // Trim old events if cache is too large
            if events.len() > self.max_cache_size {
                let drain_count = events.len() - self.max_cache_size;
                events.drain(0..drain_count);
            }
        }

        Ok(())
    }

    async fn query(&self, filter: AuditFilter) -> Result<Vec<AuditEvent>, AuditError> {
        let events = self.events.read().map_err(|e| {
            AuditError::StorageError(format!("Failed to acquire read lock: {}", e))
        })?;

        let mut results: Vec<_> = events
            .iter()
            .filter(|e| Self::matches_filter(e, &filter))
            .cloned()
            .collect();

        // Apply sorting (default: newest first)
        let sort_desc = filter.sort_desc.unwrap_or(true);
        match filter.sort_by.as_deref() {
            Some("occurred_at") | None => {
                if sort_desc {
                    results.sort_by(|a, b| b.occurred_at.cmp(&a.occurred_at));
                } else {
                    results.sort_by(|a, b| a.occurred_at.cmp(&b.occurred_at));
                }
            }
            Some("role") => {
                if sort_desc {
                    results.sort_by(|a, b| b.role.cmp(&a.role));
                } else {
                    results.sort_by(|a, b| a.role.cmp(&b.role));
                }
            }
            Some("tenant_id") => {
                if sort_desc {
                    results.sort_by(|a, b| b.tenant_id.cmp(&a.tenant_id));
                } else {
                    results.sort_by(|a, b| a.tenant_id.cmp(&b.tenant_id));
                }
            }
            Some("action") => {
                if sort_desc {
                    results.sort_by(|a, b| b.action.cmp(&a.action));
                } else {
                    results.sort_by(|a, b| a.action.cmp(&b.action));
                }
            }
            Some("duration_ms") => {
                if sort_desc {
                    results.sort_by(|a, b| b.duration_ms.cmp(&a.duration_ms));
                } else {
                    results.sort_by(|a, b| a.duration_ms.cmp(&b.duration_ms));
                }
            }
            Some(_) => {
                // Unknown sort field, default to time desc
                results.sort_by(|a, b| b.occurred_at.cmp(&a.occurred_at));
            }
        }

        // Apply offset and limit
        if let Some(offset) = filter.offset {
            results = results.into_iter().skip(offset).collect();
        }
        if let Some(limit) = filter.limit {
            results.truncate(limit);
        }

        Ok(results)
    }

    async fn count(&self, filter: AuditFilter) -> Result<usize, AuditError> {
        let events = self.events.read().map_err(|e| {
            AuditError::StorageError(format!("Failed to acquire read lock: {}", e))
        })?;

        Ok(events.iter().filter(|e| Self::matches_filter(e, &filter)).count())
    }

    async fn get(&self, event_id: Uuid) -> Result<Option<AuditEvent>, AuditError> {
        let events = self.events.read().map_err(|e| {
            AuditError::StorageError(format!("Failed to acquire read lock: {}", e))
        })?;

        Ok(events.iter().find(|e| e.event_id == event_id).cloned())
    }
}

impl FileStorage {
    /// Check if an event matches the given filter.
    fn matches_filter(event: &AuditEvent, filter: &AuditFilter) -> bool {
        if let Some(ref tenant) = filter.tenant_id {
            if &event.tenant_id != tenant {
                return false;
            }
        }
        if let Some(ref role) = filter.role {
            if &event.role != role {
                return false;
            }
        }
        if let Some(ref action) = filter.action {
            if &event.action != action {
                return false;
            }
        }
        if let Some(event_type) = filter.event_type {
            if event.event_type != event_type {
                return false;
            }
        }
        if let Some(start) = filter.start_time {
            if event.occurred_at < start {
                return false;
            }
        }
        if let Some(end) = filter.end_time {
            if event.occurred_at > end {
                return false;
            }
        }
        true
    }
}

/// Dual storage that writes to both file (JSON) and console (human-readable).
///
/// This is the recommended storage for production use, providing:
/// - Structured JSON logs for analysis and archival
/// - Human-readable output for real-time monitoring
pub struct DualStorage {
    file: FileStorage,
    console: ConsoleStorage,
}

impl DualStorage {
    /// Create a new dual storage with the given file path.
    pub fn new(path: impl AsRef<Path>) -> Result<Self, AuditError> {
        Ok(Self {
            file: FileStorage::new(path)?,
            console: ConsoleStorage::new(),
        })
    }

    /// Get the file path.
    pub fn path(&self) -> &Path {
        self.file.path()
    }
}

#[async_trait]
impl AuditStorage for DualStorage {
    async fn store(&self, event: AuditEvent) -> Result<(), AuditError> {
        // Store in both backends
        self.console.store(event.clone()).await?;
        self.file.store(event).await?;
        Ok(())
    }

    async fn query(&self, filter: AuditFilter) -> Result<Vec<AuditEvent>, AuditError> {
        // Query from file storage (has the cache)
        self.file.query(filter).await
    }

    async fn count(&self, filter: AuditFilter) -> Result<usize, AuditError> {
        self.file.count(filter).await
    }

    async fn get(&self, event_id: Uuid) -> Result<Option<AuditEvent>, AuditError> {
        self.file.get(event_id).await
    }
}

/// No-op storage that discards all events.
///
/// Useful for testing or when audit is disabled.
pub struct NullStorage;

impl NullStorage {
    /// Create a new null storage.
    pub fn new() -> Self {
        Self
    }
}

impl Default for NullStorage {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl AuditStorage for NullStorage {
    async fn store(&self, _event: AuditEvent) -> Result<(), AuditError> {
        Ok(())
    }

    async fn query(&self, _filter: AuditFilter) -> Result<Vec<AuditEvent>, AuditError> {
        Ok(vec![])
    }

    async fn count(&self, _filter: AuditFilter) -> Result<usize, AuditError> {
        Ok(0)
    }

    async fn get(&self, _event_id: Uuid) -> Result<Option<AuditEvent>, AuditError> {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::AuditEventType;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_console_storage() {
        let storage = ConsoleStorage::new();
        let event = AuditEvent::new(
            AuditEventType::ToolCalled,
            "admin",
            "acme",
            "listCustomers",
        );

        // Should not error
        storage.store(event).await.unwrap();
    }

    #[tokio::test]
    async fn test_file_storage_json_lines() {
        let temp_dir = TempDir::new().unwrap();
        let log_path = temp_dir.path().join("audit.log");

        let storage = FileStorage::new(&log_path).unwrap();

        let event = AuditEvent::builder(
            AuditEventType::QueryExecuted,
            "support_agent",
            "client_a",
            "getOrder",
        )
        .sql("SELECT * FROM orders WHERE id = 1")
        .row_count(1)
        .build();

        storage.store(event).await.unwrap();

        // Verify JSON was written
        let content = std::fs::read_to_string(&log_path).unwrap();
        assert!(content.contains("\"event_type\":\"query_executed\""));
        assert!(content.contains("\"role\":\"support_agent\""));
        assert!(content.contains("\"tenant_id\":\"client_a\""));
        assert!(content.contains("\"action\":\"getOrder\""));
    }

    #[tokio::test]
    async fn test_file_storage_query() {
        let temp_dir = TempDir::new().unwrap();
        let log_path = temp_dir.path().join("audit.log");

        let storage = FileStorage::new(&log_path).unwrap();

        let event1 = AuditEvent::new(
            AuditEventType::QueryExecuted,
            "admin",
            "client_a",
            "listOrders",
        );
        let event2 = AuditEvent::new(
            AuditEventType::QueryExecuted,
            "admin",
            "client_b",
            "listOrders",
        );

        storage.store(event1).await.unwrap();
        storage.store(event2).await.unwrap();

        let filter = AuditFilter {
            tenant_id: Some("client_a".to_string()),
            ..Default::default()
        };
        let results = storage.query(filter).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].tenant_id, "client_a");
    }

    #[tokio::test]
    async fn test_dual_storage() {
        let temp_dir = TempDir::new().unwrap();
        let log_path = temp_dir.path().join("audit.log");

        let storage = DualStorage::new(&log_path).unwrap();

        let event = AuditEvent::new(
            AuditEventType::ApprovalRequested,
            "support_agent",
            "acme",
            "deleteOrder",
        );

        storage.store(event).await.unwrap();

        // Verify JSON was written to file
        let content = std::fs::read_to_string(&log_path).unwrap();
        assert!(content.contains("\"event_type\":\"approval_requested\""));
    }

    #[tokio::test]
    async fn test_null_storage() {
        let storage = NullStorage::new();
        let event = AuditEvent::new(
            AuditEventType::ToolCalled,
            "admin",
            "acme",
            "test",
        );

        // Should not error
        storage.store(event).await.unwrap();

        let results = storage.query(AuditFilter::default()).await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_file_storage_creates_directory() {
        let temp_dir = TempDir::new().unwrap();
        let log_path = temp_dir.path().join("logs/nested/audit.log");

        let storage = FileStorage::new(&log_path).unwrap();

        let event = AuditEvent::new(
            AuditEventType::ToolCalled,
            "admin",
            "acme",
            "test",
        );

        storage.store(event).await.unwrap();

        assert!(log_path.exists());
    }
}
