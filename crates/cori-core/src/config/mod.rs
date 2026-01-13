//! Configuration types for Cori MCP Server.
//!
//! This module provides the unified configuration types used across all Cori crates.
//! Configuration follows a convention-over-configuration approach:
//!
//! # Configuration Files
//!
//! ```text
//! cori.yaml              → Main configuration (CoriConfig)
//! schema/schema.yaml     → Auto-generated database schema (SchemaDefinition)
//! schema/rules.yaml      → User-edited tenancy and validation rules (RulesDefinition)
//! schema/types.yaml      → Reusable semantic types (TypesDefinition)
//! roles/*.yaml           → Role definitions (RoleDefinition)
//! groups/*.yaml          → Approval groups (GroupDefinition)
//! ```

pub mod audit;
pub mod biscuit;
pub mod dashboard;
pub mod group_definition;
pub mod mcp;
pub mod proxy;
pub mod role_definition;
pub mod rules_definition;
pub mod schema_definition;
pub mod types_definition;

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

pub use audit::AuditConfig;
pub use biscuit::BiscuitConfig;
pub use dashboard::{AuthConfig, AuthType, BasicAuthUser, DashboardConfig, OidcConfig};
pub use group_definition::GroupDefinition;
pub use mcp::{McpConfig, Transport};
pub use proxy::{ConnectionPoolConfig, SslMode, UpstreamConfig};
pub use role_definition::{
    AllColumns, ApprovalConfig, ApprovalRequirement, ColumnCondition, ColumnList,
    ComparisonCondition, CreatableColumnConstraints, CreatableColumns, DeletableConstraints,
    DeletablePermission, ReadableConfig, ReadableConfigFull, RoleDefinition, TablePermissions,
    UpdatableColumnConstraints, UpdatableColumns,
};
pub use rules_definition::{
    ColumnRules, InheritedTenant, RulesDefinition, SoftDeleteConfig, SoftDeleteValue, TableRules,
    TenantConfig,
};
pub use schema_definition::{
    ColumnSchema, ColumnType, DatabaseEngine, DatabaseInfo, EnumDefinition, ForeignKey,
    ForeignKeyAction, ForeignKeyReference, Index, SchemaDefinition, TableSchema,
};
pub use types_definition::{TypeDef, TypesDefinition};

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

    /// Biscuit token configuration.
    #[serde(default)]
    pub biscuit: BiscuitConfig,

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

    // Convention-based directories (override defaults)
    /// Directory containing schema files (default: "schema/").
    #[serde(default)]
    pub schema_dir: Option<PathBuf>,

    /// Directory containing group files (default: "groups/").
    #[serde(default)]
    pub groups_dir: Option<PathBuf>,

    // Loaded data (populated by load_with_context)
    /// Role definitions loaded from roles/*.yaml.
    #[serde(default)]
    pub roles: HashMap<String, RoleDefinition>,

    /// Group definitions loaded from groups/*.yaml.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub groups: HashMap<String, GroupDefinition>,

    /// Schema definition loaded from schema/schema.yaml.
    #[serde(default, skip_serializing)]
    pub schema: Option<SchemaDefinition>,

    /// Rules definition loaded from schema/rules.yaml.
    #[serde(default, skip_serializing)]
    pub rules: Option<RulesDefinition>,

    /// Types definition loaded from schema/types.yaml.
    #[serde(default, skip_serializing)]
    pub types: Option<TypesDefinition>,
}

impl Default for CoriConfig {
    fn default() -> Self {
        Self {
            project: None,
            version: None,
            upstream: UpstreamConfig::default(),
            biscuit: BiscuitConfig::default(),
            mcp: McpConfig::default(),
            dashboard: DashboardConfig::default(),
            audit: AuditConfig::default(),
            virtual_schema: VirtualSchemaConfig::default(),
            guardrails: GuardrailsConfig::default(),
            observability: ObservabilityConfig::default(),
            schema_dir: None,
            groups_dir: None,
            roles: HashMap::new(),
            groups: HashMap::new(),
            schema: None,
            rules: None,
            types: None,
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
    /// This loads configuration following the convention-over-configuration approach:
    /// - `schema/schema.yaml` → SchemaDefinition (auto-generated)
    /// - `schema/rules.yaml` → RulesDefinition (user-edited tenancy/validation)
    /// - `schema/types.yaml` → TypesDefinition (reusable semantic types)
    /// - `roles/*.yaml` → RoleDefinition (one per file)
    /// - `groups/*.yaml` → GroupDefinition (one per file)
    pub fn load_with_context(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let path = path.as_ref();
        let mut config = Self::from_file(path)?;

        let base_dir = path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));

        // Determine schema directory (default: "schema/")
        let schema_dir = config
            .schema_dir
            .clone()
            .unwrap_or_else(|| PathBuf::from("schema"));
        let schema_path = if schema_dir.is_absolute() {
            schema_dir
        } else {
            base_dir.join(schema_dir)
        };

        // Load schema definition from schema/schema.yaml
        let schema_file = schema_path.join("schema.yaml");
        if schema_file.exists() {
            config.schema = Some(SchemaDefinition::from_file(&schema_file)?);
        }

        // Load rules definition from schema/rules.yaml
        let rules_file = schema_path.join("rules.yaml");
        if rules_file.exists() {
            config.rules = Some(RulesDefinition::from_file(&rules_file)?);
        }

        // Load types definition from schema/types.yaml
        let types_file = schema_path.join("types.yaml");
        if types_file.exists() {
            config.types = Some(TypesDefinition::from_file(&types_file)?);
        }

        // Load roles from roles/*.yaml (convention)
        let roles_path = base_dir.join("roles");
        if roles_path.exists() && roles_path.is_dir() {
            for entry in fs::read_dir(&roles_path)? {
                let entry = entry?;
                let path = entry.path();
                if path.extension().map(|e| e == "yaml" || e == "yml").unwrap_or(false) {
                    // Load as RoleDefinition
                    if let Ok(role) = RoleDefinition::from_file(&path) {
                        config.roles.insert(role.name.clone(), role);
                    }
                }
            }
        }

        // Determine groups directory (default: "groups/")
        let groups_dir = config
            .groups_dir
            .clone()
            .unwrap_or_else(|| PathBuf::from("groups"));
        let groups_path = if groups_dir.is_absolute() {
            groups_dir
        } else {
            base_dir.join(groups_dir)
        };

        // Load groups from groups/*.yaml
        if groups_path.exists() && groups_path.is_dir() {
            for entry in fs::read_dir(&groups_path)? {
                let entry = entry?;
                let path = entry.path();
                if path.extension().map(|e| e == "yaml" || e == "yml").unwrap_or(false) {
                    let group = GroupDefinition::from_file(&path)?;
                    config.groups.insert(group.name.clone(), group);
                }
            }
        }

        Ok(config)
    }

    /// Get a role definition by name.
    pub fn get_role(&self, name: &str) -> Option<&RoleDefinition> {
        self.roles.get(name)
    }

    /// Get the rules definition.
    pub fn get_rules(&self) -> Option<&RulesDefinition> {
        self.rules.as_ref()
    }

    /// Get the schema definition.
    pub fn get_schema(&self) -> Option<&SchemaDefinition> {
        self.schema.as_ref()
    }

    /// Get the types definition.
    pub fn get_types(&self) -> Option<&TypesDefinition> {
        self.types.as_ref()
    }

    /// Get a group by name.
    pub fn get_group(&self, name: &str) -> Option<&GroupDefinition> {
        self.groups.get(name)
    }

    /// Check if a table is globally hidden from schema introspection.
    pub fn is_table_hidden(&self, table: &str) -> bool {
        self.virtual_schema.always_hidden.iter().any(|t| t == table)
    }

    /// Check if a table is globally visible in schema introspection.
    pub fn is_table_always_visible(&self, table: &str) -> bool {
        self.virtual_schema.always_visible.iter().any(|t| t == table)
    }

    /// Get tenant configuration for a table from rules.
    pub fn get_table_tenant_config(&self, table: &str) -> Option<&TenantConfig> {
        if let Some(rules) = &self.rules {
            if let Some(config) = rules.get_tenant_config(table) {
                return Some(config);
            }
        }
        None
    }

    /// Check if a table is global (no tenant scoping).
    pub fn is_global_table(&self, table: &str) -> bool {
        if let Some(rules) = &self.rules {
            return rules.is_global_table(table);
        }
        false
    }
}
