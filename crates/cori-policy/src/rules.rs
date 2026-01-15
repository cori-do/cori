//! Rules-based validation.
//!
//! This module validates requests against rules definitions:
//! - Tenant configuration (direct, inherited, global)
//! - Column patterns (regex validation)
//! - Allowed values (enumeration validation)

use crate::error::ValidationError;
use crate::request::ValidationRequest;
use cori_core::config::rules_definition::{RulesDefinition, TenantConfig};
use serde_json::Value;

/// Validates rules-level constraints.
pub struct RulesValidator<'a> {
    /// The rules definition to validate against.
    rules: &'a RulesDefinition,
}

impl<'a> RulesValidator<'a> {
    /// Create a new rules validator.
    pub fn new(rules: &'a RulesDefinition) -> Self {
        Self { rules }
    }

    /// Get the rules definition.
    pub fn rules(&self) -> &RulesDefinition {
        self.rules
    }

    /// Validate tenant configuration from rules.
    pub fn validate_tenant(&self, request: &ValidationRequest) -> Result<(), ValidationError> {
        let Some(table_rules) = self.rules.get_table_rules(request.table) else {
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

    /// Check if a column is the tenant column for a table.
    pub fn is_tenant_column(&self, table: &str, column: &str) -> bool {
        let Some(table_rules) = self.rules.get_table_rules(table) else {
            return false;
        };

        match &table_rules.tenant {
            Some(TenantConfig::Direct(col)) => col == column,
            _ => false,
        }
    }

    /// Validate a column value against rules definition (patterns and allowed_values).
    pub fn validate_column_value(
        &self,
        table: &str,
        column: &str,
        value: &Value,
    ) -> Result<(), ValidationError> {
        let Some(table_rules) = self.rules.get_table_rules(table) else {
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
}
