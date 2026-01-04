//! Role-driven dynamic tool generation.
//!
//! This module generates MCP tools dynamically based on role permissions
//! and database schema. Tools are generated at connection time based on
//! the connecting token's role claims.
//!
//! ## Tool Generation Rules
//!
//! | Tool Pattern | Generated When | Description |
//! |--------------|----------------|-------------|
//! | `get<Entity>(id)` | Table has readable columns | Fetch single record |
//! | `list<Entity>(filters)` | Table has readable columns | Query multiple records |
//! | `create<Entity>(data)` | Role has INSERT + editable columns | Create new record |
//! | `update<Entity>(id, data)` | Role has UPDATE + editable columns | Modify record |
//! | `delete<Entity>(id)` | Role has DELETE permission | Remove record |

use crate::protocol::{ToolAnnotations, ToolDefinition};
use cori_core::{ColumnConstraints, CustomAction, ReadableColumns, RoleConfig};
use crate::schema::{DatabaseSchema, TableSchema};
use serde_json::{json, Value};

/// Generator for creating MCP tools from role permissions and schema.
pub struct ToolGenerator {
    /// The role configuration.
    role: RoleConfig,
    /// The database schema.
    schema: DatabaseSchema,
    /// Maximum rows limit from role config.
    max_rows: Option<u64>,
}

impl ToolGenerator {
    /// Create a new tool generator.
    pub fn new(role: RoleConfig, schema: DatabaseSchema) -> Self {
        let max_rows = role.max_rows_per_query;
        Self {
            role,
            schema,
            max_rows,
        }
    }

    /// Generate all tools for this role.
    pub fn generate_all(&self) -> Vec<ToolDefinition> {
        let mut tools = Vec::new();

        // Generate CRUD tools for each accessible table
        for (table_name, _perms) in &self.role.tables {
            if self.role.blocked_tables.contains(table_name) {
                continue;
            }

            if let Some(table_schema) = self.schema.get_table(table_name) {
                tools.extend(self.generate_table_tools(table_name, table_schema));
            } else {
                // Generate tools even without schema (with basic input schemas)
                tools.extend(self.generate_table_tools_no_schema(table_name));
            }
        }

        // Generate custom action tools
        for action in &self.role.custom_actions {
            tools.push(self.generate_custom_action_tool(action));
        }

        tools
    }

    /// Generate CRUD tools for a single table.
    fn generate_table_tools(
        &self,
        table_name: &str,
        table_schema: &TableSchema,
    ) -> Vec<ToolDefinition> {
        let mut tools = Vec::new();
        // Singularize the table name before converting to PascalCase for entity name
        let singular_name = singularize(table_name);
        let entity_name = pascal_case(&singular_name);

        // get{Entity} - if readable
        if self.role.can_read(table_name) {
            tools.push(self.generate_get_tool(table_name, &entity_name, table_schema));
            tools.push(self.generate_list_tool(table_name, &entity_name, table_schema));
        }

        // create{Entity} - if can create
        if self.role.can_create(table_name) {
            tools.push(self.generate_create_tool(table_name, &entity_name, table_schema));
        }

        // update{Entity} - if can update
        if self.role.can_update(table_name) {
            tools.push(self.generate_update_tool(table_name, &entity_name, table_schema));
        }

        // delete{Entity} - if can delete
        if self.role.can_delete(table_name) {
            tools.push(self.generate_delete_tool(table_name, &entity_name, table_schema));
        }

        tools
    }

    /// Generate tools without full schema (fallback).
    fn generate_table_tools_no_schema(&self, table_name: &str) -> Vec<ToolDefinition> {
        let mut tools = Vec::new();
        // Singularize the table name before converting to PascalCase
        let singular_name = singularize(table_name);
        let entity_name = pascal_case(&singular_name);

        // get{Entity}
        if self.role.can_read(table_name) {
            tools.push(ToolDefinition {
                name: format!("get{}", entity_name),
                description: Some(format!("Retrieve a {} by ID", singular_name)),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "id": {
                            "type": "integer",
                            "description": format!("{} ID", entity_name)
                        }
                    },
                    "required": ["id"]
                }),
                annotations: Some(ToolAnnotations {
                    requires_approval: Some(false),
                    dry_run_supported: Some(false),
                    read_only: Some(true),
                    ..Default::default()
                }),
            });

            tools.push(ToolDefinition {
                name: format!("list{}", pluralize(&entity_name)),
                description: Some(format!("List {} with optional filters", table_name)),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "limit": {
                            "type": "integer",
                            "default": 50,
                            "maximum": self.max_rows.unwrap_or(1000),
                            "description": "Maximum number of results"
                        },
                        "offset": {
                            "type": "integer",
                            "default": 0,
                            "description": "Offset for pagination"
                        }
                    }
                }),
                annotations: Some(ToolAnnotations {
                    requires_approval: Some(false),
                    dry_run_supported: Some(false),
                    read_only: Some(true),
                    ..Default::default()
                }),
            });
        }

        tools
    }

    /// Generate a get{Entity} tool.
    fn generate_get_tool(
        &self,
        table_name: &str,
        entity_name: &str,
        table_schema: &TableSchema,
    ) -> ToolDefinition {
        let id_type = table_schema.get_id_type();
        let id_description = if let Some(pk) = table_schema.primary_key.first() {
            pk.clone()
        } else {
            "id".to_string()
        };

        ToolDefinition {
            name: format!("get{}", entity_name),
            description: Some(format!("Retrieve a {} by ID", table_name)),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": id_type,
                        "description": format!("{} {}", entity_name, id_description)
                    }
                },
                "required": ["id"]
            }),
            annotations: Some(ToolAnnotations {
                requires_approval: Some(false),
                dry_run_supported: Some(false),
                read_only: Some(true),
                ..Default::default()
            }),
        }
    }

    /// Generate a list{Entities} tool.
    fn generate_list_tool(
        &self,
        table_name: &str,
        entity_name: &str,
        table_schema: &TableSchema,
    ) -> ToolDefinition {
        let mut properties = json!({
            "limit": {
                "type": "integer",
                "default": 50,
                "maximum": self.max_rows.unwrap_or(1000),
                "description": "Maximum number of results"
            },
            "offset": {
                "type": "integer",
                "default": 0,
                "description": "Offset for pagination"
            }
        });

        // Add filter properties for readable columns
        let filter_props = self.generate_filter_properties(table_name, table_schema);
        if let Value::Object(ref mut props) = properties {
            if let Value::Object(filters) = filter_props {
                props.extend(filters);
            }
        }

        ToolDefinition {
            name: format!("list{}", pluralize(entity_name)),
            description: Some(format!(
                "List {} with optional filters",
                pluralize(table_name)
            )),
            input_schema: json!({
                "type": "object",
                "properties": properties
            }),
            annotations: Some(ToolAnnotations {
                requires_approval: Some(false),
                dry_run_supported: Some(false),
                read_only: Some(true),
                ..Default::default()
            }),
        }
    }

    /// Generate a create{Entity} tool.
    fn generate_create_tool(
        &self,
        table_name: &str,
        entity_name: &str,
        table_schema: &TableSchema,
    ) -> ToolDefinition {
        let (properties, required) = self.generate_input_schema(table_name, table_schema, true);
        let requires_approval = self.role.table_requires_approval(table_name);
        let approval_fields = self.role.get_approval_columns(table_name);

        ToolDefinition {
            name: format!("create{}", entity_name),
            description: Some(format!("Create a new {}", table_name)),
            input_schema: json!({
                "type": "object",
                "properties": properties,
                "required": required
            }),
            annotations: Some(ToolAnnotations {
                requires_approval: Some(requires_approval),
                dry_run_supported: Some(true),
                read_only: Some(false),
                approval_fields: if approval_fields.is_empty() {
                    None
                } else {
                    Some(approval_fields.iter().map(|s| s.to_string()).collect())
                },
            }),
        }
    }

    /// Generate an update{Entity} tool.
    fn generate_update_tool(
        &self,
        table_name: &str,
        entity_name: &str,
        table_schema: &TableSchema,
    ) -> ToolDefinition {
        let (mut properties, _) = self.generate_input_schema(table_name, table_schema, false);
        let requires_approval = self.role.table_requires_approval(table_name);
        let approval_fields = self.role.get_approval_columns(table_name);
        let id_type = table_schema.get_id_type();

        // Add id to properties
        if let Value::Object(ref mut props) = properties {
            props.insert(
                "id".to_string(),
                json!({
                    "type": id_type,
                    "description": format!("{} ID", entity_name)
                }),
            );
        }

        ToolDefinition {
            name: format!("update{}", entity_name),
            description: Some(format!("Update an existing {}", table_name)),
            input_schema: json!({
                "type": "object",
                "properties": properties,
                "required": ["id"]
            }),
            annotations: Some(ToolAnnotations {
                requires_approval: Some(requires_approval),
                dry_run_supported: Some(true),
                read_only: Some(false),
                approval_fields: if approval_fields.is_empty() {
                    None
                } else {
                    Some(approval_fields.iter().map(|s| s.to_string()).collect())
                },
            }),
        }
    }

    /// Generate a delete{Entity} tool.
    fn generate_delete_tool(
        &self,
        table_name: &str,
        entity_name: &str,
        table_schema: &TableSchema,
    ) -> ToolDefinition {
        let id_type = table_schema.get_id_type();

        ToolDefinition {
            name: format!("delete{}", entity_name),
            description: Some(format!("Delete a {}", table_name)),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": id_type,
                        "description": format!("{} ID", entity_name)
                    }
                },
                "required": ["id"]
            }),
            annotations: Some(ToolAnnotations {
                requires_approval: Some(true),
                dry_run_supported: Some(true),
                read_only: Some(false),
                ..Default::default()
            }),
        }
    }

    /// Generate a custom action tool.
    fn generate_custom_action_tool(
        &self,
        action: &CustomAction,
    ) -> ToolDefinition {
        let mut properties = serde_json::Map::new();
        let mut required = Vec::new();

        for (name, input) in &action.inputs {
            let mut prop = json!({
                "type": &input.param_type
            });

            if let Some(desc) = &input.description {
                prop["description"] = json!(desc);
            }

            if let Some(enum_values) = &input.enum_values {
                prop["enum"] = json!(enum_values);
            }

            properties.insert(name.clone(), prop);

            if input.required {
                required.push(name.clone());
            }
        }

        ToolDefinition {
            name: action.name.clone(),
            description: action.description.clone(),
            input_schema: json!({
                "type": "object",
                "properties": properties,
                "required": required
            }),
            annotations: Some(ToolAnnotations {
                requires_approval: Some(action.requires_approval),
                dry_run_supported: Some(true),
                read_only: Some(false),
                ..Default::default()
            }),
        }
    }

    /// Generate input schema properties for editable columns.
    fn generate_input_schema(
        &self,
        table_name: &str,
        table_schema: &TableSchema,
        for_create: bool,
    ) -> (Value, Vec<String>) {
        let mut properties = serde_json::Map::new();
        let mut required = Vec::new();

        if let Some(perms) = self.role.tables.get(table_name) {
            for (col_name, constraints) in perms.editable.iter() {
                if let Some(col_schema) = table_schema.get_column(col_name) {
                    let prop = self.column_to_json_schema(col_schema, Some(constraints));
                    properties.insert(col_name.to_string(), prop);

                    // For create, non-nullable columns without defaults are required
                    if for_create && !col_schema.nullable && col_schema.default.is_none() {
                        required.push(col_name.to_string());
                    }
                } else {
                    // Column not in schema, use basic string type
                    properties.insert(
                        col_name.to_string(),
                        self.constraints_to_json_schema(constraints, "string"),
                    );
                }
            }
        }

        (Value::Object(properties), required)
    }

    /// Generate filter properties for readable columns.
    fn generate_filter_properties(&self, table_name: &str, table_schema: &TableSchema) -> Value {
        let mut properties = serde_json::Map::new();

        if let Some(perms) = self.role.tables.get(table_name) {
            // Get list of readable columns
            let readable_cols: Vec<&str> = match &perms.readable {
                ReadableColumns::All(_) => {
                    table_schema.columns.iter().map(|c| c.name.as_str()).collect()
                }
                ReadableColumns::List(cols) => {
                    cols.iter().map(|s: &String| s.as_str()).collect()
                }
            };

            for col_name in readable_cols {
                if let Some(col_schema) = table_schema.get_column(col_name) {
                    // Skip complex types for filtering
                    let col_type = col_schema.json_schema_type();
                    if col_type == "object" || col_type == "array" {
                        continue;
                    }

                    let mut prop = json!({
                        "type": col_type,
                        "description": format!("Filter by {}", col_name)
                    });

                    if let Some(format) = col_schema.json_schema_format() {
                        prop["format"] = json!(format);
                    }

                    properties.insert(col_name.to_string(), prop);
                }
            }
        }

        Value::Object(properties)
    }

    /// Convert a column schema to JSON Schema with constraints.
    fn column_to_json_schema(
        &self,
        column: &crate::schema::ColumnSchema,
        constraints: Option<&ColumnConstraints>,
    ) -> Value {
        let base_type = column.json_schema_type();
        let mut schema = self.constraints_to_json_schema(
            constraints.unwrap_or(&ColumnConstraints::default()),
            base_type,
        );

        // Add format if applicable
        if let Some(format) = column.json_schema_format() {
            schema["format"] = json!(format);
        }

        // Add description
        if let Some(desc) = &column.description {
            schema["description"] = json!(desc);
        }

        schema
    }

    /// Convert column constraints to JSON Schema.
    fn constraints_to_json_schema(&self, constraints: &ColumnConstraints, base_type: &str) -> Value {
        let mut schema = json!({
            "type": base_type
        });

        // Add allowed_values as enum
        if let Some(values) = &constraints.allowed_values {
            schema["enum"] = json!(values);
        }

        // Add pattern
        if let Some(pattern) = &constraints.pattern {
            schema["pattern"] = json!(pattern);
        }

        // Add min/max for numeric types
        if base_type == "integer" || base_type == "number" {
            if let Some(min) = constraints.min {
                schema["minimum"] = json!(min);
            }
            if let Some(max) = constraints.max {
                schema["maximum"] = json!(max);
            }
        }

        schema
    }
}

/// Convert a snake_case string to PascalCase.
fn pascal_case(s: &str) -> String {
    s.split('_')
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                None => String::new(),
                Some(first) => first.to_uppercase().chain(chars).collect(),
            }
        })
        .collect()
}

/// Simple singularization (converts plural to singular).
fn singularize(s: &str) -> String {
    // Common irregular plurals
    let irregulars = [
        ("people", "person"),
        ("children", "child"),
        ("men", "man"),
        ("women", "woman"),
        ("mice", "mouse"),
        ("geese", "goose"),
        ("teeth", "tooth"),
        ("feet", "foot"),
    ];
    
    for (plural, singular) in irregulars {
        if s == plural {
            return singular.to_string();
        }
    }
    
    // Words ending in 'ies' -> 'y' (e.g., categories -> category)
    if s.ends_with("ies") && s.len() > 3 {
        return format!("{}y", &s[..s.len() - 3]);
    }
    
    // Words ending in 'es' that should keep the 'e' (e.g., boxes -> box)
    // Be careful about words like "status" that end in "us" not "es"
    if s.ends_with("xes") || s.ends_with("ches") || s.ends_with("shes") || s.ends_with("sses") {
        return s[..s.len() - 2].to_string();
    }
    
    // Words ending in 'ves' -> 'f' or 'fe' (e.g., leaves -> leaf)
    if s.ends_with("ves") {
        return format!("{}f", &s[..s.len() - 3]);
    }
    
    // Words ending in 's' but not 'ss' or 'us' or 'is' (e.g., users -> user)
    if s.ends_with('s') && !s.ends_with("ss") && !s.ends_with("us") && !s.ends_with("is") {
        return s[..s.len() - 1].to_string();
    }
    
    // Return as-is if no rule matched
    s.to_string()
}

/// Simple pluralization.
fn pluralize(s: &str) -> String {
    if s.ends_with('s') || s.ends_with('x') || s.ends_with("ch") || s.ends_with("sh") {
        format!("{}es", s)
    } else if s.ends_with('y') && !s.ends_with("ey") && !s.ends_with("ay") && !s.ends_with("oy") {
        format!("{}ies", &s[..s.len() - 1])
    } else {
        format!("{}s", s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cori_core::{EditableColumns, ReadableColumns, TablePermissions};
    use crate::schema::ColumnSchema;
    use std::collections::HashMap;

    #[test]
    fn test_generate_get_tool() {
        let mut role = RoleConfig {
            name: "test".to_string(),
            description: None,
            tables: HashMap::new(),
            blocked_tables: Vec::new(),
            max_rows_per_query: Some(100),
            max_affected_rows: None,
            blocked_operations: Vec::new(),
            custom_actions: Vec::new(),
            include_actions: Vec::new(),
        };

        role.tables.insert(
            "users".to_string(),
            TablePermissions {
                readable: ReadableColumns::List(vec![
                    "id".to_string(),
                    "name".to_string(),
                ]),
                editable: EditableColumns::Map(HashMap::new()),
                operations: None,
                tenant_column: None,
            },
        );

        let mut schema = DatabaseSchema::new();
        let mut table = crate::schema::TableSchema::new("users");
        table.add_column(ColumnSchema::new("id", "integer"));
        table.add_column(ColumnSchema::new("name", "text"));
        table.primary_key.push("id".to_string());
        schema.add_table(table);

        let generator = ToolGenerator::new(role, schema);
        let tools = generator.generate_all();

        assert!(tools.iter().any(|t| t.name == "getUser"));
        assert!(tools.iter().any(|t| t.name == "listUsers"));
        // No create/update/delete since no editable columns
        assert!(!tools.iter().any(|t| t.name == "createUser"));
    }

    #[test]
    fn test_generate_update_tool_with_constraints() {
        let mut role = RoleConfig {
            name: "test".to_string(),
            description: None,
            tables: HashMap::new(),
            blocked_tables: Vec::new(),
            max_rows_per_query: Some(100),
            max_affected_rows: None,
            blocked_operations: Vec::new(),
            custom_actions: Vec::new(),
            include_actions: Vec::new(),
        };

        let mut editable = HashMap::new();
        editable.insert(
            "status".to_string(),
            ColumnConstraints {
                allowed_values: Some(vec!["open".to_string(), "closed".to_string()]),
                requires_approval: false,
                ..Default::default()
            },
        );
        editable.insert(
            "priority".to_string(),
            ColumnConstraints {
                requires_approval: true,
                ..Default::default()
            },
        );

        role.tables.insert(
            "tickets".to_string(),
            TablePermissions {
                readable: ReadableColumns::List(vec![
                    "id".to_string(),
                    "status".to_string(),
                ]),
                editable: EditableColumns::Map(editable),
                operations: None,
                tenant_column: None,
            },
        );

        let mut schema = DatabaseSchema::new();
        let mut table = crate::schema::TableSchema::new("tickets");
        table.add_column(ColumnSchema::new("id", "integer"));
        table.add_column(ColumnSchema::new("status", "text"));
        table.add_column(ColumnSchema::new("priority", "text"));
        table.primary_key.push("id".to_string());
        schema.add_table(table);

        let generator = ToolGenerator::new(role, schema);
        let tools = generator.generate_all();

        let update_tool = tools.iter().find(|t| t.name == "updateTicket").unwrap();

        // Check that status has enum constraint
        let status_schema = &update_tool.input_schema["properties"]["status"];
        assert!(status_schema["enum"].is_array());

        // Check requires approval
        let annotations = update_tool.annotations.as_ref().unwrap();
        assert_eq!(annotations.requires_approval, Some(true));
    }
}
