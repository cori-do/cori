//! Dashboard application state.

use cori_audit::AuditLogger;
use cori_biscuit::KeyPair;
use cori_core::{CoriConfig, RoleConfig, TenancyConfig};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

/// Shared application state for the dashboard.
#[derive(Clone)]
pub struct AppState {
    inner: Arc<AppStateInner>,
}

struct AppStateInner {
    /// The loaded configuration.
    pub config: RwLock<CoriConfig>,
    /// Biscuit keypair for token operations.
    pub keypair: Option<KeyPair>,
    /// Audit logger for querying events.
    pub audit_logger: Option<Arc<AuditLogger>>,
    /// Approval manager for human-in-the-loop actions.
    pub approval_manager: Option<Arc<cori_mcp::ApprovalManager>>,
    /// Cached schema information.
    pub schema_cache: RwLock<Option<SchemaInfo>>,
    /// Database URL for introspection.
    pub database_url: Option<String>,
}

impl AppState {
    /// Create a new application state.
    pub fn new(config: CoriConfig) -> Self {
        // Try to load keypair from config
        let keypair = config.biscuit.resolve_private_key()
            .ok()
            .flatten()
            .and_then(|pk| cori_biscuit::KeyPair::from_private_key_hex(&pk).ok());
        
        // Get database URL
        let database_url = Some(config.upstream.connection_string());

        Self {
            inner: Arc::new(AppStateInner {
                config: RwLock::new(config),
                keypair,
                audit_logger: None,
                approval_manager: None,
                schema_cache: RwLock::new(None),
                database_url,
            }),
        }
    }

    /// Create state with an audit logger.
    pub fn with_audit_logger(mut self, logger: Arc<AuditLogger>) -> Self {
        // We need to recreate the inner to add the logger
        let inner = Arc::try_unwrap(self.inner).unwrap_or_else(|arc| (*arc).clone_inner());
        self.inner = Arc::new(AppStateInner {
            audit_logger: Some(logger),
            ..inner
        });
        self
    }

    /// Create state with an approval manager.
    pub fn with_approval_manager(mut self, manager: Arc<cori_mcp::ApprovalManager>) -> Self {
        let inner = Arc::try_unwrap(self.inner).unwrap_or_else(|arc| (*arc).clone_inner());
        self.inner = Arc::new(AppStateInner {
            approval_manager: Some(manager),
            ..inner
        });
        self
    }

    /// Get the current configuration.
    pub fn config(&self) -> std::sync::RwLockReadGuard<'_, CoriConfig> {
        self.inner.config.read().unwrap()
    }

    /// Get mutable access to configuration.
    pub fn config_mut(&self) -> std::sync::RwLockWriteGuard<'_, CoriConfig> {
        self.inner.config.write().unwrap()
    }

    /// Get the keypair if available.
    pub fn keypair(&self) -> Option<&KeyPair> {
        self.inner.keypair.as_ref()
    }

    /// Get the audit logger if available.
    pub fn audit_logger(&self) -> Option<&Arc<AuditLogger>> {
        self.inner.audit_logger.as_ref()
    }

    /// Get the approval manager if available.
    pub fn approval_manager(&self) -> Option<&Arc<cori_mcp::ApprovalManager>> {
        self.inner.approval_manager.as_ref()
    }

    /// Get the database URL if available.
    pub fn database_url(&self) -> Option<&str> {
        self.inner.database_url.as_deref()
    }

    /// Get cached schema info.
    pub fn schema_cache(&self) -> Option<SchemaInfo> {
        self.inner.schema_cache.read().unwrap().clone()
    }

    /// Update cached schema info.
    pub fn set_schema_cache(&self, schema: SchemaInfo) {
        *self.inner.schema_cache.write().unwrap() = Some(schema);
    }

    /// Get all roles from configuration.
    pub fn get_roles(&self) -> HashMap<String, RoleConfig> {
        self.inner.config.read().unwrap().roles.clone()
    }

    /// Get a specific role by name.
    pub fn get_role(&self, name: &str) -> Option<RoleConfig> {
        self.inner.config.read().unwrap().roles.get(name).cloned()
    }

    /// Add or update a role.
    pub fn save_role(&self, name: String, role: RoleConfig) {
        self.inner.config.write().unwrap().roles.insert(name, role);
    }

    /// Delete a role.
    pub fn delete_role(&self, name: &str) -> Option<RoleConfig> {
        self.inner.config.write().unwrap().roles.remove(name)
    }

    /// Get tenancy configuration.
    pub fn get_tenancy(&self) -> TenancyConfig {
        self.inner.config.read().unwrap().tenancy.clone()
    }

    /// Update tenancy configuration.
    pub fn set_tenancy(&self, tenancy: TenancyConfig) {
        self.inner.config.write().unwrap().tenancy = tenancy;
    }
}

impl AppStateInner {
    fn clone_inner(&self) -> Self {
        Self {
            config: RwLock::new(self.config.read().unwrap().clone()),
            keypair: self.keypair.clone(),
            audit_logger: self.audit_logger.clone(),
            approval_manager: self.approval_manager.clone(),
            schema_cache: RwLock::new(self.schema_cache.read().unwrap().clone()),
            database_url: self.database_url.clone(),
        }
    }
}

/// Schema information from database introspection.
#[derive(Debug, Clone)]
pub struct SchemaInfo {
    /// Tables in the database.
    pub tables: Vec<TableInfo>,
    /// When the schema was last refreshed.
    pub refreshed_at: chrono::DateTime<chrono::Utc>,
}

/// Information about a table.
#[derive(Debug, Clone)]
pub struct TableInfo {
    /// Schema name (e.g., "public").
    pub schema: String,
    /// Table name.
    pub name: String,
    /// Columns in the table.
    pub columns: Vec<ColumnInfo>,
    /// Primary key columns.
    pub primary_key: Vec<String>,
    /// Foreign keys.
    pub foreign_keys: Vec<ForeignKeyInfo>,
    /// Detected tenant column (if any).
    pub detected_tenant_column: Option<String>,
}

/// Information about a column.
#[derive(Debug, Clone)]
pub struct ColumnInfo {
    /// Column name.
    pub name: String,
    /// Data type.
    pub data_type: String,
    /// Whether the column is nullable.
    pub nullable: bool,
    /// Default value (if any).
    pub default: Option<String>,
}

/// Information about a foreign key.
#[derive(Debug, Clone)]
pub struct ForeignKeyInfo {
    /// Constraint name.
    pub name: String,
    /// Local columns.
    pub columns: Vec<String>,
    /// Referenced table.
    pub references_table: String,
    /// Referenced columns.
    pub references_columns: Vec<String>,
}
