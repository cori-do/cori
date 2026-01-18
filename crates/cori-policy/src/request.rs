//! Validation request types.
//!
//! This module defines the request types used to pass context
//! to the validation system.

use serde_json::Value;
use std::fmt;

/// The type of operation being validated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationType {
    /// GET operation (single record by ID).
    Get,
    /// LIST operation (multiple records with filters).
    List,
    /// CREATE operation (insert new record).
    Create,
    /// UPDATE operation (modify existing record).
    Update,
    /// DELETE operation (remove record).
    Delete,
}

impl fmt::Display for OperationType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OperationType::Get => write!(f, "GET"),
            OperationType::List => write!(f, "LIST"),
            OperationType::Create => write!(f, "CREATE"),
            OperationType::Update => write!(f, "UPDATE"),
            OperationType::Delete => write!(f, "DELETE"),
        }
    }
}

/// A validation request containing all context needed for validation.
#[derive(Debug)]
pub struct ValidationRequest<'a> {
    /// The operation type.
    pub operation: OperationType,
    /// The target table name.
    pub table: &'a str,
    /// The arguments/inputs for the operation.
    pub arguments: &'a Value,
    /// The tenant ID from the execution context.
    pub tenant_id: &'a str,
    /// The role name from the execution context.
    pub role_name: &'a str,
    /// Optional: current row values for update validation (old.* conditions).
    pub current_row: Option<&'a Value>,
    /// The primary key column names for the table (supports composite keys).
    /// Returns empty vector if not provided.
    pub primary_key_columns: Option<Vec<&'a str>>,
}

impl<'a> ValidationRequest<'a> {
    /// Get the primary key column names. Returns empty vector if not provided.
    pub fn pk_columns(&self) -> Vec<&str> {
        self.primary_key_columns
            .as_ref()
            .map(|cols| cols.to_vec())
            .unwrap_or_default()
    }

    /// Check if a column is one of the primary key columns.
    pub fn is_pk_column(&self, column: &str) -> bool {
        self.pk_columns().contains(&column)
    }
}
