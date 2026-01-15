//! Main validator that composes role, rules, and constraint validation.
//!
//! The `ToolValidator` is the primary entry point for validating tool execution
//! requests. It orchestrates validation across multiple perimeters:
//!
//! 1. **Role validation** - Table access and column permissions
//! 2. **Rules validation** - Tenancy and column value patterns
//! 3. **Constraint validation** - restrict_to, only_when, required fields

use crate::constraints::ConstraintValidator;
use crate::error::ValidationError;
use crate::request::{OperationType, ValidationRequest};
use crate::role::RoleValidator;
use crate::rules::RulesValidator;
use cori_core::config::role_definition::RoleDefinition;
use cori_core::config::rules_definition::RulesDefinition;

/// Validates tool execution requests against role and rules definitions.
///
/// This is the main entry point for policy validation. It composes:
/// - [`RoleValidator`] for table/column permissions
/// - [`RulesValidator`] for tenancy and column patterns
/// - [`ConstraintValidator`] for restrict_to, only_when, etc.
pub struct ToolValidator<'a> {
    /// The role validator.
    role_validator: RoleValidator<'a>,
    /// The rules validator (optional).
    rules_validator: Option<RulesValidator<'a>>,
    /// The constraint validator.
    constraint_validator: ConstraintValidator,
}

impl<'a> ToolValidator<'a> {
    /// Create a new validator with a role definition.
    pub fn new(role: &'a RoleDefinition) -> Self {
        Self {
            role_validator: RoleValidator::new(role),
            rules_validator: None,
            constraint_validator: ConstraintValidator::new(),
        }
    }

    /// Add rules definition for additional validation.
    pub fn with_rules(mut self, rules: &'a RulesDefinition) -> Self {
        self.rules_validator = Some(RulesValidator::new(rules));
        self
    }

    /// Validate a tool execution request.
    ///
    /// Returns `Ok(())` if validation passes, or `Err(ValidationError)` if it fails.
    ///
    /// Validation is performed in this order:
    /// 1. Role presence validation
    /// 2. Table access validation
    /// 3. Tenant configuration validation (if rules provided)
    /// 4. Operation-specific validation (GET, LIST, CREATE, UPDATE, DELETE)
    /// 5. Constraint validation (restrict_to, only_when, required)
    /// 6. Column value validation against rules (patterns, allowed_values)
    pub fn validate(&self, request: &ValidationRequest) -> Result<(), ValidationError> {
        // 1. Validate role-level permissions (role presence, table access, basic operation checks)
        self.role_validator.validate(request)?;

        // 2. Validate tenant configuration (from rules, if available)
        if let Some(rules_validator) = &self.rules_validator {
            rules_validator.validate_tenant(request)?;
        }

        // 3. Validate operation-specific constraints
        match request.operation {
            OperationType::Get | OperationType::List => {
                // These are fully validated by role_validator.validate()
            }
            OperationType::Create => {
                self.validate_create(request)?;
            }
            OperationType::Update => {
                self.validate_update(request)?;
            }
            OperationType::Delete => {
                self.validate_delete(request)?;
            }
        }

        Ok(())
    }

    /// Validate CREATE operation with constraints and rules.
    fn validate_create(&self, request: &ValidationRequest) -> Result<(), ValidationError> {
        let table = request.table;
        let perms = self.role_validator.get_table_permissions(table)?;

        // Validate constraints (creatable columns, restrict_to, required fields)
        self.constraint_validator.validate_create(
            table,
            perms,
            request.arguments,
            |col| self.is_tenant_column(table, col),
        )?;

        // Validate column values against rules (patterns, allowed_values)
        self.validate_column_values(request)?;

        Ok(())
    }

    /// Validate UPDATE operation with constraints and rules.
    fn validate_update(&self, request: &ValidationRequest) -> Result<(), ValidationError> {
        let table = request.table;
        let perms = self.role_validator.get_table_permissions(table)?;

        // Validate constraints (updatable columns, only_when)
        self.constraint_validator.validate_update(
            table,
            perms,
            request.arguments,
            request.current_row,
            |col| self.is_tenant_column(table, col),
        )?;

        // Validate column values against rules (patterns, allowed_values)
        self.validate_column_values(request)?;

        Ok(())
    }

    /// Validate DELETE operation.
    fn validate_delete(&self, request: &ValidationRequest) -> Result<(), ValidationError> {
        // Role-level delete permission is already checked by role_validator.validate()
        // This method exists for symmetry and future extensions
        let _table = request.table;
        Ok(())
    }

    /// Validate column values against rules definition.
    fn validate_column_values(&self, request: &ValidationRequest) -> Result<(), ValidationError> {
        let Some(rules_validator) = &self.rules_validator else {
            return Ok(());
        };

        if let Some(obj) = request.arguments.as_object() {
            for (key, value) in obj {
                // Skip id field
                if key == "id" {
                    continue;
                }

                // Skip tenant column
                if rules_validator.is_tenant_column(request.table, key) {
                    continue;
                }

                rules_validator.validate_column_value(request.table, key, value)?;
            }
        }

        Ok(())
    }

    /// Check if a column is the tenant column for a table.
    fn is_tenant_column(&self, table: &str, column: &str) -> bool {
        self.rules_validator
            .as_ref()
            .map(|rv| rv.is_tenant_column(table, column))
            .unwrap_or(false)
    }

    /// Check if an operation requires approval.
    ///
    /// Returns `Some(fields)` if approval is required (with the list of fields needing approval),
    /// or `None` if no approval is needed.
    ///
    /// This should be called AFTER `validate()` passes to determine if approval workflow
    /// should be triggered.
    pub fn requires_approval(&self, request: &ValidationRequest) -> Option<Vec<String>> {
        match request.operation {
            OperationType::Get | OperationType::List => None,
            OperationType::Create => self.create_requires_approval(request),
            OperationType::Update => self.update_requires_approval(request),
            OperationType::Delete => self.delete_requires_approval(request),
        }
    }

    /// Check if a CREATE operation requires approval.
    fn create_requires_approval(&self, request: &ValidationRequest) -> Option<Vec<String>> {
        let perms = self.role_validator.get_table_permissions(request.table).ok()?;
        let fields = self
            .constraint_validator
            .get_create_approval_fields(perms, request.arguments);
        if fields.is_empty() {
            None
        } else {
            Some(fields)
        }
    }

    /// Check if an UPDATE operation requires approval.
    fn update_requires_approval(&self, request: &ValidationRequest) -> Option<Vec<String>> {
        let perms = self.role_validator.get_table_permissions(request.table).ok()?;
        let fields = self
            .constraint_validator
            .get_update_approval_fields(perms, request.arguments);
        if fields.is_empty() {
            None
        } else {
            Some(fields)
        }
    }

    /// Check if a DELETE operation requires approval.
    fn delete_requires_approval(&self, request: &ValidationRequest) -> Option<Vec<String>> {
        let perms = self.role_validator.get_table_permissions(request.table).ok()?;
        if self.constraint_validator.delete_requires_approval(perms) {
            Some(vec![]) // Empty vec indicates table-level approval, not column-specific
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cori_core::config::role_definition::{
        ColumnCondition, CreatableColumnConstraints, CreatableColumns, DeletablePermission,
        OnlyWhen, ReadableConfig, TablePermissions, UpdatableColumnConstraints, UpdatableColumns,
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
                readable: ReadableConfig::List(vec!["id".to_string(), "status".to_string()]),
                creatable: CreatableColumns::Map(HashMap::from([
                    (
                        "status".to_string(),
                        CreatableColumnConstraints {
                            required: true,
                            restrict_to: Some(vec![json!("pending"), json!("confirmed")]),
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
        assert_eq!(
            result.unwrap_err().kind,
            crate::error::ValidationErrorKind::RoleNotFound
        );
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
        assert_eq!(
            result.unwrap_err().kind,
            crate::error::ValidationErrorKind::TableNotInRole
        );
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
            crate::error::ValidationErrorKind::MissingIdentifier
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
            crate::error::ValidationErrorKind::CreateNotAllowed
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
            crate::error::ValidationErrorKind::RequiredFieldMissing
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
            crate::error::ValidationErrorKind::ValueNotAllowed
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
            crate::error::ValidationErrorKind::MissingIdentifier
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
            crate::error::ValidationErrorKind::UpdateNotAllowed
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
            crate::error::ValidationErrorKind::OnlyWhenViolation
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
            crate::error::ValidationErrorKind::MissingIdentifier
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
            crate::error::ValidationErrorKind::DeleteNotAllowed
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
