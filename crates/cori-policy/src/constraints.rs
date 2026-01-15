//! Constraint validation.
//!
//! This module validates constraint conditions from role definitions:
//! - `restrict_to` (allowed values for CREATE)
//! - `only_when` (conditions for UPDATE based on old.* and new.* values)
//! - Comparison conditions (greater_than, starts_with, etc.)

use crate::error::ValidationError;
use cori_core::config::role_definition::{
    ColumnCondition, ComparisonCondition, NumberOrColumnRef, OnlyWhen, TablePermissions,
};
use serde_json::Value;
use std::collections::HashMap;

/// Validates constraint conditions from role permissions.
pub struct ConstraintValidator;

impl ConstraintValidator {
    /// Create a new constraint validator.
    pub fn new() -> Self {
        Self
    }

    /// Validate CREATE operation constraints.
    ///
    /// Checks:
    /// - Column is in creatable list
    /// - restrict_to constraint is satisfied
    /// - Required fields are present (or have defaults)
    ///
    /// Note: Approval requirements are checked separately via `get_approval_requirements`.
    pub fn validate_create(
        &self,
        table: &str,
        perms: &TablePermissions,
        arguments: &Value,
        is_tenant_column: impl Fn(&str) -> bool,
    ) -> Result<(), ValidationError> {
        let args = arguments.as_object();

        // Validate each provided field
        if let Some(obj) = args {
            for (key, value) in obj {
                // Skip tenant column - it's handled separately
                if is_tenant_column(key) {
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
            }
        }

        // Check required fields
        if let Some(map) = perms.creatable.as_map() {
            for (col, constraints) in map {
                // Skip tenant column
                if is_tenant_column(col) {
                    continue;
                }

                if constraints.required {
                    let has_value = args.map(|obj| obj.contains_key(col)).unwrap_or(false);
                    let has_default = constraints.default.is_some();

                    if !has_value && !has_default {
                        return Err(ValidationError::required_field_missing(table, col));
                    }
                }
            }
        }

        Ok(())
    }

    /// Validate UPDATE operation constraints.
    ///
    /// Checks:
    /// - Column is in updatable list
    /// - only_when constraint is satisfied
    ///
    /// Note: Approval requirements are checked separately via `get_approval_requirements`.
    pub fn validate_update(
        &self,
        table: &str,
        perms: &TablePermissions,
        arguments: &Value,
        current_row: Option<&Value>,
        is_tenant_column: impl Fn(&str) -> bool,
    ) -> Result<(), ValidationError> {
        let args = arguments.as_object();

        // Validate each field being updated
        if let Some(obj) = args {
            for (key, value) in obj {
                // Skip 'id' field
                if key == "id" {
                    continue;
                }

                // Skip tenant column
                if is_tenant_column(key) {
                    continue;
                }

                // Check if column is updatable
                if !perms.updatable.contains(key) {
                    return Err(ValidationError::column_not_updatable(table, key));
                }

                // Check only_when constraint
                if let Some(constraints) = perms.updatable.get_constraints(key) {
                    if let Some(only_when) = &constraints.only_when {
                        self.validate_only_when(key, value, only_when, current_row, arguments)?;
                    }
                }
            }
        }

        Ok(())
    }

    /// Check if any fields in a CREATE operation require approval.
    ///
    /// Returns a list of column names that require approval.
    pub fn get_create_approval_fields(
        &self,
        perms: &TablePermissions,
        arguments: &Value,
    ) -> Vec<String> {
        let mut approval_fields = Vec::new();

        if let Some(obj) = arguments.as_object() {
            for key in obj.keys() {
                if let Some(constraints) = perms.creatable.get_constraints(key) {
                    if let Some(ref approval) = constraints.requires_approval {
                        if approval.is_required() {
                            approval_fields.push(key.clone());
                        }
                    }
                }
            }
        }

        approval_fields
    }

    /// Check if any fields in an UPDATE operation require approval.
    ///
    /// Returns a list of column names that require approval.
    pub fn get_update_approval_fields(
        &self,
        perms: &TablePermissions,
        arguments: &Value,
    ) -> Vec<String> {
        let mut approval_fields = Vec::new();

        if let Some(obj) = arguments.as_object() {
            for key in obj.keys() {
                if key == "id" {
                    continue;
                }
                if let Some(constraints) = perms.updatable.get_constraints(key) {
                    if let Some(ref approval) = constraints.requires_approval {
                        if approval.is_required() {
                            approval_fields.push(key.clone());
                        }
                    }
                }
            }
        }

        approval_fields
    }

    /// Check if a DELETE operation requires approval.
    pub fn delete_requires_approval(&self, perms: &TablePermissions) -> bool {
        perms.deletable.requires_approval()
    }

    /// Validate only_when constraint for an update.
    ///
    /// # Arguments
    /// - `column`: The column being validated
    /// - `new_value`: The new value for this specific column
    /// - `only_when`: The only_when constraint definition
    /// - `current_row`: The current row from the database (for old.* conditions)
    /// - `all_new_values`: All values being updated (for new.<other_column> conditions)
    pub fn validate_only_when(
        &self,
        column: &str,
        new_value: &Value,
        only_when: &OnlyWhen,
        current_row: Option<&Value>,
        all_new_values: &Value,
    ) -> Result<(), ValidationError> {
        let condition_sets = only_when.condition_sets();

        // For OR logic (multiple condition sets), at least one must match
        let mut any_matched = false;
        let mut last_error = None;

        for conditions in condition_sets {
            match self.validate_condition_set(column, new_value, conditions, current_row, all_new_values) {
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
    ///
    /// # Arguments
    /// - `column`: The column whose constraint we're validating
    /// - `new_value`: The new value for this specific column
    /// - `conditions`: Map of conditions (e.g., {"old.status": "open", "new.status": ["closed"]})
    /// - `current_row`: The current row from the database (for old.* conditions)
    /// - `all_new_values`: All values being updated (for new.<other_column> conditions)
    fn validate_condition_set(
        &self,
        column: &str,
        new_value: &Value,
        conditions: &HashMap<String, ColumnCondition>,
        current_row: Option<&Value>,
        all_new_values: &Value,
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
                    // The column being validated - use the provided new_value
                    new_value
                } else {
                    // Another column in the update - look it up in all_new_values
                    match all_new_values.get(col) {
                        Some(v) => v,
                        None => {
                            // The referenced column is not in the update.
                            // This condition cannot be satisfied if we require a specific new value.
                            // Return error to fail this condition set (OR logic will try next set).
                            return Err(ValidationError::only_when_violation(
                                column,
                                &format!(
                                    "new.{} referenced but not provided in update",
                                    col
                                ),
                            ));
                        }
                    }
                }
            } else {
                // old.* conditions require current_row - fail closed if not provided
                let Some(row) = current_row else {
                    return Err(ValidationError::only_when_violation(
                        column,
                        &format!(
                            "old.{} condition requires current row data for validation",
                            col
                        ),
                    ));
                };
                row.get(col).unwrap_or(&Value::Null)
            };

            // Validate the condition
            if !self.check_condition_with_row(value_to_check, condition, current_row) {
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
    pub fn check_condition(&self, value: &Value, condition: &ColumnCondition) -> bool {
        self.check_condition_with_row(value, condition, None)
    }

    /// Check if a value satisfies a condition, with access to current row for column references.
    pub fn check_condition_with_row(
        &self,
        value: &Value,
        condition: &ColumnCondition,
        current_row: Option<&Value>,
    ) -> bool {
        match condition {
            ColumnCondition::In(allowed) => allowed.contains(value),
            ColumnCondition::Equals(expected) => value == expected,
            ColumnCondition::Comparison(cmp) => self.check_comparison(value, cmp, current_row),
        }
    }

    /// Check a comparison condition.
    pub fn check_comparison(
        &self,
        value: &Value,
        cmp: &ComparisonCondition,
        current_row: Option<&Value>,
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

        // Handle numeric comparisons with column reference support
        if let Some(v) = value.as_f64() {
            if let Some(ref gt) = cmp.greater_than {
                let threshold = self.resolve_number_or_column_ref(gt, current_row);
                if let Some(n) = threshold {
                    if v <= n {
                        return false;
                    }
                } else if gt.is_column_ref() {
                    // Column ref but no current_row or column not found - fail closed
                    return false;
                }
            }
            if let Some(ref gte) = cmp.greater_than_or_equal {
                let threshold = self.resolve_number_or_column_ref(gte, current_row);
                if let Some(n) = threshold {
                    if v < n {
                        return false;
                    }
                } else if gte.is_column_ref() {
                    return false;
                }
            }
            if let Some(ref lt) = cmp.lower_than {
                let threshold = self.resolve_number_or_column_ref(lt, current_row);
                if let Some(n) = threshold {
                    if v >= n {
                        return false;
                    }
                } else if lt.is_column_ref() {
                    return false;
                }
            }
            if let Some(ref lte) = cmp.lower_than_or_equal {
                let threshold = self.resolve_number_or_column_ref(lte, current_row);
                if let Some(n) = threshold {
                    if v > n {
                        return false;
                    }
                } else if lte.is_column_ref() {
                    return false;
                }
            }
        }

        // Handle starts_with (for append-only pattern)
        if let Some(ref prefix_or_ref) = cmp.starts_with {
            if let Some(new_str) = value.as_str() {
                // Check if prefix_or_ref is a column reference (old.column)
                let prefix = if let Some(col) = prefix_or_ref.strip_prefix("old.") {
                    // Resolve from current_row
                    match current_row {
                        Some(row) => row.get(col).and_then(|v| v.as_str()),
                        None => {
                            // Column ref but no current_row - fail closed
                            return false;
                        }
                    }
                } else {
                    // Literal prefix
                    Some(prefix_or_ref.as_str())
                };

                if let Some(prefix_str) = prefix {
                    if !new_str.starts_with(prefix_str) {
                        return false;
                    }
                } else {
                    // Could not resolve prefix - fail closed
                    return false;
                }
            }
        }

        true
    }

    /// Resolve a NumberOrColumnRef to an f64 value.
    ///
    /// Returns None if it's a column reference that cannot be resolved.
    fn resolve_number_or_column_ref(
        &self,
        ref_or_num: &NumberOrColumnRef,
        current_row: Option<&Value>,
    ) -> Option<f64> {
        match ref_or_num {
            NumberOrColumnRef::Number(n) => Some(*n),
            NumberOrColumnRef::ColumnRef(col_ref) => {
                // Parse old.column or new.column
                let col = col_ref.strip_prefix("old.").or_else(|| col_ref.strip_prefix("new."))?;
                let row = current_row?;
                row.get(col).and_then(|v| v.as_f64())
            }
        }
    }
}

impl Default for ConstraintValidator {
    fn default() -> Self {
        Self::new()
    }
}
