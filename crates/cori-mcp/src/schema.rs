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

    /// Get all primary key column names.
    pub fn get_primary_key_names(&self) -> Vec<&str> {
        self.primary_key.iter().map(|s| s.as_str()).collect()
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
pub fn parse_schema_from_json(
    json: &serde_json::Value,
) -> Result<DatabaseSchema, SchemaParseError> {
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

    // Parse columns following SchemaDefinition.schema.json format
    if let Some(columns) = json["columns"].as_array() {
        for col_json in columns {
            let col_name = col_json["name"]
                .as_str()
                .ok_or_else(|| SchemaParseError::MissingField("column.name".to_string()))?;

            // Support both "data_type" (snapshot.json) and "type"/"native_type" (schema.yaml)
            let data_type = col_json["data_type"]
                .as_str()
                .or_else(|| col_json["native_type"].as_str())
                .or_else(|| col_json["type"].as_str())
                .ok_or_else(|| {
                    SchemaParseError::MissingField("column.data_type or column.type".to_string())
                })?;

            let mut column = ColumnSchema::new(col_name, data_type);
            column.nullable = col_json["nullable"].as_bool().unwrap_or(true);

            // Default value can be string, number, boolean, or null
            column.default = if col_json["default"].is_null() {
                None
            } else if let Some(s) = col_json["default"].as_str() {
                Some(s.to_string())
            } else {
                // For numbers/booleans, convert to string
                Some(col_json["default"].to_string())
            };

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

    // Parse foreign keys following SchemaDefinition.schema.json format
    if let Some(fks) = json["foreign_keys"].as_array() {
        for fk_json in fks {
            let fk_name = fk_json["name"].as_str().unwrap_or("").to_string();
            let mut fk = ForeignKey {
                name: fk_name,
                columns: Vec::new(),
            };

            // SchemaDefinition format: columns[] and references.columns[]
            if let Some(columns) = fk_json["columns"].as_array() {
                let references = &fk_json["references"];
                let foreign_schema = references["schema"].as_str().map(String::from);
                let foreign_table = references["table"].as_str().unwrap_or_default().to_string();
                let ref_columns = references["columns"].as_array();

                for (i, col_val) in columns.iter().enumerate() {
                    let column = col_val.as_str().unwrap_or_default().to_string();
                    let foreign_column = ref_columns
                        .and_then(|rc| rc.get(i))
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string();

                    fk.columns.push(ForeignKeyColumn {
                        column,
                        foreign_schema: foreign_schema.clone(),
                        foreign_table: foreign_table.clone(),
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

/// Convert a SchemaDefinition from cori-core to DatabaseSchema for tool generation.
///
/// This is the canonical conversion function used by CLI, dashboard, and MCP server
/// to ensure consistent tool generation across all entry points.
pub fn from_schema_definition(schema: &cori_core::config::SchemaDefinition) -> DatabaseSchema {
    let mut db_schema = DatabaseSchema::new();

    for table in &schema.tables {
        let columns: Vec<ColumnSchema> = table
            .columns
            .iter()
            .map(|c| {
                // Use native_type for data_type, fallback to column_type's Display
                let data_type = c
                    .native_type
                    .clone()
                    .unwrap_or_else(|| format!("{:?}", c.column_type).to_lowercase());

                // Convert default value to string if present
                let default = c.default.as_ref().map(|v| {
                    if let Some(s) = v.as_str() {
                        s.to_string()
                    } else {
                        v.to_string()
                    }
                });

                ColumnSchema {
                    name: c.name.clone(),
                    data_type,
                    nullable: c.nullable,
                    is_primary_key: table.primary_key.contains(&c.name),
                    default,
                    description: None,
                }
            })
            .collect();

        let foreign_keys: Vec<ForeignKey> = table
            .foreign_keys
            .iter()
            .map(|fk| {
                let fk_columns: Vec<ForeignKeyColumn> = fk
                    .columns
                    .iter()
                    .zip(fk.references.columns.iter())
                    .map(|(col, ref_col)| ForeignKeyColumn {
                        column: col.clone(),
                        foreign_schema: fk.references.schema.clone(),
                        foreign_table: fk.references.table.clone(),
                        foreign_column: ref_col.clone(),
                    })
                    .collect();

                ForeignKey {
                    name: fk.name.clone().unwrap_or_default(),
                    columns: fk_columns,
                }
            })
            .collect();

        db_schema.add_table(TableSchema {
            name: table.name.clone(),
            schema: Some(table.schema.clone()),
            columns,
            primary_key: table.primary_key.clone(),
            foreign_keys,
        });
    }

    db_schema
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_parse_schema_from_json() {
        // Test basic SchemaDefinition.schema.json format
        let json = json!({
            "version": "1.0.0",
            "captured_at": "2025-01-08T10:30:00Z",
            "database": {
                "engine": "postgres",
                "version": "16.1"
            },
            "tables": [
                {
                    "name": "users",
                    "schema": "public",
                    "columns": [
                        {"name": "id", "type": "integer", "native_type": "integer", "nullable": false},
                        {"name": "name", "type": "text", "native_type": "text", "nullable": false},
                        {"name": "email", "type": "string", "native_type": "character varying", "nullable": true}
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
        // Verify native_type is used as data_type
        assert_eq!(
            users.get_column("email").unwrap().data_type,
            "character varying"
        );
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

    #[test]
    fn test_parse_schema_definition_format() {
        // Test the new SchemaDefinition.schema.json format
        let json = json!({
            "version": "1.0.0",
            "captured_at": "2025-01-08T10:30:00Z",
            "database": {
                "engine": "postgres",
                "version": "16.1"
            },
            "extensions": ["uuid-ossp"],
            "enums": [
                {
                    "name": "order_status",
                    "schema": "public",
                    "values": ["pending", "shipped", "delivered"]
                }
            ],
            "tables": [
                {
                    "name": "customers",
                    "schema": "public",
                    "columns": [
                        {
                            "name": "id",
                            "type": "uuid",
                            "native_type": "uuid",
                            "nullable": false,
                            "default": "gen_random_uuid()"
                        },
                        {
                            "name": "name",
                            "type": "string",
                            "native_type": "character varying",
                            "nullable": false,
                            "max_length": 255
                        },
                        {
                            "name": "email",
                            "type": "string",
                            "native_type": "character varying",
                            "nullable": true
                        }
                    ],
                    "primary_key": ["id"],
                    "foreign_keys": []
                },
                {
                    "name": "orders",
                    "schema": "public",
                    "columns": [
                        {
                            "name": "id",
                            "type": "uuid",
                            "native_type": "uuid",
                            "nullable": false
                        },
                        {
                            "name": "customer_id",
                            "type": "uuid",
                            "native_type": "uuid",
                            "nullable": false
                        },
                        {
                            "name": "status",
                            "type": "enum",
                            "native_type": "USER-DEFINED",
                            "nullable": false,
                            "enum_name": "order_status"
                        }
                    ],
                    "primary_key": ["id"],
                    "foreign_keys": [
                        {
                            "name": "orders_customer_fk",
                            "columns": ["customer_id"],
                            "references": {
                                "table": "customers",
                                "schema": "public",
                                "columns": ["id"]
                            },
                            "on_delete": "cascade"
                        }
                    ]
                }
            ]
        });

        let schema = parse_schema_from_json(&json).unwrap();

        // Verify customers table
        let customers = schema.get_table("customers").unwrap();
        assert_eq!(customers.name, "customers");
        assert_eq!(customers.columns.len(), 3);
        assert_eq!(customers.primary_key, vec!["id"]);

        // Verify column uses native_type
        let id_col = customers.get_column("id").unwrap();
        assert_eq!(id_col.data_type, "uuid");
        assert!(id_col.is_primary_key);

        let name_col = customers.get_column("name").unwrap();
        assert_eq!(name_col.data_type, "character varying");
        assert!(!name_col.nullable);

        // Verify orders table with foreign key
        let orders = schema.get_table("orders").unwrap();
        assert_eq!(orders.name, "orders");
        assert_eq!(orders.foreign_keys.len(), 1);

        let fk = &orders.foreign_keys[0];
        assert_eq!(fk.name, "orders_customer_fk");
        assert_eq!(fk.columns.len(), 1);
        assert_eq!(fk.columns[0].column, "customer_id");
        assert_eq!(fk.columns[0].foreign_table, "customers");
        assert_eq!(fk.columns[0].foreign_column, "id");
    }
}
