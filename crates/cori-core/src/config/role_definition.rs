//! Role definition types with readable/creatable/updatable/deletable permission model.
//!
//! This module defines the role configuration that controls what AI agents can do,
//! including table permissions, column constraints, and approval requirements.
//!
//! # Roles Location
//!
//! By convention, roles are stored at `roles/*.yaml` relative to the project root
//! (one file per role).
//!
//! # Permission Model
//!
//! The permission model uses four permission types:
//! - `readable`: Columns that can be read (SELECT)
//! - `creatable`: Columns that can be set on INSERT with constraints
//! - `updatable`: Columns that can be modified on UPDATE with constraints
//! - `deletable`: Whether DELETE is allowed with options

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

use super::ConfigError;

/// Role definition for AI agent access control.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoleDefinition {
    /// Unique role identifier (e.g., "support_agent").
    pub name: String,

    /// Human-readable description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Default approval configuration for all requires_approval flags.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approvals: Option<ApprovalConfig>,

    /// Table permissions.
    #[serde(default)]
    pub tables: HashMap<String, TablePermissions>,
}

impl Default for RoleDefinition {
    fn default() -> Self {
        Self {
            name: "default".to_string(),
            description: None,
            approvals: None,
            tables: HashMap::new(),
        }
    }
}

impl RoleDefinition {
    /// Load role definition from a YAML file.
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let content = fs::read_to_string(path.as_ref())?;
        Self::from_yaml(&content)
    }

    /// Parse role definition from YAML content.
    pub fn from_yaml(content: &str) -> Result<Self, ConfigError> {
        serde_yaml::from_str(content).map_err(ConfigError::from)
    }

    /// Check if a table is accessible to this role.
    pub fn can_access_table(&self, table: &str) -> bool {
        self.tables.contains_key(table)
    }

    /// Check if a column is readable.
    pub fn can_read_column(&self, table: &str, column: &str) -> bool {
        self.tables
            .get(table)
            .map(|t| t.readable.contains(column))
            .unwrap_or(false)
    }

    /// Check if a column is creatable.
    pub fn can_create_column(&self, table: &str, column: &str) -> bool {
        self.tables
            .get(table)
            .map(|t| t.creatable.contains(column))
            .unwrap_or(false)
    }

    /// Check if a column is updatable.
    pub fn can_update_column(&self, table: &str, column: &str) -> bool {
        self.tables
            .get(table)
            .map(|t| t.updatable.contains(column))
            .unwrap_or(false)
    }

    /// Check if deletion is allowed for a table.
    pub fn can_delete(&self, table: &str) -> bool {
        self.tables
            .get(table)
            .map(|t| t.deletable.is_allowed())
            .unwrap_or(false)
    }

    /// Check if the role has read permission for a table.
    pub fn can_read(&self, table: &str) -> bool {
        self.tables
            .get(table)
            .map(|t| !t.readable.is_empty())
            .unwrap_or(false)
    }

    /// Check if the role has create permission for a table.
    pub fn can_create(&self, table: &str) -> bool {
        self.tables
            .get(table)
            .map(|t| !t.creatable.is_empty())
            .unwrap_or(false)
    }

    /// Check if the role has update permission for a table.
    pub fn can_update(&self, table: &str) -> bool {
        self.tables
            .get(table)
            .map(|t| !t.updatable.is_empty())
            .unwrap_or(false)
    }

    /// Get table permissions.
    pub fn get_table_permissions(&self, table: &str) -> Option<&TablePermissions> {
        self.tables.get(table)
    }

    /// Get creatable column constraints.
    pub fn get_creatable_constraints(
        &self,
        table: &str,
        column: &str,
    ) -> Option<&CreatableColumnConstraints> {
        self.tables
            .get(table)
            .and_then(|t| t.creatable.get_constraints(column))
    }

    /// Get updatable column constraints.
    pub fn get_updatable_constraints(
        &self,
        table: &str,
        column: &str,
    ) -> Option<&UpdatableColumnConstraints> {
        self.tables
            .get(table)
            .and_then(|t| t.updatable.get_constraints(column))
    }

    /// Get readable columns for a table.
    pub fn get_readable_columns(&self, table: &str) -> Option<&ReadableConfig> {
        self.tables.get(table).map(|t| &t.readable)
    }

    /// Get max_per_page for a table's list operations.
    pub fn get_max_per_page(&self, table: &str) -> Option<u64> {
        self.tables
            .get(table)
            .and_then(|t| t.readable.max_per_page())
    }

    /// Check if any column in a table requires approval (for create or update).
    pub fn table_requires_approval(&self, table: &str) -> bool {
        if let Some(perms) = self.tables.get(table) {
            // Check creatable columns
            if let Some(map) = perms.creatable.as_map() {
                if map.values().any(|c| c.requires_approval.is_some()) {
                    return true;
                }
            }
            // Check updatable columns
            if let Some(map) = perms.updatable.as_map() {
                if map.values().any(|c| c.requires_approval.is_some()) {
                    return true;
                }
            }
            // Check deletable
            if let DeletablePermission::WithConstraints(opts) = &perms.deletable {
                if opts.requires_approval.is_some() {
                    return true;
                }
            }
        }
        false
    }

    /// Get the columns that require approval for a table.
    pub fn get_approval_columns(&self, table: &str) -> Vec<&str> {
        let mut cols = Vec::new();
        if let Some(perms) = self.tables.get(table) {
            // Check creatable columns
            if let Some(map) = perms.creatable.as_map() {
                for (col, constraints) in map {
                    if constraints.requires_approval.is_some() {
                        cols.push(col.as_str());
                    }
                }
            }
            // Check updatable columns
            if let Some(map) = perms.updatable.as_map() {
                for (col, constraints) in map {
                    if constraints.requires_approval.is_some() {
                        cols.push(col.as_str());
                    }
                }
            }
        }
        cols
    }

    /// Get creatable columns for a table.
    pub fn get_creatable_columns(&self, table: &str) -> Option<&CreatableColumns> {
        self.tables.get(table).map(|t| &t.creatable)
    }

    /// Get updatable columns for a table.
    pub fn get_updatable_columns(&self, table: &str) -> Option<&UpdatableColumns> {
        self.tables.get(table).map(|t| &t.updatable)
    }
}

/// Permission configuration for a single table.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TablePermissions {
    /// Columns that can be read (SELECT) with optional pagination limit.
    #[serde(default)]
    pub readable: ReadableConfig,

    /// Columns that can be set on INSERT with constraints.
    #[serde(default)]
    pub creatable: CreatableColumns,

    /// Columns that can be modified on UPDATE with constraints.
    #[serde(default)]
    pub updatable: UpdatableColumns,

    /// Whether records can be deleted from this table.
    #[serde(default)]
    pub deletable: DeletablePermission,
}

/// List of columns (can be "*" for all or a specific list).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ColumnList {
    /// All columns.
    All(AllColumns),
    /// Specific columns.
    List(Vec<String>),
}

/// Marker type for "*" (all columns).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct AllColumns;

impl TryFrom<String> for AllColumns {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value == "*" {
            Ok(AllColumns)
        } else {
            Err(format!("Expected '*', got '{}'", value))
        }
    }
}

impl From<AllColumns> for String {
    fn from(_: AllColumns) -> Self {
        "*".to_string()
    }
}

impl Default for ColumnList {
    fn default() -> Self {
        ColumnList::List(Vec::new())
    }
}

impl ColumnList {
    /// Check if a column is included.
    pub fn contains(&self, column: &str) -> bool {
        match self {
            ColumnList::All(_) => true,
            ColumnList::List(cols) => cols.iter().any(|c| c == column),
        }
    }

    /// Check if this represents "all columns".
    pub fn is_all(&self) -> bool {
        matches!(self, ColumnList::All(_))
    }

    /// Check if the list is empty.
    pub fn is_empty(&self) -> bool {
        match self {
            ColumnList::All(_) => false,
            ColumnList::List(cols) => cols.is_empty(),
        }
    }

    /// Get the list of columns (None if "all").
    pub fn as_list(&self) -> Option<&[String]> {
        match self {
            ColumnList::All(_) => None,
            ColumnList::List(cols) => Some(cols),
        }
    }
}

/// Readable configuration with columns and optional max_per_page.
/// Supports three formats:
/// - `"*"` for all columns
/// - `["col1", "col2"]` for specific columns
/// - `{columns: [...], max_per_page: N}` for columns with pagination limit
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ReadableConfig {
    /// All columns (shorthand: "*").
    All(AllColumns),
    /// List of specific column names (shorthand: ["col1", "col2"]).
    List(Vec<String>),
    /// Full config with columns and optional max_per_page.
    Config(ReadableConfigFull),
}

/// Full readable configuration with columns and optional max_per_page.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadableConfigFull {
    /// Columns that can be read.
    pub columns: ColumnList,
    /// Maximum number of rows per page for list operations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_per_page: Option<u64>,
}

impl Default for ReadableConfig {
    fn default() -> Self {
        ReadableConfig::List(Vec::new())
    }
}

impl ReadableConfig {
    /// Check if a column is included.
    pub fn contains(&self, column: &str) -> bool {
        match self {
            ReadableConfig::All(_) => true,
            ReadableConfig::List(cols) => cols.iter().any(|c| c == column),
            ReadableConfig::Config(cfg) => cfg.columns.contains(column),
        }
    }

    /// Check if this represents "all columns".
    pub fn is_all(&self) -> bool {
        match self {
            ReadableConfig::All(_) => true,
            ReadableConfig::List(_) => false,
            ReadableConfig::Config(cfg) => cfg.columns.is_all(),
        }
    }

    /// Check if the list is empty.
    pub fn is_empty(&self) -> bool {
        match self {
            ReadableConfig::All(_) => false,
            ReadableConfig::List(cols) => cols.is_empty(),
            ReadableConfig::Config(cfg) => cfg.columns.is_empty(),
        }
    }

    /// Get the list of columns (None if "all").
    pub fn as_list(&self) -> Option<&[String]> {
        match self {
            ReadableConfig::All(_) => None,
            ReadableConfig::List(cols) => Some(cols),
            ReadableConfig::Config(cfg) => cfg.columns.as_list(),
        }
    }

    /// Get the max_per_page limit if configured.
    pub fn max_per_page(&self) -> Option<u64> {
        match self {
            ReadableConfig::All(_) | ReadableConfig::List(_) => None,
            ReadableConfig::Config(cfg) => cfg.max_per_page,
        }
    }

    /// Get the underlying ColumnList.
    pub fn columns(&self) -> ColumnList {
        match self {
            ReadableConfig::All(all) => ColumnList::All(all.clone()),
            ReadableConfig::List(cols) => ColumnList::List(cols.clone()),
            ReadableConfig::Config(cfg) => cfg.columns.clone(),
        }
    }
}

/// Creatable columns configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CreatableColumns {
    /// All columns are creatable with default constraints.
    All(AllColumns),
    /// Map of column names to their INSERT constraints.
    Map(HashMap<String, CreatableColumnConstraints>),
}

impl Default for CreatableColumns {
    fn default() -> Self {
        CreatableColumns::Map(HashMap::new())
    }
}

impl CreatableColumns {
    /// Check if a column is creatable.
    pub fn contains(&self, column: &str) -> bool {
        match self {
            CreatableColumns::All(_) => true,
            CreatableColumns::Map(cols) => cols.contains_key(column),
        }
    }

    /// Get constraints for a column.
    pub fn get_constraints(&self, column: &str) -> Option<&CreatableColumnConstraints> {
        match self {
            CreatableColumns::All(_) => None,
            CreatableColumns::Map(cols) => cols.get(column),
        }
    }

    /// Check if this represents "all columns".
    pub fn is_all(&self) -> bool {
        matches!(self, CreatableColumns::All(_))
    }

    /// Check if there are no creatable columns.
    pub fn is_empty(&self) -> bool {
        match self {
            CreatableColumns::All(_) => false,
            CreatableColumns::Map(cols) => cols.is_empty(),
        }
    }

    /// Get the map of columns (None if "all").
    pub fn as_map(&self) -> Option<&HashMap<String, CreatableColumnConstraints>> {
        match self {
            CreatableColumns::All(_) => None,
            CreatableColumns::Map(cols) => Some(cols),
        }
    }

    /// Get column names.
    pub fn column_names(&self) -> Vec<&str> {
        match self {
            CreatableColumns::All(_) => Vec::new(),
            CreatableColumns::Map(cols) => cols.keys().map(|s| s.as_str()).collect(),
        }
    }
}

/// Constraints on a column for INSERT operations.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CreatableColumnConstraints {
    /// If true, this role must provide a value on INSERT.
    #[serde(default)]
    pub required: bool,

    /// Role-specific default value if not provided.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<serde_json::Value>,

    /// Subset of allowed values that this role can use.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restrict_to: Option<Vec<serde_json::Value>>,

    /// Whether creating with this column requires human approval.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requires_approval: Option<ApprovalRequirement>,

    /// Instructions for AI agents on how to use this column.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guidance: Option<String>,
}

/// Updatable columns configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum UpdatableColumns {
    /// All columns are updatable with default constraints.
    All(AllColumns),
    /// Map of column names to their UPDATE constraints.
    Map(HashMap<String, UpdatableColumnConstraints>),
}

impl Default for UpdatableColumns {
    fn default() -> Self {
        UpdatableColumns::Map(HashMap::new())
    }
}

impl UpdatableColumns {
    /// Check if a column is updatable.
    pub fn contains(&self, column: &str) -> bool {
        match self {
            UpdatableColumns::All(_) => true,
            UpdatableColumns::Map(cols) => cols.contains_key(column),
        }
    }

    /// Get constraints for a column.
    pub fn get_constraints(&self, column: &str) -> Option<&UpdatableColumnConstraints> {
        match self {
            UpdatableColumns::All(_) => None,
            UpdatableColumns::Map(cols) => cols.get(column),
        }
    }

    /// Check if this represents "all columns".
    pub fn is_all(&self) -> bool {
        matches!(self, UpdatableColumns::All(_))
    }

    /// Check if there are no updatable columns.
    pub fn is_empty(&self) -> bool {
        match self {
            UpdatableColumns::All(_) => false,
            UpdatableColumns::Map(cols) => cols.is_empty(),
        }
    }

    /// Get the map of columns (None if "all").
    pub fn as_map(&self) -> Option<&HashMap<String, UpdatableColumnConstraints>> {
        match self {
            UpdatableColumns::All(_) => None,
            UpdatableColumns::Map(cols) => Some(cols),
        }
    }

    /// Get column names.
    pub fn column_names(&self) -> Vec<&str> {
        match self {
            UpdatableColumns::All(_) => Vec::new(),
            UpdatableColumns::Map(cols) => cols.keys().map(|s| s.as_str()).collect(),
        }
    }
}

/// Constraints on a column for UPDATE operations.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UpdatableColumnConstraints {
    /// Conditions on current row (old.*) and incoming values (new.*).
    /// Use object for AND logic, array for OR logic.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub only_when: Option<OnlyWhen>,

    /// Whether updating this column requires human approval.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requires_approval: Option<ApprovalRequirement>,

    /// Instructions for AI agents on how to use this column.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guidance: Option<String>,
}

/// Conditions for when an update is allowed.
/// Keys use 'old.<column>' for current row values or 'new.<column>' for incoming values.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum OnlyWhen {
    /// Single condition set (AND logic).
    Single(HashMap<String, ColumnCondition>),
    /// Multiple condition sets (OR logic) - update allowed if ANY matches.
    Multiple(Vec<HashMap<String, ColumnCondition>>),
}

impl OnlyWhen {
    /// Get all condition sets (single wrapped in vec, or multiple as-is).
    pub fn condition_sets(&self) -> Vec<&HashMap<String, ColumnCondition>> {
        match self {
            OnlyWhen::Single(conditions) => vec![conditions],
            OnlyWhen::Multiple(sets) => sets.iter().collect(),
        }
    }

    /// Check if this is a simple restriction on new value (new.<col>: [values]).
    pub fn get_new_value_restriction(&self, column: &str) -> Option<&Vec<serde_json::Value>> {
        let key = format!("new.{}", column);
        if let OnlyWhen::Single(conditions) = self {
            if conditions.len() == 1 {
                if let Some(ColumnCondition::In(values)) = conditions.get(&key) {
                    return Some(values);
                }
            }
        }
        None
    }

    /// Check if any condition references old.* values (current row data).
    pub fn has_old_conditions(&self) -> bool {
        for condition_set in self.condition_sets() {
            for key in condition_set.keys() {
                if key.starts_with("old.") {
                    return true;
                }
            }
        }
        false
    }
}

impl UpdatableColumnConstraints {
    /// Check if the constraints have any only_when conditions.
    pub fn has_conditions(&self) -> bool {
        self.only_when.is_some()
    }
}

/// Condition on a column's current value.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ColumnCondition {
    /// Column must be one of these values (IN).
    In(Vec<serde_json::Value>),

    /// Comparison condition (object with comparison operators).
    Comparison(ComparisonCondition),

    /// Column must equal this single value (scalar fallback).
    Equals(serde_json::Value),
}

/// Detailed comparison condition.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ComparisonCondition {
    /// Column must equal this value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub equals: Option<serde_json::Value>,

    /// Column must not equal this value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_equals: Option<serde_json::Value>,

    /// Column must be greater than this value or column reference (e.g., "old.quantity").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub greater_than: Option<NumberOrColumnRef>,

    /// Column must be greater than or equal to this value or column reference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub greater_than_or_equal: Option<NumberOrColumnRef>,

    /// Column must be lower than this value or column reference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lower_than: Option<NumberOrColumnRef>,

    /// Column must be lower than or equal to this value or column reference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lower_than_or_equal: Option<NumberOrColumnRef>,

    /// Column must not be null.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_null: Option<bool>,

    /// Column must be null.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_null: Option<bool>,

    /// Column must be one of these values.
    #[serde(rename = "in", default, skip_serializing_if = "Option::is_none")]
    pub in_values: Option<Vec<serde_json::Value>>,

    /// Column must not be one of these values.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_in: Option<Vec<serde_json::Value>>,

    /// Column must start with this value or column reference (e.g., "old.notes" for append-only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub starts_with: Option<String>,
}

/// A numeric value or a reference to another column (e.g., "old.quantity").
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum NumberOrColumnRef {
    /// A literal numeric value.
    Number(f64),
    /// A reference to another column using "old.<column>" or "new.<column>" syntax.
    ColumnRef(String),
}

impl NumberOrColumnRef {
    /// Check if this is a column reference.
    pub fn is_column_ref(&self) -> bool {
        matches!(self, NumberOrColumnRef::ColumnRef(_))
    }

    /// Get the column reference if this is one.
    pub fn as_column_ref(&self) -> Option<&str> {
        match self {
            NumberOrColumnRef::ColumnRef(s) => Some(s),
            NumberOrColumnRef::Number(_) => None,
        }
    }

    /// Get the numeric value if this is one.
    pub fn as_number(&self) -> Option<f64> {
        match self {
            NumberOrColumnRef::Number(n) => Some(*n),
            NumberOrColumnRef::ColumnRef(_) => None,
        }
    }
}

/// Deletable permission configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DeletablePermission {
    /// Simple boolean: true = allowed, false = denied.
    Allowed(bool),

    /// Deletion with constraints.
    WithConstraints(DeletableConstraints),
}

impl Default for DeletablePermission {
    fn default() -> Self {
        DeletablePermission::Allowed(false)
    }
}

impl DeletablePermission {
    /// Check if deletion is allowed.
    pub fn is_allowed(&self) -> bool {
        match self {
            DeletablePermission::Allowed(allowed) => *allowed,
            DeletablePermission::WithConstraints(_) => true,
        }
    }

    /// Check if deletion requires approval.
    pub fn requires_approval(&self) -> bool {
        match self {
            DeletablePermission::Allowed(_) => false,
            DeletablePermission::WithConstraints(c) => c
                .requires_approval
                .as_ref()
                .map_or(false, |r| r.is_required()),
        }
    }

    /// Check if soft delete is configured.
    pub fn is_soft_delete(&self) -> bool {
        match self {
            DeletablePermission::Allowed(_) => false,
            DeletablePermission::WithConstraints(c) => c.soft_delete,
        }
    }
}

/// Constraints on DELETE operations.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DeletableConstraints {
    /// Whether deletion requires human approval.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requires_approval: Option<ApprovalRequirement>,

    /// If true, use soft delete instead of hard delete.
    #[serde(default)]
    pub soft_delete: bool,
}

/// Approval requirement configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ApprovalRequirement {
    /// Simple boolean: true = use default approval group.
    Simple(bool),

    /// Detailed approval configuration.
    Detailed(ApprovalConfig),
}

impl ApprovalRequirement {
    /// Check if approval is required.
    pub fn is_required(&self) -> bool {
        match self {
            ApprovalRequirement::Simple(required) => *required,
            ApprovalRequirement::Detailed(_) => true,
        }
    }

    /// Get the approval group (if specified).
    pub fn get_group(&self) -> Option<&str> {
        match self {
            ApprovalRequirement::Simple(_) => None,
            ApprovalRequirement::Detailed(config) => Some(&config.group),
        }
    }
}

/// Detailed approval configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalConfig {
    /// Name of the approval group (must exist in groups/).
    pub group: String,

    /// Whether to notify group members when approvals are pending.
    #[serde(default = "default_notify")]
    pub notify_on_pending: bool,

    /// Custom message to display to approvers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

fn default_notify() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_role_definition() {
        let yaml = r#"
name: support_agent
description: "AI agent for customer support"

approvals:
  group: support_managers
  notify_on_pending: true

tables:
  customers:
    readable: [id, name, email]
    creatable: {}
    updatable: {}
    deletable: false
  tickets:
    readable: [id, subject, status, priority]
    creatable:
      subject:
        required: true
      priority:
        default: "low"
        restrict_to: [low, medium, high]
    updatable:
      status:
        only_when:
          - old.status: open
            new.status: [in_progress]
          - old.status: in_progress
            new.status: [resolved, open]
      priority:
        requires_approval: true
    deletable: false
"#;

        let role = RoleDefinition::from_yaml(yaml).unwrap();
        assert_eq!(role.name, "support_agent");
        assert!(role.can_access_table("customers"));
        assert!(role.can_read_column("customers", "id"));
        assert!(!role.can_create_column("customers", "id"));
        assert!(role.can_create_column("tickets", "subject"));
        assert!(role.can_update_column("tickets", "status"));
        assert!(!role.can_delete("tickets"));

        // Check constraints
        let subject_constraints = role
            .get_creatable_constraints("tickets", "subject")
            .unwrap();
        assert!(subject_constraints.required);

        let status_constraints = role.get_updatable_constraints("tickets", "status").unwrap();
        assert!(status_constraints.has_conditions());
    }

    #[test]
    fn test_readable_all_columns() {
        let yaml = r#"
name: admin
tables:
  users:
    readable: "*"
    creatable: "*"
    updatable: "*"
    deletable: true
"#;

        let role = RoleDefinition::from_yaml(yaml).unwrap();
        assert!(role.can_read_column("users", "any_column"));
        assert!(role.can_create_column("users", "any_column"));
        assert!(role.can_update_column("users", "any_column"));
        assert!(role.can_delete("users"));

        // Also check the higher-level can_create/can_update methods used by tool generator
        assert!(
            role.can_create("users"),
            "can_create should be true for creatable: '*'"
        );
        assert!(
            role.can_update("users"),
            "can_update should be true for updatable: '*'"
        );
    }

    #[test]
    fn test_deletable_with_constraints() {
        let yaml = r#"
name: manager
tables:
  orders:
    readable: "*"
    deletable:
      requires_approval: true
      soft_delete: true
"#;

        let role = RoleDefinition::from_yaml(yaml).unwrap();
        let perms = role.get_table_permissions("orders").unwrap();
        assert!(perms.deletable.is_allowed());
        assert!(perms.deletable.requires_approval());
        assert!(perms.deletable.is_soft_delete());
    }

    #[test]
    fn test_only_when_single_condition() {
        let yaml = r#"
name: warehouse_agent
tables:
  orders:
    readable: "*"
    updatable:
      status:
        only_when:
          old.status: [paid, processing]
          old.shipping_address:
            not_null: true
          new.status: shipped
"#;

        let role = RoleDefinition::from_yaml(yaml).unwrap();
        let constraints = role.get_updatable_constraints("orders", "status").unwrap();
        assert!(constraints.has_conditions());

        if let Some(OnlyWhen::Single(conditions)) = &constraints.only_when {
            assert!(conditions.contains_key("old.status"));
            assert!(conditions.contains_key("old.shipping_address"));
            assert!(conditions.contains_key("new.status"));
        } else {
            panic!("Expected single condition set");
        }
    }

    #[test]
    fn test_only_when_multiple_conditions() {
        let yaml = r#"
name: support_agent
tables:
  tickets:
    readable: "*"
    updatable:
      status:
        only_when:
          - old.status: open
            new.status: [in_progress, cancelled]
          - old.status: in_progress
            new.status: [resolved, cancelled]
"#;

        let role = RoleDefinition::from_yaml(yaml).unwrap();
        let constraints = role.get_updatable_constraints("tickets", "status").unwrap();

        if let Some(OnlyWhen::Multiple(sets)) = &constraints.only_when {
            assert_eq!(sets.len(), 2);
            assert!(sets[0].contains_key("old.status"));
            assert!(sets[0].contains_key("new.status"));
        } else {
            panic!("Expected multiple condition sets");
        }
    }

    #[test]
    fn test_increment_only_pattern() {
        let yaml = r#"
name: warehouse_agent
tables:
  inventory:
    readable: "*"
    updatable:
      quantity:
        only_when:
          new.quantity:
            greater_than: old.quantity
"#;

        let role = RoleDefinition::from_yaml(yaml).unwrap();
        let constraints = role
            .get_updatable_constraints("inventory", "quantity")
            .unwrap();
        assert!(constraints.has_conditions());

        if let Some(OnlyWhen::Single(conditions)) = &constraints.only_when {
            if let Some(ColumnCondition::Comparison(cmp)) = conditions.get("new.quantity") {
                assert!(cmp.greater_than.is_some());
                if let Some(NumberOrColumnRef::ColumnRef(col)) = &cmp.greater_than {
                    assert_eq!(col, "old.quantity");
                } else {
                    panic!("Expected column reference");
                }
            } else {
                panic!("Expected comparison condition");
            }
        }
    }

    #[test]
    fn test_append_only_pattern() {
        let yaml = r#"
name: support_agent
tables:
  tickets:
    readable: "*"
    updatable:
      notes:
        only_when:
          new.notes:
            starts_with: old.notes
"#;

        let role = RoleDefinition::from_yaml(yaml).unwrap();
        let constraints = role.get_updatable_constraints("tickets", "notes").unwrap();

        if let Some(OnlyWhen::Single(conditions)) = &constraints.only_when {
            if let Some(ColumnCondition::Comparison(cmp)) = conditions.get("new.notes") {
                assert_eq!(cmp.starts_with.as_deref(), Some("old.notes"));
            } else {
                panic!("Expected comparison condition");
            }
        }
    }
}
