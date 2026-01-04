//! Schema conversion utilities.

use crate::state::{SchemaInfo, TableInfo, ColumnInfo, ForeignKeyInfo};
use cori_mcp::schema::{DatabaseSchema, TableSchema, ColumnSchema, ForeignKey, ForeignKeyColumn};
use chrono::Utc;

/// Convert JSON schema from introspection to SchemaInfo.
pub fn json_to_schema_info(json: &serde_json::Value) -> anyhow::Result<SchemaInfo> {
    let tables_json = json.get("tables")
        .and_then(|t| t.as_array())
        .ok_or_else(|| anyhow::anyhow!("Missing 'tables' array in schema JSON"))?;
    
    let mut tables = Vec::new();
    let empty_vec = vec![];
    
    for table_json in tables_json {
        let schema = table_json.get("schema")
            .and_then(|s| s.as_str())
            .unwrap_or("public")
            .to_string();
        
        let name = table_json.get("name")
            .and_then(|n| n.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing table name"))?
            .to_string();
        
        let columns_json = table_json.get("columns")
            .and_then(|c| c.as_array())
            .unwrap_or(&empty_vec);
        
        let columns: Vec<ColumnInfo> = columns_json.iter().map(|c| {
            ColumnInfo {
                name: c.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string(),
                data_type: c.get("data_type").and_then(|t| t.as_str()).unwrap_or("").to_string(),
                nullable: c.get("nullable").and_then(|n| n.as_bool()).unwrap_or(true),
                default: c.get("default").and_then(|d| d.as_str()).map(|s| s.to_string()),
            }
        }).collect();
        
        let primary_key: Vec<String> = table_json.get("primary_key")
            .and_then(|pk| pk.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
            .unwrap_or_default();
        
        // Foreign keys from introspect_schema_json have structure:
        // { "name": "fk_name", "mappings": [{ "column": "col", "references": { "schema": "...", "table": "...", "column": "..." } }] }
        let foreign_keys: Vec<ForeignKeyInfo> = table_json.get("foreign_keys")
            .and_then(|fks| fks.as_array())
            .map(|arr| arr.iter().filter_map(|fk| {
                let name = fk.get("name").and_then(|n| n.as_str())?.to_string();
                let mappings = fk.get("mappings").and_then(|m| m.as_array())?;
                
                let columns: Vec<String> = mappings.iter()
                    .filter_map(|m| m.get("column").and_then(|c| c.as_str()).map(|s| s.to_string()))
                    .collect();
                
                let references_table = mappings.first()
                    .and_then(|m| m.get("references"))
                    .and_then(|r| r.get("table"))
                    .and_then(|t| t.as_str())
                    .unwrap_or("")
                    .to_string();
                
                let references_columns: Vec<String> = mappings.iter()
                    .filter_map(|m| m.get("references").and_then(|r| r.get("column")).and_then(|c| c.as_str()).map(|s| s.to_string()))
                    .collect();
                
                Some(ForeignKeyInfo {
                    name,
                    columns,
                    references_table,
                    references_columns,
                })
            }).collect())
            .unwrap_or_default();
        
        // Detect tenant column based on common naming conventions
        let detected_tenant_column = detect_tenant_column(&columns);
        
        tables.push(TableInfo {
            schema,
            name,
            columns,
            primary_key,
            foreign_keys,
            detected_tenant_column,
        });
    }
    
    Ok(SchemaInfo {
        tables,
        refreshed_at: Utc::now(),
    })
}

/// Detect tenant column based on common naming conventions.
fn detect_tenant_column(columns: &[ColumnInfo]) -> Option<String> {
    const TENANT_COLUMN_NAMES: &[&str] = &[
        "tenant_id",
        "organization_id",
        "org_id",
        "customer_id",
        "account_id",
        "client_id",
        "company_id",
        "workspace_id",
    ];
    
    for col in columns {
        let lower_name = col.name.to_lowercase();
        for pattern in TENANT_COLUMN_NAMES {
            if lower_name == *pattern {
                return Some(col.name.clone());
            }
        }
    }
    
    None
}

/// Convert SchemaInfo to DatabaseSchema for MCP tool generation.
pub fn convert_to_db_schema(schema: &SchemaInfo) -> DatabaseSchema {
    let tables = schema.tables.iter().map(|t| {
        let columns = t.columns.iter().map(|c| {
            ColumnSchema {
                name: c.name.clone(),
                data_type: c.data_type.clone(),
                nullable: c.nullable,
                is_primary_key: t.primary_key.contains(&c.name),
                default: c.default.clone(),
                description: None,
            }
        }).collect();
        
        let foreign_keys = t.foreign_keys.iter().map(|fk| {
            let fk_columns: Vec<ForeignKeyColumn> = fk.columns.iter().zip(fk.references_columns.iter())
                .map(|(col, ref_col)| ForeignKeyColumn {
                    column: col.clone(),
                    foreign_schema: None,
                    foreign_table: fk.references_table.clone(),
                    foreign_column: ref_col.clone(),
                })
                .collect();
            
            ForeignKey {
                name: fk.name.clone(),
                columns: fk_columns,
            }
        }).collect();
        
        (t.name.clone(), TableSchema {
            name: t.name.clone(),
            schema: Some(t.schema.clone()),
            columns,
            primary_key: t.primary_key.clone(),
            foreign_keys,
        })
    }).collect();
    
    DatabaseSchema { tables }
}
