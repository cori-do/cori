//! # cori-policy
//!
//! Policy validation and enforcement for Cori.
//!
//! This crate provides comprehensive validation of tool calls against:
//! - **Role definitions**: table access, column permissions, constraints
//! - **Rules definitions**: tenancy, soft delete, column validation
//!
//! All validation happens BEFORE any database operation is executed.
//!
//! ## Architecture
//!
//! The validation system is organized into perimeters:
//!
//! | Module | Responsibility |
//! |--------|----------------|
//! | [`role`] | Role-based permissions (table access, column CRUD) |
//! | [`rules`] | Rules-based validation (tenancy, patterns, allowed_values) |
//! | [`constraints`] | Constraint checking (restrict_to, only_when, required) |
//! | [`validator`] | Main `ToolValidator` that composes all validation |
//!
//! ## Usage
//!
//! ```ignore
//! use cori_policy::{ToolValidator, ValidationRequest, OperationType};
//! use cori_core::RoleDefinition;
//! use serde_json::json;
//!
//! // Load role and optionally rules
//! let role = RoleDefinition::from_file("roles/support_agent.yaml")?;
//!
//! // Create validator
//! let validator = ToolValidator::new(&role);
//!
//! // Build validation request
//! let request = ValidationRequest {
//!     operation: OperationType::Update,
//!     table: "tickets",
//!     arguments: &json!({"id": 1, "status": "resolved"}),
//!     tenant_id: "client_a",
//!     role_name: "support_agent",
//!     current_row: None,
//! };
//!
//! // Validate
//! validator.validate(&request)?;
//! ```

pub mod constraints;
pub mod error;
pub mod request;
pub mod role;
pub mod rules;
pub mod validator;

// Re-export main types at crate root
pub use error::{ValidationError, ValidationErrorKind};
pub use request::{OperationType, ValidationRequest};
pub use validator::ToolValidator;

// Re-export sub-validators for advanced use
pub use constraints::ConstraintValidator;
pub use role::RoleValidator;
pub use rules::RulesValidator;
