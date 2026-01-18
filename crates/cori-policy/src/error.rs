//! Validation error types for policy enforcement.
//!
//! This module defines the error types used when validation fails,
//! organized by the type of policy violation.

use serde_json::Value;
use std::fmt;

/// Error type for validation failures.
#[derive(Debug, Clone)]
pub struct ValidationError {
    /// The kind of validation error.
    pub kind: ValidationErrorKind,
    /// Human-readable error message.
    pub message: String,
}

impl ValidationError {
    /// Create a new validation error.
    pub fn new(kind: ValidationErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    // =========================================================================
    // ROLE VALIDATION ERRORS
    // =========================================================================

    /// Create a role not found error.
    pub fn role_not_found() -> Self {
        Self::new(
            ValidationErrorKind::RoleNotFound,
            "Role is required but was not found in the execution context",
        )
    }

    /// Create a table access denied error.
    pub fn table_access_denied(table: &str, reason: &str) -> Self {
        Self::new(
            ValidationErrorKind::TableAccessDenied,
            format!("Access denied to table '{}': {}", table, reason),
        )
    }

    /// Create a table not found in role error.
    pub fn table_not_in_role(table: &str) -> Self {
        Self::new(
            ValidationErrorKind::TableNotInRole,
            format!(
                "Table '{}' is not listed in the role's tables configuration",
                table
            ),
        )
    }

    // =========================================================================
    // COLUMN PERMISSION ERRORS
    // =========================================================================

    /// Create a column not readable error.
    pub fn column_not_readable(table: &str, column: &str) -> Self {
        Self::new(
            ValidationErrorKind::ColumnNotReadable,
            format!(
                "Column '{}' in table '{}' is not in the readable columns list",
                column, table
            ),
        )
    }

    /// Create a column not creatable error.
    pub fn column_not_creatable(table: &str, column: &str) -> Self {
        Self::new(
            ValidationErrorKind::ColumnNotCreatable,
            format!(
                "Column '{}' in table '{}' is not in the creatable columns list",
                column, table
            ),
        )
    }

    /// Create a column not updatable error.
    pub fn column_not_updatable(table: &str, column: &str) -> Self {
        Self::new(
            ValidationErrorKind::ColumnNotUpdatable,
            format!(
                "Column '{}' in table '{}' is not in the updatable columns list",
                column, table
            ),
        )
    }

    // =========================================================================
    // OPERATION PERMISSION ERRORS
    // =========================================================================

    /// Create a delete not allowed error.
    pub fn delete_not_allowed(table: &str) -> Self {
        Self::new(
            ValidationErrorKind::DeleteNotAllowed,
            format!(
                "Delete operations are not allowed on table '{}' for this role",
                table
            ),
        )
    }

    /// Create a create not allowed error.
    pub fn create_not_allowed(table: &str) -> Self {
        Self::new(
            ValidationErrorKind::CreateNotAllowed,
            format!(
                "Create operations are not allowed on table '{}' for this role (no creatable columns)",
                table
            ),
        )
    }

    /// Create an update not allowed error.
    pub fn update_not_allowed(table: &str) -> Self {
        Self::new(
            ValidationErrorKind::UpdateNotAllowed,
            format!(
                "Update operations are not allowed on table '{}' for this role (no updatable columns)",
                table
            ),
        )
    }

    // =========================================================================
    // CONSTRAINT ERRORS
    // =========================================================================

    /// Create a missing identifier error.
    pub fn missing_identifier(operation: &str) -> Self {
        Self::new(
            ValidationErrorKind::MissingIdentifier,
            format!(
                "{} operations require an 'id' field to identify the specific row to modify",
                operation
            ),
        )
    }

    /// Create a max_per_page exceeded error.
    pub fn max_per_page_exceeded(requested: u64, max: u64, table: &str) -> Self {
        Self::new(
            ValidationErrorKind::MaxPerPageExceeded,
            format!(
                "Requested limit {} exceeds max_per_page {} for table '{}'",
                requested, max, table
            ),
        )
    }

    /// Create a required field missing error.
    pub fn required_field_missing(table: &str, column: &str) -> Self {
        Self::new(
            ValidationErrorKind::RequiredFieldMissing,
            format!(
                "Required field '{}' is missing for create operation on table '{}'",
                column, table
            ),
        )
    }

    /// Create a value not allowed error (restrict_to violation).
    pub fn value_not_allowed(column: &str, value: &Value, allowed: &[Value]) -> Self {
        Self::new(
            ValidationErrorKind::ValueNotAllowed,
            format!(
                "Value {} for column '{}' is not in allowed values: {:?}",
                value, column, allowed
            ),
        )
    }

    /// Create an only_when constraint violation error.
    pub fn only_when_violation(column: &str, message: &str) -> Self {
        Self::new(
            ValidationErrorKind::OnlyWhenViolation,
            format!(
                "Update constraint violation for column '{}': {}",
                column, message
            ),
        )
    }

    // =========================================================================
    // RULES VALIDATION ERRORS
    // =========================================================================

    /// Create a tenant required error.
    pub fn tenant_required(table: &str) -> Self {
        Self::new(
            ValidationErrorKind::TenantRequired,
            format!(
                "Table '{}' requires tenant isolation but no tenant_id was provided in the context",
                table
            ),
        )
    }

    /// Create a pattern validation error.
    pub fn pattern_validation_failed(column: &str, pattern: &str) -> Self {
        Self::new(
            ValidationErrorKind::PatternValidationFailed,
            format!(
                "Value for column '{}' does not match required pattern: {}",
                column, pattern
            ),
        )
    }

    /// Create an allowed values validation error.
    pub fn allowed_values_violation(column: &str, allowed: &[Value]) -> Self {
        Self::new(
            ValidationErrorKind::AllowedValuesViolation,
            format!(
                "Value for column '{}' is not in the allowed values defined in rules: {:?}",
                column, allowed
            ),
        )
    }
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for ValidationError {}

/// Categories of validation errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValidationErrorKind {
    // =========================================================================
    // Role validation errors
    // =========================================================================
    /// Role was not found in the execution context.
    RoleNotFound,
    /// Table is not listed in the role's tables.
    TableNotInRole,
    /// General table access denied.
    TableAccessDenied,

    // =========================================================================
    // Column permission errors
    // =========================================================================
    /// Column is not readable.
    ColumnNotReadable,
    /// Column is not creatable.
    ColumnNotCreatable,
    /// Column is not updatable.
    ColumnNotUpdatable,

    // =========================================================================
    // Operation permission errors
    // =========================================================================
    /// Delete is not allowed on this table.
    DeleteNotAllowed,
    /// Create is not allowed on this table.
    CreateNotAllowed,
    /// Update is not allowed on this table.
    UpdateNotAllowed,

    // =========================================================================
    // Constraint errors
    // =========================================================================
    /// Missing required identifier for update/delete.
    MissingIdentifier,
    /// Requested limit exceeds max_per_page.
    MaxPerPageExceeded,
    /// Required field is missing for create.
    RequiredFieldMissing,
    /// Value is not in the allowed list (restrict_to).
    ValueNotAllowed,
    /// Update violates only_when constraint.
    OnlyWhenViolation,

    // =========================================================================
    // Rules validation errors
    // =========================================================================
    /// Tenant is required but missing.
    TenantRequired,
    /// Value does not match pattern from rules.
    PatternValidationFailed,
    /// Value is not in allowed_values from rules.
    AllowedValuesViolation,
}
