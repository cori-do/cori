//! Error types for the RLS crate.

use thiserror::Error;

/// Errors that can occur during RLS injection.
#[derive(Debug, Error)]
pub enum RlsError {
    /// SQL parsing failed.
    #[error("failed to parse SQL: {0}")]
    ParseError(String),

    /// Query references a table that cannot be safely scoped.
    #[error("cannot scope query: {reason}")]
    UnscopableQuery { reason: String },

    /// Query contains a cross-tenant join.
    #[error("cross-tenant join detected between {table1} and {table2}")]
    CrossTenantJoin { table1: String, table2: String },

    /// DDL statement is not allowed.
    #[error("DDL statement not allowed: {statement}")]
    DdlNotAllowed { statement: String },

    /// Operation is blocked for this role.
    #[error("operation {operation} is blocked")]
    OperationBlocked { operation: String },

    /// Table access is not allowed.
    #[error("access to table {table} is not allowed")]
    TableAccessDenied { table: String },

    /// Column access is not allowed.
    #[error("access to column {table}.{column} is not allowed")]
    ColumnAccessDenied { table: String, column: String },

    /// Query contains subquery accessing other tenants.
    #[error("subquery may access other tenants")]
    SubqueryTenantLeak,

    /// Tenant value required but not provided.
    #[error("tenant value required but not provided")]
    MissingTenantValue,

    /// Internal error.
    #[error("internal error: {0}")]
    Internal(#[from] anyhow::Error),
}

