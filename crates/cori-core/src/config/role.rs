//! Role configuration types.
//!
//! This module defines role configurations that control what AI agents can do,
//! including table permissions, column constraints, and custom actions.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

use super::ConfigError;

/// Operations that can be performed on a table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Operation {
    Create,
    Read,
    Update,
    Delete,
}

/// Constraints on an editable column.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ColumnConstraints {
    /// Allowed values (whitelist).
    #[serde(default)]
    pub allowed_values: Option<Vec<String>>,

    /// Regex pattern the value must match.
    #[serde(default)]
    pub pattern: Option<String>,

    /// Minimum value (for numeric columns).
    #[serde(default)]
    pub min: Option<f64>,

    /// Maximum value (for numeric columns).
    #[serde(default)]
    pub max: Option<f64>,

    /// Whether changes require human approval.
    #[serde(default)]
    pub requires_approval: bool,
}

/// Permission configuration for a single table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TablePermissions {
    /// Columns that can be read. Use "*" for all columns.
    #[serde(default)]
    pub readable: ReadableColumns,

    /// Columns that can be edited, with optional constraints.
    #[serde(default)]
    pub editable: EditableColumns,

    /// Allowed operations on this table.
    #[serde(default)]
    pub operations: Option<Vec<Operation>>,

    /// Tenant column override for this table.
    #[serde(default)]
    pub tenant_column: Option<String>,
}

impl Default for TablePermissions {
    fn default() -> Self {
        Self {
            readable: ReadableColumns::default(),
            editable: EditableColumns::default(),
            operations: None,
            tenant_column: None,
        }
    }
}

/// Readable columns can be a list or "*" for all.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ReadableColumns {
    /// All columns are readable.
    All(String), // "*"
    /// Specific columns are readable.
    List(Vec<String>),
}

impl Default for ReadableColumns {
    fn default() -> Self {
        ReadableColumns::List(Vec::new())
    }
}

impl ReadableColumns {
    /// Check if a column is readable.
    pub fn contains(&self, column: &str) -> bool {
        match self {
            ReadableColumns::All(s) => s == "*",
            ReadableColumns::List(cols) => cols.iter().any(|c| c == column),
        }
    }

    /// Get the list of readable columns, if not "all".
    pub fn as_list(&self) -> Option<&[String]> {
        match self {
            ReadableColumns::All(_) => None,
            ReadableColumns::List(cols) => Some(cols),
        }
    }

    /// Check if this represents "all columns".
    pub fn is_all(&self) -> bool {
        matches!(self, ReadableColumns::All(s) if s == "*")
    }

    /// Convert to Vec, returning None if "all".
    pub fn to_vec(&self) -> Option<Vec<String>> {
        match self {
            ReadableColumns::All(_) => None,
            ReadableColumns::List(cols) => Some(cols.clone()),
        }
    }
}

/// Editable columns can be "*" for all, or a map of column -> constraints.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum EditableColumns {
    /// All columns are editable (with default constraints).
    All(String), // "*"
    /// Specific columns with their constraints.
    Map(HashMap<String, ColumnConstraints>),
}

impl Default for EditableColumns {
    fn default() -> Self {
        EditableColumns::Map(HashMap::new())
    }
}

impl EditableColumns {
    /// Check if a column is editable.
    pub fn contains(&self, column: &str) -> bool {
        match self {
            EditableColumns::All(s) => s == "*",
            EditableColumns::Map(cols) => cols.contains_key(column),
        }
    }

    /// Get the constraints for a specific column.
    pub fn get_constraints(&self, column: &str) -> Option<&ColumnConstraints> {
        match self {
            EditableColumns::All(_) => None, // All columns with default constraints
            EditableColumns::Map(cols) => cols.get(column),
        }
    }

    /// Get the map of editable columns, if not "all".
    pub fn as_map(&self) -> Option<&HashMap<String, ColumnConstraints>> {
        match self {
            EditableColumns::All(_) => None,
            EditableColumns::Map(cols) => Some(cols),
        }
    }

    /// Check if this represents "all columns".
    pub fn is_all(&self) -> bool {
        matches!(self, EditableColumns::All(s) if s == "*")
    }

    /// Check if there are no editable columns.
    pub fn is_empty(&self) -> bool {
        match self {
            EditableColumns::All(_) => false,
            EditableColumns::Map(cols) => cols.is_empty(),
        }
    }

    /// Iterate over constraint values (only for Map variant).
    pub fn values(&self) -> impl Iterator<Item = &ColumnConstraints> {
        match self {
            EditableColumns::All(_) => None.into_iter().flatten(),
            EditableColumns::Map(cols) => Some(cols.values()).into_iter().flatten(),
        }
    }

    /// Iterate over column name and constraint pairs (only for Map variant).
    pub fn iter(&self) -> impl Iterator<Item = (&String, &ColumnConstraints)> {
        match self {
            EditableColumns::All(_) => None.into_iter().flatten(),
            EditableColumns::Map(cols) => Some(cols.iter()).into_iter().flatten(),
        }
    }

    /// Get column names as a Vec (only for Map variant).
    pub fn column_names(&self) -> Vec<&str> {
        match self {
            EditableColumns::All(_) => Vec::new(),
            EditableColumns::Map(cols) => cols.keys().map(|s| s.as_str()).collect(),
        }
    }
}

/// A complete role configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoleConfig {
    /// Role name.
    pub name: String,

    /// Human-readable description.
    #[serde(default)]
    pub description: Option<String>,

    /// Table permissions.
    #[serde(default)]
    pub tables: HashMap<String, TablePermissions>,

    /// Tables that are explicitly blocked.
    #[serde(default)]
    pub blocked_tables: Vec<String>,

    /// Maximum rows per query.
    #[serde(default)]
    pub max_rows_per_query: Option<u64>,

    /// Maximum affected rows for mutations.
    #[serde(default)]
    pub max_affected_rows: Option<u64>,

    /// Blocked SQL operations (e.g., DELETE, TRUNCATE, DROP).
    #[serde(default)]
    pub blocked_operations: Vec<String>,

    /// Custom actions for this role.
    #[serde(default)]
    pub custom_actions: Vec<CustomAction>,

    /// Include shared action files.
    #[serde(default)]
    pub include_actions: Vec<String>,
}

impl Default for RoleConfig {
    fn default() -> Self {
        Self {
            name: "default".to_string(),
            description: None,
            tables: HashMap::new(),
            blocked_tables: Vec::new(),
            max_rows_per_query: Some(100),
            max_affected_rows: Some(10),
            blocked_operations: Vec::new(),
            custom_actions: Vec::new(),
            include_actions: Vec::new(),
        }
    }
}

impl RoleConfig {
    /// Load a role configuration from a YAML file.
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let content = fs::read_to_string(path.as_ref())?;
        Self::from_yaml(&content)
    }

    /// Parse a role configuration from YAML content.
    pub fn from_yaml(content: &str) -> Result<Self, ConfigError> {
        serde_yaml::from_str(content).map_err(ConfigError::from)
    }

    /// Check if a table is accessible to this role.
    pub fn can_access_table(&self, table: &str) -> bool {
        !self.blocked_tables.contains(&table.to_string()) && self.tables.contains_key(table)
    }

    /// Check if a column is readable.
    pub fn can_read_column(&self, table: &str, column: &str) -> bool {
        if let Some(perms) = self.tables.get(table) {
            perms.readable.contains(column)
        } else {
            false
        }
    }

    /// Check if a column is editable.
    pub fn can_edit_column(&self, table: &str, column: &str) -> bool {
        if let Some(perms) = self.tables.get(table) {
            perms.editable.contains(column)
        } else {
            false
        }
    }

    /// Get the constraints for an editable column.
    pub fn get_column_constraints(&self, table: &str, column: &str) -> Option<&ColumnConstraints> {
        self.tables
            .get(table)
            .and_then(|t| t.editable.get_constraints(column))
    }

    /// Check if the role has read permission for a table.
    pub fn can_read(&self, table: &str) -> bool {
        if let Some(perms) = self.tables.get(table) {
            if let Some(ops) = &perms.operations {
                ops.contains(&Operation::Read)
            } else {
                // Default: can read if there are readable columns
                !matches!(perms.readable, ReadableColumns::List(ref cols) if cols.is_empty())
            }
        } else {
            false
        }
    }

    /// Check if the role has create permission for a table.
    pub fn can_create(&self, table: &str) -> bool {
        if let Some(perms) = self.tables.get(table) {
            if let Some(ops) = &perms.operations {
                ops.contains(&Operation::Create) && !perms.editable.is_empty()
            } else {
                // Default: can create if there are editable columns
                !perms.editable.is_empty()
            }
        } else {
            false
        }
    }

    /// Check if the role has update permission for a table.
    pub fn can_update(&self, table: &str) -> bool {
        if let Some(perms) = self.tables.get(table) {
            if let Some(ops) = &perms.operations {
                ops.contains(&Operation::Update) && !perms.editable.is_empty()
            } else {
                // Default: can update if there are editable columns
                !perms.editable.is_empty()
            }
        } else {
            false
        }
    }

    /// Check if the role has delete permission for a table.
    pub fn can_delete(&self, table: &str) -> bool {
        // Check if DELETE is globally blocked
        if self
            .blocked_operations
            .iter()
            .any(|op| op.eq_ignore_ascii_case("DELETE"))
        {
            return false;
        }

        if let Some(perms) = self.tables.get(table) {
            if let Some(ops) = &perms.operations {
                ops.contains(&Operation::Delete)
            } else {
                false // Delete must be explicitly granted
            }
        } else {
            false
        }
    }

    /// Check if any editable column in a table requires approval.
    pub fn table_requires_approval(&self, table: &str) -> bool {
        if let Some(perms) = self.tables.get(table) {
            perms
                .editable
                .values()
                .any(|constraints| constraints.requires_approval)
        } else {
            false
        }
    }

    /// Get the columns that require approval for a table.
    pub fn get_approval_columns(&self, table: &str) -> Vec<&str> {
        if let Some(perms) = self.tables.get(table) {
            perms
                .editable
                .iter()
                .filter(|(_, constraints)| constraints.requires_approval)
                .map(|(col, _)| col.as_str())
                .collect()
        } else {
            Vec::new()
        }
    }

    /// Get readable columns for a table.
    pub fn get_readable_columns(&self, table: &str) -> Option<&ReadableColumns> {
        self.tables.get(table).map(|t| &t.readable)
    }

    /// Get editable columns for a table.
    pub fn get_editable_columns(&self, table: &str) -> Option<&EditableColumns> {
        self.tables.get(table).map(|t| &t.editable)
    }
}

/// A custom action definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomAction {
    /// Action name.
    pub name: String,

    /// Human-readable description.
    #[serde(default)]
    pub description: Option<String>,

    /// Input parameters.
    #[serde(default)]
    pub inputs: HashMap<String, CustomActionInput>,

    /// Whether this action requires human approval.
    #[serde(default)]
    pub requires_approval: bool,
}

/// Input parameter for a custom action.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomActionInput {
    /// Parameter type (e.g., "integer", "string", "boolean").
    #[serde(rename = "type")]
    pub param_type: String,

    /// Whether this parameter is required.
    #[serde(default)]
    pub required: bool,

    /// Description of the parameter.
    #[serde(default)]
    pub description: Option<String>,

    /// Allowed values (for enum-like parameters).
    #[serde(default, rename = "enum")]
    pub enum_values: Option<Vec<String>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_role_config() {
        let yaml = r#"
name: support_agent
description: "AI agent for customer support"

tables:
  customers:
    readable: [id, name, email]
    editable: {}
  tickets:
    readable: [id, subject, status]
    editable:
      status:
        allowed_values: [open, closed]
      priority:
        requires_approval: true

blocked_tables:
  - users
  - billing

max_rows_per_query: 100
blocked_operations:
  - DELETE
"#;

        let config = RoleConfig::from_yaml(yaml).unwrap();

        assert_eq!(config.name, "support_agent");
        assert!(config.can_access_table("customers"));
        assert!(config.can_read_column("customers", "id"));
        assert!(!config.can_edit_column("customers", "id"));
        assert!(config.can_edit_column("tickets", "status"));
        assert!(!config.can_delete("tickets"));
        assert!(config.table_requires_approval("tickets"));
        assert_eq!(config.get_approval_columns("tickets"), vec!["priority"]);
    }

    #[test]
    fn test_readable_all_columns() {
        let yaml = r#"
name: admin
tables:
  users:
    readable: "*"
    editable: {}
"#;

        let config = RoleConfig::from_yaml(yaml).unwrap();
        assert!(config.can_read_column("users", "any_column"));
    }
}
