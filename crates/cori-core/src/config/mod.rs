//! Configuration types for Cori AI Database Proxy.
//!
//! This module provides the unified configuration types used across all Cori crates.
//! Configuration can be loaded from YAML files (cori.yaml, tenancy.yaml, roles/*.yaml)
//! and combined into a single `CoriConfig` structure.
//!
//! # Configuration Files
//!
//! - **cori.yaml**: Main configuration file with upstream DB, proxy, biscuit, and feature settings
//! - **tenancy.yaml**: Defines how multi-tenancy is structured (tenant columns per table)
//! - **roles/*.yaml**: Individual role definitions with table permissions and constraints

pub mod audit;
pub mod biscuit;
pub mod dashboard;
pub mod mcp;
pub mod proxy;
pub mod role;
pub mod tenancy;

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

pub use audit::AuditConfig;
pub use biscuit::BiscuitConfig;
pub use dashboard::DashboardConfig;
pub use mcp::{McpConfig, Transport};
pub use proxy::{ProxyConfig, UpstreamConfig};
pub use role::{
    ColumnConstraints, CustomAction, CustomActionInput, EditableColumns, Operation,
    ReadableColumns, RoleConfig, TablePermissions,
};
pub use tenancy::{TableTenancyConfig, TenancyConfig};

/// Complete Cori configuration loaded from files.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoriConfig {
    /// Project name.
    #[serde(default)]
    pub project: Option<String>,

    /// Configuration version.
    #[serde(default)]
    pub version: Option<String>,

    /// Upstream Postgres connection.
    pub upstream: UpstreamConfig,

    /// Proxy settings.
    #[serde(default)]
    pub proxy: ProxyConfig,

    /// Biscuit token configuration.
    #[serde(default)]
    pub biscuit: BiscuitConfig,

    /// Tenancy configuration (inline or from file).
    #[serde(default)]
    pub tenancy: TenancyConfig,

    /// Path to tenancy configuration file (alternative to inline).
    #[serde(default)]
    pub tenancy_file: Option<PathBuf>,

    /// MCP server configuration.
    #[serde(default)]
    pub mcp: McpConfig,

    /// Dashboard configuration.
    #[serde(default)]
    pub dashboard: DashboardConfig,

    /// Audit logging configuration.
    #[serde(default)]
    pub audit: AuditConfig,

    /// Virtual schema configuration.
    #[serde(default)]
    pub virtual_schema: VirtualSchemaConfig,

    /// Global guardrails.
    #[serde(default)]
    pub guardrails: GuardrailsConfig,

    /// Observability configuration.
    #[serde(default)]
    pub observability: ObservabilityConfig,

    /// Directory containing role definition files.
    #[serde(default)]
    pub roles_dir: Option<PathBuf>,

    /// List of individual role definition files.
    #[serde(default)]
    pub role_files: Vec<PathBuf>,

    /// Inline role definitions.
    #[serde(default)]
    pub roles: HashMap<String, RoleConfig>,
}

impl Default for CoriConfig {
    fn default() -> Self {
        Self {
            project: None,
            version: None,
            upstream: UpstreamConfig::default(),
            proxy: ProxyConfig::default(),
            biscuit: BiscuitConfig::default(),
            tenancy: TenancyConfig::default(),
            tenancy_file: None,
            mcp: McpConfig::default(),
            dashboard: DashboardConfig::default(),
            audit: AuditConfig::default(),
            virtual_schema: VirtualSchemaConfig::default(),
            guardrails: GuardrailsConfig::default(),
            observability: ObservabilityConfig::default(),
            roles_dir: None,
            role_files: Vec::new(),
            roles: HashMap::new(),
        }
    }
}

/// Virtual schema filtering configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VirtualSchemaConfig {
    /// Whether virtual schema filtering is enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Tables that should never appear in schema introspection.
    #[serde(default)]
    pub always_hidden: Vec<String>,

    /// Tables visible to all tokens (e.g., reference data).
    #[serde(default)]
    pub always_visible: Vec<String>,

    /// Whether to expose row counts.
    #[serde(default)]
    pub expose_row_counts: bool,

    /// Whether to expose index definitions.
    #[serde(default)]
    pub expose_indexes: bool,
}

impl Default for VirtualSchemaConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            always_hidden: Vec::new(),
            always_visible: Vec::new(),
            expose_row_counts: false,
            expose_indexes: false,
        }
    }
}

/// Global guardrails configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuardrailsConfig {
    /// Maximum rows that can be returned in any query.
    #[serde(default = "default_max_rows")]
    pub max_rows_per_query: u64,

    /// Maximum rows that can be affected by UPDATE/DELETE.
    #[serde(default = "default_max_affected")]
    pub max_affected_rows: u64,

    /// Operations that are never allowed (regardless of role).
    #[serde(default)]
    pub blocked_operations: Vec<String>,

    /// Rate limiting configuration.
    #[serde(default)]
    pub rate_limit: Option<RateLimitConfig>,
}

impl Default for GuardrailsConfig {
    fn default() -> Self {
        Self {
            max_rows_per_query: default_max_rows(),
            max_affected_rows: default_max_affected(),
            blocked_operations: vec![
                "TRUNCATE".to_string(),
                "DROP".to_string(),
                "ALTER".to_string(),
            ],
            rate_limit: None,
        }
    }
}

/// Rate limiting configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimitConfig {
    /// Maximum queries per minute per token.
    #[serde(default = "default_queries_per_minute")]
    pub queries_per_minute: u32,

    /// Maximum mutations per minute per token.
    #[serde(default = "default_mutations_per_minute")]
    pub mutations_per_minute: u32,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            queries_per_minute: default_queries_per_minute(),
            mutations_per_minute: default_mutations_per_minute(),
        }
    }
}

/// Observability configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ObservabilityConfig {
    /// Prometheus metrics configuration.
    #[serde(default)]
    pub metrics: Option<MetricsConfig>,

    /// Health check configuration.
    #[serde(default)]
    pub health: Option<HealthConfig>,

    /// Tracing configuration.
    #[serde(default)]
    pub tracing: Option<TracingConfig>,
}

/// Metrics configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsConfig {
    /// Whether metrics are enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Port for metrics endpoint.
    #[serde(default = "default_metrics_port")]
    pub port: u16,

    /// Path for metrics endpoint.
    #[serde(default = "default_metrics_path")]
    pub path: String,
}

impl Default for MetricsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            port: default_metrics_port(),
            path: default_metrics_path(),
        }
    }
}

/// Health check configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthConfig {
    /// Whether health checks are enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Port for health check endpoint.
    #[serde(default = "default_health_port")]
    pub port: u16,

    /// Path for health check endpoint.
    #[serde(default = "default_health_path")]
    pub path: String,
}

impl Default for HealthConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            port: default_health_port(),
            path: default_health_path(),
        }
    }
}

/// Tracing configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TracingConfig {
    /// Whether tracing is enabled.
    #[serde(default)]
    pub enabled: bool,

    /// OpenTelemetry endpoint.
    #[serde(default)]
    pub endpoint: Option<String>,
}

impl Default for TracingConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            endpoint: None,
        }
    }
}

// Default value functions
fn default_true() -> bool {
    true
}

fn default_max_rows() -> u64 {
    10000
}

fn default_max_affected() -> u64 {
    1000
}

fn default_queries_per_minute() -> u32 {
    100
}

fn default_mutations_per_minute() -> u32 {
    20
}

fn default_metrics_port() -> u16 {
    9090
}

fn default_metrics_path() -> String {
    "/metrics".to_string()
}

fn default_health_port() -> u16 {
    8081
}

fn default_health_path() -> String {
    "/health".to_string()
}

/// Error type for configuration loading.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("YAML parse error: {0}")]
    Yaml(#[from] serde_yaml::Error),

    #[error("Configuration error: {0}")]
    Config(String),
}

impl CoriConfig {
    /// Load configuration from a YAML file.
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let content = fs::read_to_string(path.as_ref())?;
        Self::from_yaml(&content)
    }

    /// Parse configuration from YAML content.
    pub fn from_yaml(content: &str) -> Result<Self, ConfigError> {
        serde_yaml::from_str(content).map_err(ConfigError::from)
    }

    /// Load configuration and resolve all external references.
    ///
    /// This loads:
    /// - Tenancy configuration from `tenancy_file` if specified
    /// - Role configurations from `roles_dir` and `role_files`
    pub fn load_with_context(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let path = path.as_ref();
        let mut config = Self::from_file(path)?;

        let base_dir = path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));

        // Load tenancy configuration from file if specified
        if let Some(tenancy_file) = &config.tenancy_file {
            let tenancy_path = if tenancy_file.is_absolute() {
                tenancy_file.clone()
            } else {
                base_dir.join(tenancy_file)
            };

            if tenancy_path.exists() {
                let tenancy_content = fs::read_to_string(&tenancy_path)?;
                config.tenancy = TenancyConfig::from_yaml(&tenancy_content)?;
            }
        }

        // Load roles from directory
        if let Some(roles_dir) = &config.roles_dir {
            let roles_path = if roles_dir.is_absolute() {
                roles_dir.clone()
            } else {
                base_dir.join(roles_dir)
            };

            if roles_path.exists() && roles_path.is_dir() {
                for entry in fs::read_dir(&roles_path)? {
                    let entry = entry?;
                    let path = entry.path();
                    if path.extension().map(|e| e == "yaml" || e == "yml").unwrap_or(false) {
                        let role = RoleConfig::from_file(&path)?;
                        config.roles.insert(role.name.clone(), role);
                    }
                }
            }
        }

        // Load roles from individual files
        for role_file in &config.role_files.clone() {
            let role_path = if role_file.is_absolute() {
                role_file.clone()
            } else {
                base_dir.join(role_file)
            };

            if role_path.exists() {
                let role = RoleConfig::from_file(&role_path)?;
                config.roles.insert(role.name.clone(), role);
            }
        }

        Ok(config)
    }

    /// Get a role by name.
    pub fn get_role(&self, name: &str) -> Option<&RoleConfig> {
        self.roles.get(name)
    }

    /// Get the tenancy configuration, resolving any file references.
    pub fn get_tenancy(&self) -> &TenancyConfig {
        &self.tenancy
    }

    /// Check if a table is globally hidden from schema introspection.
    pub fn is_table_hidden(&self, table: &str) -> bool {
        self.virtual_schema.always_hidden.iter().any(|t| t == table)
    }

    /// Check if a table is globally visible in schema introspection.
    pub fn is_table_always_visible(&self, table: &str) -> bool {
        self.virtual_schema.always_visible.iter().any(|t| t == table)
    }
}
