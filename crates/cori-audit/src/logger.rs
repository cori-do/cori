//! Audit logger implementation.

use cori_core::AuditConfig;
use crate::error::AuditError;
use crate::event::AuditEvent;
use crate::storage::AuditStorage;

/// The main audit logger.
pub struct AuditLogger {
    config: AuditConfig,
    storage: Box<dyn AuditStorage>,
}

impl AuditLogger {
    /// Create a new audit logger with the given configuration.
    pub fn new(config: AuditConfig) -> Result<Self, AuditError> {
        let storage = crate::storage::create_storage(&config)?;
        Ok(Self { config, storage })
    }

    /// Log an audit event.
    pub async fn log(&self, event: AuditEvent) -> Result<(), AuditError> {
        if !self.config.enabled {
            return Ok(());
        }

        // Log to tracing as well
        tracing::info!(
            event_id = %event.event_id,
            event_type = ?event.event_type,
            tenant = ?event.tenant_id,
            role = ?event.role,
            "Audit event"
        );

        self.storage.store(event).await
    }

    /// Query audit events with filters.
    pub async fn query(&self, filter: AuditFilter) -> Result<Vec<AuditEvent>, AuditError> {
        self.storage.query(filter).await
    }

    /// Get an audit event by ID.
    pub async fn get(&self, event_id: uuid::Uuid) -> Result<Option<AuditEvent>, AuditError> {
        self.storage.get(event_id).await
    }
}

/// Filter for querying audit events.
#[derive(Debug, Clone, Default)]
pub struct AuditFilter {
    /// Filter by tenant ID.
    pub tenant_id: Option<String>,
    /// Filter by role.
    pub role: Option<String>,
    /// Filter by event type.
    pub event_type: Option<crate::event::AuditEventType>,
    /// Filter by start time.
    pub start_time: Option<chrono::DateTime<chrono::Utc>>,
    /// Filter by end time.
    pub end_time: Option<chrono::DateTime<chrono::Utc>>,
    /// Maximum number of results.
    pub limit: Option<usize>,
    /// Offset for pagination.
    pub offset: Option<usize>,
}

