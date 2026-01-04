//! Schema types for tool generation.
//!
//! This module provides types representing database schema information
//! used for generating MCP tools.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Schema information for a database.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DatabaseSchema {
    /// Tables in the database.
    pub tables: HashMap<String, TableSchema>,
}

impl DatabaseSchema {
    /// Create a new empty database schema.
    pub fn new() -> Self {
        Self {
            tables: HashMap::new(),
        }
    }

    /// Add a table to the schema.
    pub fn add_table(&mut self, table: TableSchema) {
        self.tables.insert(table.name.clone(), table);
    }

    /// Get a table by name.
    pub fn get_table(&self, name: &str) -> Option<&TableSchema> {
        self.tables.get(name)
    }
}

/// Schema information for a database table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableSchema {
    /// Table name.
    pub name: String,

    /// Schema name (e.g., "public").
    #[serde(default)]
    pub schema: Option<String>,

    /// Columns in the table.
    pub columns: Vec<ColumnSchema>,

    /// Primary key columns.
    #[serde(default)]
    pub primary_key: Vec<String>,

    /// Foreign key relationships.
    #[serde(default)]
    pub foreign_keys: Vec<ForeignKey>,
}

impl TableSchema {
    /// Create a new table schema.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            schema: None,
            columns: Vec::new(),
            primary_key: Vec::new(),
            foreign_keys: Vec::new(),
        }
    }

    /// Add a column to the table.
    pub fn add_column(&mut self, column: ColumnSchema) {
        self.columns.push(column);
    }

    /// Get a column by name.
    pub fn get_column(&self, name: &str) -> Option<&ColumnSchema> {
        self.columns.iter().find(|c| c.name == name)
    }

    /// Get the primary key column(s).
    pub fn get_primary_key_columns(&self) -> Vec<&ColumnSchema> {
        self.columns
            .iter()
            .filter(|c| self.primary_key.contains(&c.name))
            .collect()
    }

    /// Get the ID column type for JSON schema (defaults to "integer").
    pub fn get_id_type(&self) -> &str {
        if let Some(pk) = self.primary_key.first() {
            if let Some(col) = self.get_column(pk) {
                return col.json_schema_type();
            }
        }
        "integer"
    }
}

/// Schema information for a database column.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColumnSchema {
    /// Column name.
    pub name: String,

    /// SQL data type.
    pub data_type: String,

    /// Whether the column is nullable.
    pub nullable: bool,

    /// Whether this is a primary key column.
    #[serde(default)]
    pub is_primary_key: bool,

    /// Default value (if any).
    #[serde(default)]
    pub default: Option<String>,

    /// Column description/comment.
    #[serde(default)]
    pub description: Option<String>,
}

impl ColumnSchema {
    /// Create a new column schema.
    pub fn new(name: impl Into<String>, data_type: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            data_type: data_type.into(),
            nullable: true,
            is_primary_key: false,
            default: None,
            description: None,
        }
    }

    /// Convert SQL type to JSON Schema type.
    pub fn json_schema_type(&self) -> &str {
        let dt = self.data_type.to_lowercase();

        if dt.contains("int") || dt.contains("serial") {
            "integer"
        } else if dt.contains("numeric")
            || dt.contains("decimal")
            || dt.contains("float")
            || dt.contains("double")
            || dt.contains("real")
        {
            "number"
        } else if dt.contains("bool") {
            "boolean"
        } else if dt.contains("json") {
            "object"
        } else if dt.contains("array") {
            "array"
        } else {
            // text, varchar, char, uuid, timestamp, date, etc.
            "string"
        }
    }

    /// Get a format hint for JSON Schema (if applicable).
    pub fn json_schema_format(&self) -> Option<&str> {
        let dt = self.data_type.to_lowercase();

        if dt.contains("uuid") {
            Some("uuid")
        } else if dt.contains("date") && !dt.contains("timestamp") {
            Some("date")
        } else if dt.contains("timestamp") || dt.contains("timestamptz") {
            Some("date-time")
        } else if dt.contains("time") && !dt.contains("timestamp") {
            Some("time")
        } else if dt == "text" && self.name.contains("email") {
            Some("email")
        } else if dt == "text" && self.name.contains("uri") {
            Some("uri")
        } else {
            None
        }
    }
}

/// Foreign key relationship.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForeignKey {
    /// Constraint name.
    pub name: String,

    /// Column mappings (local -> foreign).
    pub columns: Vec<ForeignKeyColumn>,
}

/// A single column mapping in a foreign key.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForeignKeyColumn {
    /// Local column name.
    pub column: String,

    /// Referenced table schema.
    #[serde(default)]
    pub foreign_schema: Option<String>,

    /// Referenced table name.
    pub foreign_table: String,

    /// Referenced column name.
    pub foreign_column: String,
}

/// Parse database schema from introspection JSON.
pub fn parse_schema_from_json(json: &serde_json::Value) -> Result<DatabaseSchema, SchemaParseError> {
    let mut schema = DatabaseSchema::new();

    let tables = json["tables"]
        .as_array()
        .ok_or_else(|| SchemaParseError::MissingField("tables".to_string()))?;

    for table_json in tables {
        let table = parse_table_from_json(table_json)?;
        schema.add_table(table);
    }

    Ok(schema)
}

fn parse_table_from_json(json: &serde_json::Value) -> Result<TableSchema, SchemaParseError> {
    let name = json["name"]
        .as_str()
        .ok_or_else(|| SchemaParseError::MissingField("name".to_string()))?
        .to_string();

    let schema_name = json["schema"].as_str().map(String::from);

    let mut table = TableSchema::new(&name);
    table.schema = schema_name;

    // Parse columns
    if let Some(columns) = json["columns"].as_array() {
        for col_json in columns {
            let col_name = col_json["name"]
                .as_str()
                .ok_or_else(|| SchemaParseError::MissingField("column.name".to_string()))?;
            let data_type = col_json["data_type"]
                .as_str()
                .ok_or_else(|| SchemaParseError::MissingField("column.data_type".to_string()))?;

            let mut column = ColumnSchema::new(col_name, data_type);
            column.nullable = col_json["nullable"].as_bool().unwrap_or(true);
            column.default = col_json["default"].as_str().map(String::from);

            table.add_column(column);
        }
    }

    // Parse primary key
    if let Some(pk) = json["primary_key"].as_array() {
        for pk_col in pk {
            if let Some(col_name) = pk_col.as_str() {
                table.primary_key.push(col_name.to_string());

                // Mark column as primary key
                if let Some(col) = table.columns.iter_mut().find(|c| c.name == col_name) {
                    col.is_primary_key = true;
                }
            }
        }
    }

    // Parse foreign keys
    if let Some(fks) = json["foreign_keys"].as_array() {
        for fk_json in fks {
            let fk_name = fk_json["name"].as_str().unwrap_or("").to_string();
            let mut fk = ForeignKey {
                name: fk_name,
                columns: Vec::new(),
            };

            if let Some(mappings) = fk_json["mappings"].as_array() {
                for mapping in mappings {
                    let column = mapping["column"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string();
                    let foreign_schema = mapping["references"]["schema"].as_str().map(String::from);
                    let foreign_table = mapping["references"]["table"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string();
                    let foreign_column = mapping["references"]["column"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string();

                    fk.columns.push(ForeignKeyColumn {
                        column,
                        foreign_schema,
                        foreign_table,
                        foreign_column,
                    });
                }
            }

            if !fk.columns.is_empty() {
                table.foreign_keys.push(fk);
            }
        }
    }

    Ok(table)
}

/// Errors that can occur when parsing schema.
#[derive(Debug, thiserror::Error)]
pub enum SchemaParseError {
    #[error("Missing required field: {0}")]
    MissingField(String),

    #[error("Invalid value for field {field}: {message}")]
    InvalidValue { field: String, message: String },
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_parse_schema_from_json() {
        let json = json!({
            "tables": [
                {
                    "name": "users",
                    "schema": "public",
                    "columns": [
                        {"name": "id", "data_type": "integer", "nullable": false},
                        {"name": "name", "data_type": "text", "nullable": false},
                        {"name": "email", "data_type": "text", "nullable": true}
                    ],
                    "primary_key": ["id"],
                    "foreign_keys": []
                }
            ]
        });

        let schema = parse_schema_from_json(&json).unwrap();
        let users = schema.get_table("users").unwrap();

        assert_eq!(users.name, "users");
        assert_eq!(users.columns.len(), 3);
        assert_eq!(users.primary_key, vec!["id"]);
        assert!(users.get_column("id").unwrap().is_primary_key);
    }

    #[test]
    fn test_json_schema_type_mapping() {
        let int_col = ColumnSchema::new("id", "integer");
        assert_eq!(int_col.json_schema_type(), "integer");

        let text_col = ColumnSchema::new("name", "text");
        assert_eq!(text_col.json_schema_type(), "string");

        let numeric_col = ColumnSchema::new("price", "numeric(10,2)");
        assert_eq!(numeric_col.json_schema_type(), "number");

        let bool_col = ColumnSchema::new("active", "boolean");
        assert_eq!(bool_col.json_schema_type(), "boolean");

        let json_col = ColumnSchema::new("data", "jsonb");
        assert_eq!(json_col.json_schema_type(), "object");
    }
}
