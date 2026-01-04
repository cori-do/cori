//! Audit logging configuration.

use serde::{Deserialize, Serialize};

/// Configuration for audit logging.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditConfig {
    /// Whether audit logging is enabled.
    #[serde(default = "default_enabled")]
    pub enabled: bool,

    /// Whether to log queries.
    #[serde(default = "default_log_queries")]
    pub log_queries: bool,

    /// Whether to log query results (may contain sensitive data).
    #[serde(default)]
    pub log_results: bool,

    /// Whether to log errors.
    #[serde(default = "default_enabled")]
    pub log_errors: bool,

    /// Fields to include in audit entries.
    #[serde(default)]
    pub include: Vec<String>,

    /// Retention period in days.
    #[serde(default = "default_retention_days")]
    pub retention_days: u32,

    /// Output destinations.
    #[serde(default)]
    pub output: Vec<AuditOutput>,

    /// Storage backend configuration (legacy).
    #[serde(default)]
    pub storage: StorageConfig,

    /// Whether to enable tamper-evident integrity.
    #[serde(default)]
    pub integrity_enabled: bool,
}

/// Audit output destination.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditOutput {
    /// Output type: "file", "stdout", "postgres".
    #[serde(rename = "type")]
    pub output_type: String,

    /// File path (for file output).
    #[serde(default)]
    pub path: Option<String>,

    /// Database table (for postgres output).
    #[serde(default)]
    pub table: Option<String>,
}

/// Storage backend configuration (legacy).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StorageConfig {
    /// Storage backend type.
    #[serde(default)]
    pub backend: StorageBackend,

    /// File path (for file backend).
    #[serde(default)]
    pub file_path: Option<String>,

    /// Database URL (for database backend).
    #[serde(default)]
    pub database_url: Option<String>,
}

/// Storage backend type.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum StorageBackend {
    /// Log to stdout/stderr.
    #[default]
    Console,
    /// Log to a file.
    File,
    /// Store in a database.
    Database,
}

impl Default for AuditConfig {
    fn default() -> Self {
        Self {
            enabled: default_enabled(),
            log_queries: default_log_queries(),
            log_results: false,
            log_errors: true,
            include: Vec::new(),
            retention_days: default_retention_days(),
            output: Vec::new(),
            storage: StorageConfig::default(),
            integrity_enabled: false,
        }
    }
}

fn default_enabled() -> bool {
    true
}

fn default_log_queries() -> bool {
    true
}

fn default_retention_days() -> u32 {
    90
}
