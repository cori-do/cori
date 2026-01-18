//! Role-based validation.
//!
//! This module validates requests against role definitions:
//! - Table access (is the table listed in the role?)
//! - Column permissions (readable, creatable, updatable)
//! - Operation permissions (get, list, create, update, delete)

use crate::error::ValidationError;
use crate::request::{OperationType, ValidationRequest};
use cori_core::config::role_definition::{RoleDefinition, TablePermissions};

/// Validates role-level permissions.
pub struct RoleValidator<'a> {
    /// The role definition to validate against.
    role: &'a RoleDefinition,
}

impl<'a> RoleValidator<'a> {
    /// Create a new role validator.
    pub fn new(role: &'a RoleDefinition) -> Self {
        Self { role }
    }

    /// Get the role definition.
    pub fn role(&self) -> &RoleDefinition {
        self.role
    }

    /// Validate that a role is present in the request.
    pub fn validate_role_present(
        &self,
        request: &ValidationRequest,
    ) -> Result<(), ValidationError> {
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
    pub fn validate_table_access(&self, table: &str) -> Result<(), ValidationError> {
        if !self.role.tables.contains_key(table) {
            return Err(ValidationError::table_not_in_role(table));
        }
        Ok(())
    }

    /// Get table permissions, returning error if not found.
    pub fn get_table_permissions(&self, table: &str) -> Result<&TablePermissions, ValidationError> {
        self.role
            .tables
            .get(table)
            .ok_or_else(|| ValidationError::table_not_in_role(table))
    }

    /// Validate GET operation permissions.
    pub fn validate_get(&self, request: &ValidationRequest) -> Result<(), ValidationError> {
        let table = request.table;
        let perms = self.get_table_permissions(table)?;

        // GET requires readable columns
        if perms.readable.is_empty() {
            return Err(ValidationError::table_access_denied(
                table,
                "no readable columns defined",
            ));
        }

        // GET requires all primary key columns to be provided
        for pk_column in request.pk_columns() {
            if request.arguments.get(pk_column).is_none() {
                return Err(ValidationError::missing_identifier("GET"));
            }
        }

        Ok(())
    }

    /// Validate LIST operation permissions.
    pub fn validate_list(&self, request: &ValidationRequest) -> Result<(), ValidationError> {
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
        if let Some(max) = perms.readable.max_per_page()
            && let Some(limit) = request.arguments.get("limit").and_then(|v| v.as_u64())
                && limit > max {
                    return Err(ValidationError::max_per_page_exceeded(limit, max, table));
                }

        Ok(())
    }

    /// Validate CREATE operation permissions (without constraint validation).
    pub fn validate_create_permissions(
        &self,
        table: &str,
    ) -> Result<&TablePermissions, ValidationError> {
        let perms = self.get_table_permissions(table)?;

        // Check if create is allowed
        if perms.creatable.is_empty() {
            return Err(ValidationError::create_not_allowed(table));
        }

        Ok(perms)
    }

    /// Validate UPDATE operation permissions (without constraint validation).
    pub fn validate_update_permissions(
        &self,
        request: &ValidationRequest,
    ) -> Result<&TablePermissions, ValidationError> {
        let table = request.table;
        let perms = self.get_table_permissions(table)?;

        // Check if update is allowed
        if perms.updatable.is_empty() {
            return Err(ValidationError::update_not_allowed(table));
        }

        // UPDATE requires all primary key columns (single row update)
        for pk_column in request.pk_columns() {
            if request.arguments.get(pk_column).is_none() {
                return Err(ValidationError::missing_identifier("UPDATE"));
            }
        }

        Ok(perms)
    }

    /// Validate DELETE operation permissions.
    pub fn validate_delete(&self, request: &ValidationRequest) -> Result<(), ValidationError> {
        let table = request.table;
        let perms = self.get_table_permissions(table)?;

        // Check if delete is allowed
        if !perms.deletable.is_allowed() {
            return Err(ValidationError::delete_not_allowed(table));
        }

        // DELETE requires all primary key columns (single row delete)
        for pk_column in request.pk_columns() {
            if request.arguments.get(pk_column).is_none() {
                return Err(ValidationError::missing_identifier("DELETE"));
            }
        }

        Ok(())
    }

    /// Dispatch validation based on operation type.
    pub fn validate(&self, request: &ValidationRequest) -> Result<(), ValidationError> {
        // 1. Validate role presence
        self.validate_role_present(request)?;

        // 2. Validate table access
        self.validate_table_access(request.table)?;

        // 3. Validate operation-specific permissions (basic checks only)
        match request.operation {
            OperationType::Get => self.validate_get(request)?,
            OperationType::List => self.validate_list(request)?,
            OperationType::Create => {
                self.validate_create_permissions(request.table)?;
            }
            OperationType::Update => {
                self.validate_update_permissions(request)?;
            }
            OperationType::Delete => self.validate_delete(request)?,
        }

        Ok(())
    }
}
