//! Tenancy configuration for multi-tenant databases.
//!
//! This module defines how multi-tenancy is structured in the database,
//! including tenant column names per table and global tables.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

use super::ConfigError;

/// Configuration for tenant isolation (RLS).
///
/// This defines the database-level structure of multi-tenancy,
/// separate from access control (roles).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TenancyConfig {
    /// Tenant identifier configuration.
    #[serde(default)]
    pub tenant_id: TenantIdConfig,

    /// Default column name for tenant isolation.
    /// Used when not overridden per-table.
    #[serde(default = "default_tenant_column")]
    pub default_column: String,

    /// Per-table tenant column configuration.
    #[serde(default)]
    pub tables: HashMap<String, TableTenancyConfig>,

    /// Tables that are global (no tenant scoping applied).
    #[serde(default)]
    pub global_tables: Vec<String>,

    /// Auto-detection configuration.
    #[serde(default)]
    pub auto_detect: AutoDetectConfig,
}

impl Default for TenancyConfig {
    fn default() -> Self {
        Self {
            tenant_id: TenantIdConfig::default(),
            default_column: default_tenant_column(),
            tables: HashMap::new(),
            global_tables: Vec::new(),
            auto_detect: AutoDetectConfig::default(),
        }
    }
}

/// Tenant identifier type configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TenantIdConfig {
    /// Type of tenant identifier (uuid, integer, string).
    #[serde(default = "default_tenant_type", rename = "type")]
    pub id_type: String,

    /// Human-readable description.
    #[serde(default)]
    pub description: Option<String>,
}

impl Default for TenantIdConfig {
    fn default() -> Self {
        Self {
            id_type: default_tenant_type(),
            description: None,
        }
    }
}

/// Per-table tenancy configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableTenancyConfig {
    /// The column name used for tenant isolation in this table.
    #[serde(default)]
    pub tenant_column: Option<String>,

    /// Column name alias (for compatibility).
    #[serde(default)]
    pub column: Option<String>,

    /// The type of the tenant column (uuid, integer, string).
    #[serde(default)]
    pub tenant_type: Option<String>,

    /// Whether this is a global table (no tenant scoping).
    #[serde(default)]
    pub global: bool,

    /// Foreign key reference for inherited tenancy.
    /// Format: "parent_table.fk_column"
    #[serde(default)]
    pub tenant_via: Option<String>,

    /// Foreign key column in this table.
    #[serde(default)]
    pub fk_column: Option<String>,
}

impl TableTenancyConfig {
    /// Get the effective tenant column name.
    pub fn get_column(&self) -> Option<&str> {
        self.tenant_column
            .as_deref()
            .or(self.column.as_deref())
    }
}

/// Auto-detection configuration for tables not explicitly configured.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoDetectConfig {
    /// Whether auto-detection is enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Column names to look for (in order of priority).
    #[serde(default = "default_auto_detect_columns")]
    pub columns: Vec<String>,
}

impl Default for AutoDetectConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            columns: default_auto_detect_columns(),
        }
    }
}

impl TenancyConfig {
    /// Load tenancy configuration from a YAML file.
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let content = fs::read_to_string(path.as_ref())?;
        Self::from_yaml(&content)
    }

    /// Load tenancy configuration from a file path relative to a base directory.
    ///
    /// If the tenancy_file path is absolute, it is used directly.
    /// Otherwise, it is resolved relative to the base_dir.
    ///
    /// Returns the loaded configuration, or a default with warning if file is not found.
    pub fn load_from_path(
        tenancy_file: impl AsRef<Path>,
        base_dir: impl AsRef<Path>,
    ) -> Result<Self, ConfigError> {
        let tenancy_file = tenancy_file.as_ref();
        let tenancy_path = if tenancy_file.is_absolute() {
            tenancy_file.to_path_buf()
        } else {
            base_dir.as_ref().join(tenancy_file)
        };

        if tenancy_path.exists() {
            Self::from_file(&tenancy_path)
        } else {
            Err(ConfigError::Config(format!(
                "Tenancy file not found: {}",
                tenancy_path.display()
            )))
        }
    }

    /// Parse tenancy configuration from YAML content.
    pub fn from_yaml(content: &str) -> Result<Self, ConfigError> {
        serde_yaml::from_str(content).map_err(ConfigError::from)
    }

    /// Get the tenant column for a given table.
    ///
    /// Returns the column name to use for RLS injection, or None if the table
    /// is global (no tenant scoping).
    pub fn get_tenant_column(&self, table_name: &str) -> Option<&str> {
        // Check if table is in global list
        if self.global_tables.iter().any(|t| t == table_name) {
            return None;
        }

        // Check for per-table configuration
        if let Some(table_config) = self.tables.get(table_name) {
            // Check if explicitly marked as global
            if table_config.global {
                return None;
            }

            // Return per-table column if configured
            if let Some(col) = table_config.get_column() {
                return Some(col);
            }
        }

        // Use default column
        Some(&self.default_column)
    }

    /// Check if a table is global (no tenant scoping).
    pub fn is_global_table(&self, table_name: &str) -> bool {
        // Check global list
        if self.global_tables.iter().any(|t| t == table_name) {
            return true;
        }

        // Check per-table configuration
        if let Some(table_config) = self.tables.get(table_name) {
            return table_config.global;
        }

        false
    }

    /// Get the tenant via configuration for FK-inherited tenancy.
    pub fn get_tenant_via(&self, table_name: &str) -> Option<(&str, &str)> {
        self.tables.get(table_name).and_then(|config| {
            config.tenant_via.as_ref().and_then(|via| {
                // Parse "parent_table.fk_column" format
                let parts: Vec<&str> = via.split('.').collect();
                if parts.len() == 2 {
                    Some((parts[0], parts[1]))
                } else {
                    None
                }
            })
        })
    }

    /// Get the FK column for inherited tenancy.
    pub fn get_fk_column(&self, table_name: &str) -> Option<&str> {
        self.tables
            .get(table_name)
            .and_then(|config| config.fk_column.as_deref())
    }
}

// Default value functions
fn default_tenant_column() -> String {
    "tenant_id".to_string()
}

fn default_tenant_type() -> String {
    "uuid".to_string()
}

fn default_true() -> bool {
    true
}

fn default_auto_detect_columns() -> Vec<String> {
    vec![
        "tenant_id".to_string(),
        "organization_id".to_string(),
        "customer_id".to_string(),
        "account_id".to_string(),
        "client_id".to_string(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_tenant_column() {
        let config = TenancyConfig::default();
        assert_eq!(config.get_tenant_column("orders"), Some("tenant_id"));
    }

    #[test]
    fn test_per_table_override() {
        let mut config = TenancyConfig::default();
        config.tables.insert(
            "orders".to_string(),
            TableTenancyConfig {
                tenant_column: Some("customer_id".to_string()),
                column: None,
                tenant_type: None,
                global: false,
                tenant_via: None,
                fk_column: None,
            },
        );
        assert_eq!(config.get_tenant_column("orders"), Some("customer_id"));
        assert_eq!(config.get_tenant_column("users"), Some("tenant_id"));
    }

    #[test]
    fn test_global_tables() {
        let mut config = TenancyConfig::default();
        config.global_tables.push("products".to_string());
        assert_eq!(config.get_tenant_column("products"), None);
        assert!(config.is_global_table("products"));
    }

    #[test]
    fn test_parse_tenancy_yaml() {
        let yaml = r#"
tenant_id:
  type: uuid
  description: "Organization UUID"

default_column: organization_id

tables:
  customers:
    tenant_column: organization_id
  orders:
    tenant_column: customer_org_id
  products:
    global: true
"#;
        let config = TenancyConfig::from_yaml(yaml).unwrap();
        assert_eq!(config.default_column, "organization_id");
        assert_eq!(
            config.get_tenant_column("customers"),
            Some("organization_id")
        );
        assert_eq!(
            config.get_tenant_column("orders"),
            Some("customer_org_id")
        );
        assert!(config.is_global_table("products"));
    }
}
