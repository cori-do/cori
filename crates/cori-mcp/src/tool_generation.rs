//! Canonical tool generation helpers.
//!
//! This module provides the single source of truth for tool generation logic.
//! CLI, MCP server, and Dashboard should all use these functions to ensure
//! consistent tool generation across all entry points.
//!
//! ## Usage
//!
//! ```ignore
//! use cori_core::config::{CoriConfig, SchemaDefinition, RoleDefinition};
//! use cori_mcp::tool_generation;
//!
//! // From CoriConfig (preferred)
//! let tools = tool_generation::generate_tools_for_role(&config, "support_agent")?;
//!
//! // From SchemaDefinition directly
//! let tools = tool_generation::generate_tools(&schema_def, &role_def);
//! ```

use crate::protocol::ToolDefinition;
use crate::schema::{DatabaseSchema, from_schema_definition};
use crate::tool_generator::ToolGenerator;
use cori_core::config::{CoriConfig, RoleDefinition, SchemaDefinition};

/// Error type for tool generation.
#[derive(Debug, thiserror::Error)]
pub enum ToolGenerationError {
    #[error("Role '{0}' not found in configuration")]
    RoleNotFound(String),

    #[error("No schema found. Run 'cori db sync' to generate schema/schema.yaml")]
    NoSchema,
}

/// Generate tools for a role using CoriConfig.
///
/// This is the canonical way to generate tools. It:
/// 1. Looks up the role by name from config
/// 2. Gets the schema from config
/// 3. Converts schema to DatabaseSchema using from_schema_definition
/// 4. Generates tools using ToolGenerator
///
/// All entry points (CLI, MCP, Dashboard) should use this function
/// to ensure consistent tool generation.
pub fn generate_tools_for_role(
    config: &CoriConfig,
    role_name: &str,
) -> Result<Vec<ToolDefinition>, ToolGenerationError> {
    let role = config
        .get_role(role_name)
        .ok_or_else(|| ToolGenerationError::RoleNotFound(role_name.to_string()))?;

    let schema = config.get_schema().ok_or(ToolGenerationError::NoSchema)?;

    Ok(generate_tools(schema, role))
}

/// Generate tools from a SchemaDefinition and RoleDefinition.
///
/// This is the low-level function that does the actual tool generation.
/// Prefer `generate_tools_for_role` when you have a CoriConfig.
pub fn generate_tools(schema: &SchemaDefinition, role: &RoleDefinition) -> Vec<ToolDefinition> {
    let db_schema = from_schema_definition(schema);
    generate_tools_with_db_schema(&db_schema, role)
}

/// Generate tools from a DatabaseSchema and RoleDefinition.
///
/// Use this when you already have a converted DatabaseSchema.
pub fn generate_tools_with_db_schema(
    db_schema: &DatabaseSchema,
    role: &RoleDefinition,
) -> Vec<ToolDefinition> {
    let generator = ToolGenerator::new(role.clone(), db_schema.clone());
    generator.generate_all()
}

/// Generate tools for all roles in a config.
///
/// Returns a HashMap of role_name -> Vec<ToolDefinition>.
/// Useful for pre-generating tools at startup (HTTP mode).
pub fn generate_tools_for_all_roles(
    config: &CoriConfig,
) -> Result<std::collections::HashMap<String, Vec<ToolDefinition>>, ToolGenerationError> {
    let schema = config.get_schema().ok_or(ToolGenerationError::NoSchema)?;

    let db_schema = from_schema_definition(schema);

    let mut result = std::collections::HashMap::new();
    for (role_name, role) in config.roles() {
        let tools = generate_tools_with_db_schema(&db_schema, role);
        result.insert(role_name.clone(), tools);
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cori_core::config::schema_definition::{DatabaseEngine, DatabaseInfo};

    #[test]
    fn test_generate_tools_empty_schema() {
        let schema = SchemaDefinition {
            version: "1.0.0".to_string(),
            captured_at: "2026-01-01T00:00:00Z".to_string(),
            database: DatabaseInfo {
                engine: DatabaseEngine::Postgres,
                version: Some("16.0".to_string()),
            },
            extensions: vec![],
            enums: vec![],
            tables: vec![],
        };
        let role = RoleDefinition {
            name: "test".to_string(),
            description: None,
            approvals: None,
            tables: std::collections::HashMap::new(),
        };

        let tools = generate_tools(&schema, &role);
        assert!(tools.is_empty());
    }
}
