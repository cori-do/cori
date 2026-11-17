// SPDX-License-Identifier: Apache-2.0
//! The files that define a store: its tables (`schema_definition`), the rules every role obeys (`rules_definition`:
//! tenancy, soft delete, column rules) and what each role may do (`role_definition`).

pub mod role_definition;
pub mod rules_definition;
pub mod schema_definition;

pub use role_definition::{
    AllColumns, ApprovalConfig, ApprovalRequirement, ColumnCondition, ColumnList,
    ComparisonCondition, CreatableColumnConstraints, CreatableColumns, DeletableConstraints,
    DeletablePermission, ForeignKeyConstraint, ReadableConfig, ReadableConfigFull, RoleDefinition,
    RowScope, TablePermissions, UpdatableColumnConstraints, UpdatableColumns, fk_verify_key,
};
pub use rules_definition::{
    ColumnRules, InheritedTenant, RulesDefinition, SoftDeleteConfig, SoftDeleteValue, TableRules,
    TenantConfig,
};
pub use schema_definition::{
    ColumnSchema, ColumnType, DatabaseEngine, DatabaseInfo, EnumDefinition, ForeignKey,
    ForeignKeyAction, ForeignKeyReference, Index, SchemaDefinition, TableSchema,
};

/// A definition file that cannot be read or parsed.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("YAML parse error: {0}")]
    Yaml(#[from] serde_yaml::Error),

    #[error("Configuration error: {0}")]
    Config(String),
}
