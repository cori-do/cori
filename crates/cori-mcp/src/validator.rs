//! Request validation for MCP tool execution.
//!
//! This module provides comprehensive validation of tool calls against:
//! - **Role definitions**: table access, column permissions, constraints
//! - **Rules definitions**: tenancy, soft delete, column validation
//!
//! All validation happens BEFORE any database operation is executed.

use cori_core::config::role_definition::{
    ColumnCondition, OnlyWhen, RoleDefinition,
    TablePermissions,
};
use cori_core::config::rules_definition::{RulesDefinition, TenantConfig};
use serde_json::Value;
use std::fmt;

// =============================================================================
// VALIDATION ERROR TYPES
// =============================================================================

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
    // Role validation errors
    /// Role was not found in the execution context.
    RoleNotFound,
    /// Table is not listed in the role's tables.
    TableNotInRole,
    /// General table access denied.
    TableAccessDenied,

    // Column permission errors
    /// Column is not readable.
    ColumnNotReadable,
    /// Column is not creatable.
    ColumnNotCreatable,
    /// Column is not updatable.
    ColumnNotUpdatable,

    // Operation permission errors
    /// Delete is not allowed on this table.
    DeleteNotAllowed,
    /// Create is not allowed on this table.
    CreateNotAllowed,
    /// Update is not allowed on this table.
    UpdateNotAllowed,

    // Constraint errors
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

    // Rules validation errors
    /// Tenant is required but missing.
    TenantRequired,
    /// Value does not match pattern from rules.
    PatternValidationFailed,
    /// Value is not in allowed_values from rules.
    AllowedValuesViolation,
}

// =============================================================================
// VALIDATION REQUEST
// =============================================================================

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
}

// =============================================================================
// TOOL VALIDATOR
// =============================================================================

/// Validates tool execution requests against role and rules definitions.
pub struct ToolValidator<'a> {
    /// The role definition to validate against.
    role: &'a RoleDefinition,
    /// The rules definition (optional, for tenancy and column validation).
    rules: Option<&'a RulesDefinition>,
}

impl<'a> ToolValidator<'a> {
    /// Create a new validator with a role definition.
    pub fn new(role: &'a RoleDefinition) -> Self {
        Self { role, rules: None }
    }

    /// Add rules definition for additional validation.
    pub fn with_rules(mut self, rules: &'a RulesDefinition) -> Self {
        self.rules = Some(rules);
        self
    }

    /// Validate a tool execution request.
    ///
    /// Returns `Ok(())` if validation passes, or `Err(ValidationError)` if it fails.
    pub fn validate(&self, request: &ValidationRequest) -> Result<(), ValidationError> {
        // 1. Validate role presence
        self.validate_role_present(request)?;

        // 2. Validate table access
        self.validate_table_access(request.table)?;

        // 3. Validate tenant configuration (from rules)
        self.validate_tenant(request)?;

        // 4. Validate operation-specific permissions and constraints
        match request.operation {
            OperationType::Get => self.validate_get(request)?,
            OperationType::List => self.validate_list(request)?,
            OperationType::Create => self.validate_create(request)?,
            OperationType::Update => self.validate_update(request)?,
            OperationType::Delete => self.validate_delete(request)?,
        }

        Ok(())
    }

    /// Validate that a role is present.
    fn validate_role_present(&self, request: &ValidationRequest) -> Result<(), ValidationError> {
        if request.role_name.is_empty() {
            return Err(ValidationError::role_not_found());
        }
        // Additional check: verify the role name matches
        if self.role.name != request.role_name && request.role_name != "unknown" {
            // Allow "unknown" for backwards compatibility, but warn
            tracing::warn!(
                "Role name mismatch: context has '{}' but validator has '{}'",
                request.role_name,
                self.role.name
            );
        }
        Ok(())
    }

    /// Validate table access (table existence in role).
    fn validate_table_access(&self, table: &str) -> Result<(), ValidationError> {
        // Check if table is in role's tables configuration
        if !self.role.tables.contains_key(table) {
            return Err(ValidationError::table_not_in_role(table));
        }

        Ok(())
    }

    /// Validate tenant configuration from rules.
    fn validate_tenant(&self, request: &ValidationRequest) -> Result<(), ValidationError> {
        let Some(rules) = self.rules else {
            // No rules definition, skip tenant validation
            return Ok(());
        };

        let Some(table_rules) = rules.get_table_rules(request.table) else {
            // Table not in rules, skip tenant validation
            return Ok(());
        };

        // Check if table is global (no tenant scoping)
        if table_rules.global.unwrap_or(false) {
            return Ok(());
        }

        // Check if table requires tenant
        if table_rules.tenant.is_some() {
            // Tenant is required
            if request.tenant_id.is_empty() || request.tenant_id == "unknown" {
                return Err(ValidationError::tenant_required(request.table));
            }
        }

        Ok(())
    }

    /// Validate GET operation.
    fn validate_get(&self, request: &ValidationRequest) -> Result<(), ValidationError> {
        let table = request.table;
        let perms = self.get_table_permissions(table)?;

        // GET requires readable columns
        if perms.readable.is_empty() {
            return Err(ValidationError::table_access_denied(
                table,
                "no readable columns defined",
            ));
        }

        // GET requires an ID
        if request.arguments.get("id").is_none() {
            return Err(ValidationError::missing_identifier("GET"));
        }

        Ok(())
    }

    /// Validate LIST operation.
    fn validate_list(&self, request: &ValidationRequest) -> Result<(), ValidationError> {
        let table = request.table;
        let perms = self.get_table_permissions(table)?;

        // LIST requires readable columns
        if perms.readable.is_empty() {
            return Err(ValidationError::table_access_denied(
                table,
                "no readable columns defined",
            ));
        }

        // Check max_per_page limit
        if let Some(max) = perms.readable.max_per_page() {
            if let Some(limit) = request.arguments.get("limit").and_then(|v| v.as_u64()) {
                if limit > max {
                    return Err(ValidationError::max_per_page_exceeded(limit, max, table));
                }
            }
        }

        Ok(())
    }

    /// Validate CREATE operation.
    fn validate_create(&self, request: &ValidationRequest) -> Result<(), ValidationError> {
        let table = request.table;
        let perms = self.get_table_permissions(table)?;

        // Check if create is allowed
        if perms.creatable.is_empty() {
            return Err(ValidationError::create_not_allowed(table));
        }

        let args = request.arguments.as_object();

        // Validate each provided field
        if let Some(obj) = args {
            for (key, value) in obj {
                // Skip tenant column - it's handled separately
                if self.is_tenant_column(table, key) {
                    continue;
                }

                // Check if column is creatable
                if !perms.creatable.contains(key) {
                    return Err(ValidationError::column_not_creatable(table, key));
                }

                // Check restrict_to constraint
                if let Some(constraints) = perms.creatable.get_constraints(key) {
                    if let Some(allowed) = &constraints.restrict_to {
                        if !allowed.contains(value) {
                            return Err(ValidationError::value_not_allowed(key, value, allowed));
                        }
                    }
                }

                // Validate against rules (pattern, allowed_values)
                self.validate_column_value(table, key, value)?;
            }
        }

        // Check required fields
        if let Some(map) = perms.creatable.as_map() {
            for (col, constraints) in map {
                // Skip tenant column
                if self.is_tenant_column(table, col) {
                    continue;
                }

                if constraints.required {
                    let has_value = args
                        .map(|obj| obj.contains_key(col))
                        .unwrap_or(false);
                    let has_default = constraints.default.is_some();

                    if !has_value && !has_default {
                        return Err(ValidationError::required_field_missing(table, col));
                    }
                }
            }
        }

        Ok(())
    }

    /// Validate UPDATE operation.
    fn validate_update(&self, request: &ValidationRequest) -> Result<(), ValidationError> {
        let table = request.table;
        let perms = self.get_table_permissions(table)?;

        // Check if update is allowed
        if perms.updatable.is_empty() {
            return Err(ValidationError::update_not_allowed(table));
        }

        // UPDATE requires an ID (single row update)
        if request.arguments.get("id").is_none() {
            return Err(ValidationError::missing_identifier("UPDATE"));
        }

        let args = request.arguments.as_object();

        // Validate each field being updated
        if let Some(obj) = args {
            for (key, value) in obj {
                // Skip 'id' field
                if key == "id" {
                    continue;
                }

                // Skip tenant column
                if self.is_tenant_column(table, key) {
                    continue;
                }

                // Check if column is updatable
                if !perms.updatable.contains(key) {
                    return Err(ValidationError::column_not_updatable(table, key));
                }

                // Check only_when constraint
                if let Some(constraints) = perms.updatable.get_constraints(key) {
                    if let Some(only_when) = &constraints.only_when {
                        self.validate_only_when(key, value, only_when, request.current_row)?;
                    }
                }

                // Validate against rules (pattern, allowed_values)
                self.validate_column_value(table, key, value)?;
            }
        }

        Ok(())
    }

    /// Validate DELETE operation.
    fn validate_delete(&self, request: &ValidationRequest) -> Result<(), ValidationError> {
        let table = request.table;
        let perms = self.get_table_permissions(table)?;

        // Check if delete is allowed
        if !perms.deletable.is_allowed() {
            return Err(ValidationError::delete_not_allowed(table));
        }

        // DELETE requires an ID (single row delete)
        if request.arguments.get("id").is_none() {
            return Err(ValidationError::missing_identifier("DELETE"));
        }

        Ok(())
    }

    /// Get table permissions, returning error if not found.
    fn get_table_permissions(&self, table: &str) -> Result<&TablePermissions, ValidationError> {
        self.role
            .tables
            .get(table)
            .ok_or_else(|| ValidationError::table_not_in_role(table))
    }

    /// Check if a column is the tenant column for a table.
    fn is_tenant_column(&self, table: &str, column: &str) -> bool {
        let Some(rules) = self.rules else {
            return false;
        };
        let Some(table_rules) = rules.get_table_rules(table) else {
            return false;
        };

        match &table_rules.tenant {
            Some(TenantConfig::Direct(col)) => col == column,
            _ => false,
        }
    }

    /// Validate a column value against rules definition.
    fn validate_column_value(
        &self,
        table: &str,
        column: &str,
        value: &Value,
    ) -> Result<(), ValidationError> {
        let Some(rules) = self.rules else {
            return Ok(());
        };
        let Some(table_rules) = rules.get_table_rules(table) else {
            return Ok(());
        };
        let Some(column_rules) = table_rules.columns.get(column) else {
            return Ok(());
        };

        // Check pattern validation
        if let Some(pattern) = &column_rules.pattern {
            if let Some(s) = value.as_str() {
                match regex::Regex::new(pattern) {
                    Ok(re) => {
                        if !re.is_match(s) {
                            return Err(ValidationError::pattern_validation_failed(column, pattern));
                        }
                    }
                    Err(_) => {
                        tracing::warn!("Invalid regex pattern for column {}: {}", column, pattern);
                    }
                }
            }
        }

        // Check allowed_values validation
        if let Some(allowed) = &column_rules.allowed_values {
            if !allowed.contains(value) {
                return Err(ValidationError::allowed_values_violation(column, allowed));
            }
        }

        Ok(())
    }

    /// Validate only_when constraint for an update.
    fn validate_only_when(
        &self,
        column: &str,
        new_value: &Value,
        only_when: &OnlyWhen,
        current_row: Option<&Value>,
    ) -> Result<(), ValidationError> {
        let condition_sets = only_when.condition_sets();

        // For OR logic (multiple condition sets), at least one must match
        let mut any_matched = false;
        let mut last_error = None;

        for conditions in condition_sets {
            match self.validate_condition_set(column, new_value, conditions, current_row) {
                Ok(()) => {
                    any_matched = true;
                    break;
                }
                Err(e) => {
                    last_error = Some(e);
                }
            }
        }

        if any_matched {
            Ok(())
        } else {
            Err(last_error.unwrap_or_else(|| {
                ValidationError::only_when_violation(column, "no condition matched")
            }))
        }
    }

    /// Validate a single condition set (AND logic within the set).
    fn validate_condition_set(
        &self,
        column: &str,
        new_value: &Value,
        conditions: &std::collections::HashMap<String, ColumnCondition>,
        current_row: Option<&Value>,
    ) -> Result<(), ValidationError> {
        for (key, condition) in conditions {
            // Parse key to determine if it's old.col or new.col
            let (prefix, col) = if let Some(col) = key.strip_prefix("old.") {
                ("old", col)
            } else if let Some(col) = key.strip_prefix("new.") {
                ("new", col)
            } else {
                // Legacy format without prefix, assume new.*
                ("new", key.as_str())
            };

            let value_to_check = if prefix == "new" {
                if col == column {
                    new_value
                } else {
                    // For other new.* conditions, we'd need access to all new values
                    // For now, skip (this is a limitation)
                    continue;
                }
            } else {
                // old.* conditions require current_row
                let Some(row) = current_row else {
                    // Skip old.* validation if we don't have current row
                    // The executor should provide this for full validation
                    continue;
                };
                row.get(col).unwrap_or(&Value::Null)
            };

            // Validate the condition
            if !self.check_condition(value_to_check, condition) {
                return Err(ValidationError::only_when_violation(
                    column,
                    &format!(
                        "{}.{} condition not satisfied (value: {})",
                        prefix, col, value_to_check
                    ),
                ));
            }
        }

        Ok(())
    }

    /// Check if a value satisfies a condition.
    fn check_condition(&self, value: &Value, condition: &ColumnCondition) -> bool {
        match condition {
            ColumnCondition::In(allowed) => allowed.contains(value),
            ColumnCondition::Equals(expected) => value == expected,
            ColumnCondition::Comparison(cmp) => self.check_comparison(value, cmp),
        }
    }

    /// Check a comparison condition.
    fn check_comparison(
        &self,
        value: &Value,
        cmp: &cori_core::config::role_definition::ComparisonCondition,
    ) -> bool {
        // Handle equals
        if let Some(expected) = &cmp.equals {
            if value != expected {
                return false;
            }
        }

        // Handle not_equals
        if let Some(not_expected) = &cmp.not_equals {
            if value == not_expected {
                return false;
            }
        }

        // Handle not_null
        if let Some(true) = cmp.not_null {
            if value.is_null() {
                return false;
            }
        }

        // Handle is_null
        if let Some(true) = cmp.is_null {
            if !value.is_null() {
                return false;
            }
        }

        // Handle in_values
        if let Some(allowed) = &cmp.in_values {
            if !allowed.contains(value) {
                return false;
            }
        }

        // Handle not_in
        if let Some(disallowed) = &cmp.not_in {
            if disallowed.contains(value) {
                return false;
            }
        }

        // Handle numeric comparisons
        if let Some(v) = value.as_f64() {
            if let Some(ref gt) = cmp.greater_than {
                if let Some(n) = gt.as_number() {
                    if v <= n {
                        return false;
                    }
                }
            }
            if let Some(ref gte) = cmp.greater_than_or_equal {
                if let Some(n) = gte.as_number() {
                    if v < n {
                        return false;
                    }
                }
            }
            if let Some(ref lt) = cmp.lower_than {
                if let Some(n) = lt.as_number() {
                    if v >= n {
                        return false;
                    }
                }
            }
            if let Some(ref lte) = cmp.lower_than_or_equal {
                if let Some(n) = lte.as_number() {
                    if v > n {
                        return false;
                    }
                }
            }
        }

        // Handle starts_with (for append-only pattern)
        if let Some(ref prefix) = cmp.starts_with {
            if let Some(s) = value.as_str() {
                // If prefix refers to old.*, we'd need the old value
                // For now, just check if it's a string that starts with something
                if !s.starts_with(prefix) {
                    return false;
                }
            }
        }

        true
    }
}

// =============================================================================
// TESTS
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use cori_core::config::role_definition::{
        CreatableColumnConstraints, CreatableColumns, DeletablePermission, ReadableConfig,
        TablePermissions, UpdatableColumnConstraints, UpdatableColumns,
    };
    use serde_json::json;
    use std::collections::HashMap;

    fn create_test_role() -> RoleDefinition {
        let mut tables = HashMap::new();
        tables.insert(
            "customers".to_string(),
            TablePermissions {
                readable: ReadableConfig::List(vec![
                    "id".to_string(),
                    "name".to_string(),
                    "email".to_string(),
                ]),
                creatable: CreatableColumns::default(),
                updatable: UpdatableColumns::default(),
                deletable: DeletablePermission::default(),
            },
        );
        tables.insert(
            "orders".to_string(),
            TablePermissions {
                readable: ReadableConfig::List(vec![
                    "id".to_string(),
                    "status".to_string(),
                ]),
                creatable: CreatableColumns::Map(HashMap::from([
                    (
                        "status".to_string(),
                        CreatableColumnConstraints {
                            required: true,
                            restrict_to: Some(vec![
                                json!("pending"),
                                json!("confirmed"),
                            ]),
                            ..Default::default()
                        },
                    ),
                    (
                        "notes".to_string(),
                        CreatableColumnConstraints::default(),
                    ),
                ])),
                updatable: UpdatableColumns::Map(HashMap::from([(
                    "status".to_string(),
                    UpdatableColumnConstraints {
                        only_when: Some(OnlyWhen::Single(HashMap::from([(
                            "new.status".to_string(),
                            ColumnCondition::In(vec![
                                json!("pending"),
                                json!("confirmed"),
                                json!("shipped"),
                            ]),
                        )]))),
                        ..Default::default()
                    },
                )])),
                deletable: DeletablePermission::Allowed(true),
            },
        );

        RoleDefinition {
            name: "test_role".to_string(),
            description: None,
            approvals: None,
            tables,
        }
    }

    #[test]
    fn test_role_present_validation() {
        let role = create_test_role();
        let validator = ToolValidator::new(&role);

        let request = ValidationRequest {
            operation: OperationType::Get,
            table: "customers",
            arguments: &json!({"id": 1}),
            tenant_id: "tenant1",
            role_name: "",
            current_row: None,
        };

        let result = validator.validate(&request);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().kind, ValidationErrorKind::RoleNotFound);
    }

    #[test]
    fn test_table_not_in_role_validation() {
        let role = create_test_role();
        let validator = ToolValidator::new(&role);

        let request = ValidationRequest {
            operation: OperationType::Get,
            table: "unknown_table",
            arguments: &json!({"id": 1}),
            tenant_id: "tenant1",
            role_name: "test_role",
            current_row: None,
        };

        let result = validator.validate(&request);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().kind, ValidationErrorKind::TableNotInRole);
    }

    #[test]
    fn test_get_validation_success() {
        let role = create_test_role();
        let validator = ToolValidator::new(&role);

        let request = ValidationRequest {
            operation: OperationType::Get,
            table: "customers",
            arguments: &json!({"id": 1}),
            tenant_id: "tenant1",
            role_name: "test_role",
            current_row: None,
        };

        let result = validator.validate(&request);
        assert!(result.is_ok());
    }

    #[test]
    fn test_get_missing_id() {
        let role = create_test_role();
        let validator = ToolValidator::new(&role);

        let request = ValidationRequest {
            operation: OperationType::Get,
            table: "customers",
            arguments: &json!({}),
            tenant_id: "tenant1",
            role_name: "test_role",
            current_row: None,
        };

        let result = validator.validate(&request);
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().kind,
            ValidationErrorKind::MissingIdentifier
        );
    }

    #[test]
    fn test_create_not_allowed() {
        let role = create_test_role();
        let validator = ToolValidator::new(&role);

        let request = ValidationRequest {
            operation: OperationType::Create,
            table: "customers",
            arguments: &json!({"name": "test"}),
            tenant_id: "tenant1",
            role_name: "test_role",
            current_row: None,
        };

        let result = validator.validate(&request);
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().kind,
            ValidationErrorKind::CreateNotAllowed
        );
    }

    #[test]
    fn test_create_required_field_missing() {
        let role = create_test_role();
        let validator = ToolValidator::new(&role);

        let request = ValidationRequest {
            operation: OperationType::Create,
            table: "orders",
            arguments: &json!({"notes": "test"}),
            tenant_id: "tenant1",
            role_name: "test_role",
            current_row: None,
        };

        let result = validator.validate(&request);
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().kind,
            ValidationErrorKind::RequiredFieldMissing
        );
    }

    #[test]
    fn test_create_restrict_to_violation() {
        let role = create_test_role();
        let validator = ToolValidator::new(&role);

        let request = ValidationRequest {
            operation: OperationType::Create,
            table: "orders",
            arguments: &json!({"status": "invalid_status"}),
            tenant_id: "tenant1",
            role_name: "test_role",
            current_row: None,
        };

        let result = validator.validate(&request);
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().kind,
            ValidationErrorKind::ValueNotAllowed
        );
    }

    #[test]
    fn test_update_missing_id() {
        let role = create_test_role();
        let validator = ToolValidator::new(&role);

        let request = ValidationRequest {
            operation: OperationType::Update,
            table: "orders",
            arguments: &json!({"status": "shipped"}),
            tenant_id: "tenant1",
            role_name: "test_role",
            current_row: None,
        };

        let result = validator.validate(&request);
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().kind,
            ValidationErrorKind::MissingIdentifier
        );
    }

    #[test]
    fn test_update_column_not_updatable() {
        let role = create_test_role();
        let validator = ToolValidator::new(&role);

        let request = ValidationRequest {
            operation: OperationType::Update,
            table: "customers",
            arguments: &json!({"id": 1, "name": "new name"}),
            tenant_id: "tenant1",
            role_name: "test_role",
            current_row: None,
        };

        let result = validator.validate(&request);
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().kind,
            ValidationErrorKind::UpdateNotAllowed
        );
    }

    #[test]
    fn test_update_only_when_violation() {
        let role = create_test_role();
        let validator = ToolValidator::new(&role);

        let request = ValidationRequest {
            operation: OperationType::Update,
            table: "orders",
            arguments: &json!({"id": 1, "status": "invalid_status"}),
            tenant_id: "tenant1",
            role_name: "test_role",
            current_row: None,
        };

        let result = validator.validate(&request);
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().kind,
            ValidationErrorKind::OnlyWhenViolation
        );
    }

    #[test]
    fn test_update_only_when_success() {
        let role = create_test_role();
        let validator = ToolValidator::new(&role);

        let request = ValidationRequest {
            operation: OperationType::Update,
            table: "orders",
            arguments: &json!({"id": 1, "status": "shipped"}),
            tenant_id: "tenant1",
            role_name: "test_role",
            current_row: None,
        };

        let result = validator.validate(&request);
        assert!(result.is_ok());
    }

    #[test]
    fn test_delete_missing_id() {
        let role = create_test_role();
        let validator = ToolValidator::new(&role);

        let request = ValidationRequest {
            operation: OperationType::Delete,
            table: "orders",
            arguments: &json!({}),
            tenant_id: "tenant1",
            role_name: "test_role",
            current_row: None,
        };

        let result = validator.validate(&request);
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().kind,
            ValidationErrorKind::MissingIdentifier
        );
    }

    #[test]
    fn test_delete_not_allowed() {
        let role = create_test_role();
        let validator = ToolValidator::new(&role);

        let request = ValidationRequest {
            operation: OperationType::Delete,
            table: "customers",
            arguments: &json!({"id": 1}),
            tenant_id: "tenant1",
            role_name: "test_role",
            current_row: None,
        };

        let result = validator.validate(&request);
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().kind,
            ValidationErrorKind::DeleteNotAllowed
        );
    }

    #[test]
    fn test_delete_success() {
        let role = create_test_role();
        let validator = ToolValidator::new(&role);

        let request = ValidationRequest {
            operation: OperationType::Delete,
            table: "orders",
            arguments: &json!({"id": 1}),
            tenant_id: "tenant1",
            role_name: "test_role",
            current_row: None,
        };

        let result = validator.validate(&request);
        assert!(result.is_ok());
    }
}
