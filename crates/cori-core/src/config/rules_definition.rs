//! Rules definition types for user-modifiable column rules.
//!
//! This module defines types for the rules configuration that controls tenancy,
//! soft delete, and column validation. Unlike the schema definition (auto-generated),
//! rules are user-edited.
//!
//! # Rules Location
//!
//! By convention, rules are stored at `schema/rules.yaml` relative to the
//! project root.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

use super::ConfigError;

/// User-modifiable column rules definition.
///
/// This structure defines tenancy configuration, soft delete behavior,
/// and column validation rules. It is user-edited (not auto-generated).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RulesDefinition {
    /// Rules version (semver format).
    pub version: String,

    /// Per-table rules configuration.
    #[serde(default)]
    pub tables: HashMap<String, TableRules>,
}

impl Default for RulesDefinition {
    fn default() -> Self {
        Self {
            version: "1.0.0".to_string(),
            tables: HashMap::new(),
        }
    }
}

impl RulesDefinition {
    /// Load rules definition from a YAML file.
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let content = fs::read_to_string(path.as_ref())?;
        Self::from_yaml(&content)
    }

    /// Parse rules definition from YAML content.
    pub fn from_yaml(content: &str) -> Result<Self, ConfigError> {
        serde_yaml::from_str(content).map_err(ConfigError::from)
    }

    /// Get rules for a specific table.
    pub fn get_table_rules(&self, table: &str) -> Option<&TableRules> {
        self.tables.get(table)
    }

    /// Check if a table is global (no tenant scoping).
    pub fn is_global_table(&self, table: &str) -> bool {
        self.tables
            .get(table)
            .map(|r| r.global.unwrap_or(false))
            .unwrap_or(false)
    }

    /// Get the tenant configuration for a table.
    pub fn get_tenant_config(&self, table: &str) -> Option<&TenantConfig> {
        self.tables.get(table).and_then(|r| r.tenant.as_ref())
    }
}

/// Rules for a single table.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TableRules {
    /// Human-readable description of the table.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Tenant isolation configuration.
    ///
    /// Either a direct column name or inherited via FK.
    /// Mutually exclusive with `global`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tenant: Option<TenantConfig>,

    /// If true, this table is shared across all tenants (no tenant filtering).
    ///
    /// Mutually exclusive with `tenant`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub global: Option<bool>,

    /// Soft delete configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub soft_delete: Option<SoftDeleteConfig>,

    /// Per-column rules.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub columns: HashMap<String, ColumnRules>,
}

impl TableRules {
    /// Check if this table is tenant-scoped (has direct or inherited tenancy).
    pub fn is_tenant_scoped(&self) -> bool {
        self.tenant.is_some() && !self.global.unwrap_or(false)
    }

    /// Get the direct tenant column name (if using direct tenancy).
    pub fn get_direct_tenant_column(&self) -> Option<&str> {
        match &self.tenant {
            Some(TenantConfig::Direct(col)) => Some(col.as_str()),
            _ => None,
        }
    }

    /// Get the inherited tenant configuration (if using FK-inherited tenancy).
    pub fn get_inherited_tenant(&self) -> Option<&InheritedTenant> {
        match &self.tenant {
            Some(TenantConfig::Inherited(inherited)) => Some(inherited),
            _ => None,
        }
    }
}

/// Tenant configuration for a table.
///
/// Can be either a direct column name or inherited via foreign key.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum TenantConfig {
    /// Direct tenant column in this table.
    Direct(String),

    /// Tenant inherited via foreign key relationship.
    Inherited(InheritedTenant),
}

/// Configuration for FK-inherited tenancy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InheritedTenant {
    /// Foreign key column in this table.
    pub via: String,

    /// Parent table to inherit tenant from.
    pub references: String,
}

/// Soft delete configuration for a table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SoftDeleteConfig {
    /// Column name used for soft delete (e.g., "deleted_at", "is_deleted").
    pub column: String,

    /// Value that indicates a row is deleted.
    ///
    /// Defaults: timestamp columns → "NOW()", boolean → true
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted_value: Option<SoftDeleteValue>,

    /// Value for non-deleted rows.
    ///
    /// Defaults: timestamp columns → null, boolean → false
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_value: Option<SoftDeleteValue>,
}

/// Possible values for soft delete columns.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SoftDeleteValue {
    /// Boolean value.
    Boolean(bool),

    /// SQL expression (e.g., "NOW()").
    Expression(String),

    /// Null value (for active_value).
    Null,
}

/// Rules for a single column.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ColumnRules {
    /// Human-readable description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Reference to a type defined in types.yaml.
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub type_ref: Option<String>,

    /// Inline regex pattern for validation (overrides type).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,

    /// Whitelist of allowed values.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_values: Option<Vec<serde_json::Value>>,

    /// Categorization tags (e.g., "pii", "sensitive", "immutable", "auto").
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
}

impl ColumnRules {
    /// Check if this column has a specific tag.
    pub fn has_tag(&self, tag: &str) -> bool {
        self.tags.iter().any(|t| t == tag)
    }

    /// Check if this column is marked as PII.
    pub fn is_pii(&self) -> bool {
        self.has_tag("pii")
    }

    /// Check if this column is marked as sensitive.
    pub fn is_sensitive(&self) -> bool {
        self.has_tag("sensitive")
    }

    /// Check if this column is marked as immutable.
    pub fn is_immutable(&self) -> bool {
        self.has_tag("immutable")
    }

    /// Check if this column is auto-generated.
    pub fn is_auto(&self) -> bool {
        self.has_tag("auto")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_rules_definition() {
        let yaml = r#"
version: "1.0.0"
tables:
  customers:
    description: "Customer accounts"
    tenant: organization_id
    columns:
      email:
        type: email
        tags: [pii]
  orders:
    tenant:
      via: customer_id
      references: customers
    soft_delete:
      column: deleted_at
      deleted_value: "NOW()"
  products:
    global: true
"#;

        let rules = RulesDefinition::from_yaml(yaml).unwrap();
        assert_eq!(rules.version, "1.0.0");
        assert_eq!(rules.tables.len(), 3);

        // Check direct tenant config
        let customers = rules.get_table_rules("customers").unwrap();
        assert_eq!(
            customers.get_direct_tenant_column(),
            Some("organization_id")
        );

        // Check inherited tenant config
        let orders = rules.get_table_rules("orders").unwrap();
        let inherited = orders.get_inherited_tenant().unwrap();
        assert_eq!(inherited.via, "customer_id");
        assert_eq!(inherited.references, "customers");

        // Check global table
        assert!(rules.is_global_table("products"));

        // Check column rules
        let email_rules = customers.columns.get("email").unwrap();
        assert!(email_rules.is_pii());
        assert_eq!(email_rules.type_ref, Some("email".to_string()));
    }

    #[test]
    fn test_soft_delete_config() {
        let yaml = r#"
version: "1.0.0"
tables:
  users:
    soft_delete:
      column: is_deleted
      deleted_value: true
      active_value: false
  orders:
    soft_delete:
      column: deleted_at
      deleted_value: "NOW()"
"#;

        let rules = RulesDefinition::from_yaml(yaml).unwrap();

        let users = rules.get_table_rules("users").unwrap();
        let soft_delete = users.soft_delete.as_ref().unwrap();
        assert_eq!(soft_delete.column, "is_deleted");

        let orders = rules.get_table_rules("orders").unwrap();
        let soft_delete = orders.soft_delete.as_ref().unwrap();
        assert_eq!(soft_delete.column, "deleted_at");
    }
}
