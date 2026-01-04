//! Virtual schema response generation.
//!
//! This module generates synthetic responses for schema queries based on
//! the token's permissions.

use super::detector::{SchemaQueryInfo, SchemaView};
use std::collections::HashMap;

/// A virtual schema response containing filtered schema information.
#[derive(Debug, Clone)]
pub struct VirtualSchemaResponse {
    /// Column names in the response.
    pub columns: Vec<ColumnDef>,
    /// Row data.
    pub rows: Vec<Vec<Option<String>>>,
    /// Number of rows.
    pub row_count: usize,
}

/// Column definition in a schema response.
#[derive(Debug, Clone)]
pub struct ColumnDef {
    /// Column name.
    pub name: String,
    /// PostgreSQL type OID.
    pub type_oid: u32,
}

impl ColumnDef {
    /// Create a new text column.
    pub fn text(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            type_oid: 25, // TEXT
        }
    }

    /// Create a new integer column.
    pub fn int(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            type_oid: 23, // INT4
        }
    }

    /// Create a new boolean column.
    pub fn boolean(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            type_oid: 16, // BOOL
        }
    }
}

/// Permissions used to filter schema responses.
#[derive(Debug, Clone, Default)]
pub struct SchemaPermissions {
    /// Tables that are accessible.
    pub accessible_tables: Vec<String>,
    /// Readable columns per table.
    pub readable_columns: HashMap<String, Vec<String>>,
    /// Tables that are always visible (from config).
    pub always_visible: Vec<String>,
    /// The schema to show (typically "public").
    pub schema_name: String,
}

impl SchemaPermissions {
    /// Create new permissions with accessible tables.
    pub fn new(tables: Vec<String>) -> Self {
        Self {
            accessible_tables: tables,
            readable_columns: HashMap::new(),
            always_visible: Vec::new(),
            schema_name: "public".to_string(),
        }
    }

    /// Add readable columns for a table.
    pub fn with_columns(mut self, table: impl Into<String>, columns: Vec<String>) -> Self {
        self.readable_columns.insert(table.into(), columns);
        self
    }

    /// Set always visible tables.
    pub fn with_always_visible(mut self, tables: Vec<String>) -> Self {
        self.always_visible = tables;
        self
    }

    /// Check if a table is visible.
    pub fn is_table_visible(&self, table: &str) -> bool {
        self.accessible_tables.iter().any(|t| t == table)
            || self.always_visible.iter().any(|t| t == table)
    }

    /// Get readable columns for a table.
    pub fn get_readable_columns(&self, table: &str) -> Option<&Vec<String>> {
        self.readable_columns.get(table)
    }

    /// Get all visible tables.
    pub fn all_visible_tables(&self) -> Vec<&str> {
        let mut tables: Vec<&str> = self.accessible_tables.iter().map(|s| s.as_str()).collect();
        for t in &self.always_visible {
            if !tables.contains(&t.as_str()) {
                tables.push(t.as_str());
            }
        }
        tables
    }
}

/// Response generator for virtual schema queries.
pub struct ResponseGenerator;

impl ResponseGenerator {
    /// Generate a response for a schema query with the given permissions.
    pub fn generate(
        query_info: &SchemaQueryInfo,
        permissions: &SchemaPermissions,
    ) -> VirtualSchemaResponse {
        let mut response = match query_info.primary_view {
            SchemaView::InformationSchemaTables => {
                Self::generate_information_schema_tables(query_info, permissions)
            }
            SchemaView::InformationSchemaColumns => {
                Self::generate_information_schema_columns(query_info, permissions)
            }
            SchemaView::InformationSchemaTableConstraints => {
                Self::generate_information_schema_table_constraints(query_info, permissions)
            }
            SchemaView::InformationSchemaKeyColumnUsage => {
                Self::generate_information_schema_key_column_usage(query_info, permissions)
            }
            SchemaView::InformationSchemaSchemata => {
                Self::generate_information_schema_schemata(permissions)
            }
            SchemaView::PgCatalogPgTables => {
                Self::generate_pg_catalog_pg_tables(query_info, permissions)
            }
            SchemaView::PgCatalogPgAttribute => {
                Self::generate_pg_catalog_pg_attribute(query_info, permissions)
            }
            SchemaView::PgCatalogPgClass => {
                Self::generate_pg_catalog_pg_class(query_info, permissions)
            }
            SchemaView::PgCatalogPgNamespace => Self::generate_pg_catalog_pg_namespace(permissions),
            SchemaView::OtherInformationSchema | SchemaView::OtherPgCatalog => {
                // Return empty result for unknown schema views
                VirtualSchemaResponse {
                    columns: vec![],
                    rows: vec![],
                    row_count: 0,
                }
            }
        };

        // Filter columns if specific columns were selected (not SELECT *)
        if !query_info.selected_columns.is_empty() 
            && !query_info.selected_columns.iter().any(|c| c == "*") {
            response = Self::filter_columns(response, &query_info.selected_columns);
        }

        response
    }

    /// Filter response to only include selected columns.
    /// This handles queries like "SELECT nspname FROM pg_namespace" by removing unselected columns.
    fn filter_columns(
        response: VirtualSchemaResponse,
        selected_columns: &[String],
    ) -> VirtualSchemaResponse {
        // Build a map of column names to their indices in the full response
        let column_indices: HashMap<String, usize> = response
            .columns
            .iter()
            .enumerate()
            .map(|(idx, col)| (col.name.to_lowercase(), idx))
            .collect();

        // Find indices of selected columns
        let mut selected_indices = Vec::new();
        let mut filtered_columns = Vec::new();
        
        for selected_col in selected_columns {
            let col_lower = selected_col.to_lowercase();
            if let Some(&idx) = column_indices.get(&col_lower) {
                selected_indices.push(idx);
                filtered_columns.push(response.columns[idx].clone());
            }
        }

        // If no matches found, return original response (safer than empty)
        if selected_indices.is_empty() {
            return response;
        }

        // Filter rows to only include selected column values
        let filtered_rows: Vec<Vec<Option<String>>> = response
            .rows
            .into_iter()
            .map(|row| {
                selected_indices
                    .iter()
                    .filter_map(|&idx| row.get(idx).cloned())
                    .collect()
            })
            .collect();

        VirtualSchemaResponse {
            columns: filtered_columns,
            rows: filtered_rows,
            row_count: response.row_count,
        }
    }

    /// Generate response for information_schema.tables.
    fn generate_information_schema_tables(
        query_info: &SchemaQueryInfo,
        permissions: &SchemaPermissions,
    ) -> VirtualSchemaResponse {
        let columns = vec![
            ColumnDef::text("table_catalog"),
            ColumnDef::text("table_schema"),
            ColumnDef::text("table_name"),
            ColumnDef::text("table_type"),
            ColumnDef::text("self_referencing_column_name"),
            ColumnDef::text("reference_generation"),
            ColumnDef::text("user_defined_type_catalog"),
            ColumnDef::text("user_defined_type_schema"),
            ColumnDef::text("user_defined_type_name"),
            ColumnDef::text("is_insertable_into"),
            ColumnDef::text("is_typed"),
            ColumnDef::text("commit_action"),
        ];

        let mut rows = Vec::new();
        let schema_name = &permissions.schema_name;

        // Filter by table_name if specified in query
        let table_filter = query_info.table_name_filter.as_deref();

        for table in permissions.all_visible_tables() {
            // Apply table_name filter if present
            if let Some(filter) = table_filter {
                if table != filter {
                    continue;
                }
            }

            rows.push(vec![
                Some("postgres".to_string()), // table_catalog
                Some(schema_name.clone()),    // table_schema
                Some(table.to_string()),      // table_name
                Some("BASE TABLE".to_string()), // table_type
                None,                         // self_referencing_column_name
                None,                         // reference_generation
                None,                         // user_defined_type_catalog
                None,                         // user_defined_type_schema
                None,                         // user_defined_type_name
                Some("YES".to_string()),      // is_insertable_into
                Some("NO".to_string()),       // is_typed
                None,                         // commit_action
            ]);
        }

        let row_count = rows.len();
        VirtualSchemaResponse {
            columns,
            rows,
            row_count,
        }
    }

    /// Generate response for information_schema.columns.
    fn generate_information_schema_columns(
        query_info: &SchemaQueryInfo,
        permissions: &SchemaPermissions,
    ) -> VirtualSchemaResponse {
        let columns = vec![
            ColumnDef::text("table_catalog"),
            ColumnDef::text("table_schema"),
            ColumnDef::text("table_name"),
            ColumnDef::text("column_name"),
            ColumnDef::int("ordinal_position"),
            ColumnDef::text("column_default"),
            ColumnDef::text("is_nullable"),
            ColumnDef::text("data_type"),
            ColumnDef::int("character_maximum_length"),
            ColumnDef::int("character_octet_length"),
            ColumnDef::int("numeric_precision"),
            ColumnDef::int("numeric_scale"),
            ColumnDef::text("datetime_precision"),
            ColumnDef::text("character_set_catalog"),
            ColumnDef::text("character_set_schema"),
            ColumnDef::text("character_set_name"),
            ColumnDef::text("collation_catalog"),
            ColumnDef::text("collation_schema"),
            ColumnDef::text("collation_name"),
            ColumnDef::text("domain_catalog"),
            ColumnDef::text("domain_schema"),
            ColumnDef::text("domain_name"),
            ColumnDef::text("udt_catalog"),
            ColumnDef::text("udt_schema"),
            ColumnDef::text("udt_name"),
            ColumnDef::text("scope_catalog"),
            ColumnDef::text("scope_schema"),
            ColumnDef::text("scope_name"),
            ColumnDef::int("maximum_cardinality"),
            ColumnDef::text("dtd_identifier"),
            ColumnDef::text("is_self_referencing"),
            ColumnDef::text("is_identity"),
            ColumnDef::text("identity_generation"),
            ColumnDef::text("identity_start"),
            ColumnDef::text("identity_increment"),
            ColumnDef::text("identity_maximum"),
            ColumnDef::text("identity_minimum"),
            ColumnDef::text("identity_cycle"),
            ColumnDef::text("is_generated"),
            ColumnDef::text("generation_expression"),
            ColumnDef::text("is_updatable"),
        ];

        let mut rows = Vec::new();
        let schema_name = &permissions.schema_name;

        // Filter by table_name if specified in query
        let table_filter = query_info.table_name_filter.as_deref();

        for table in permissions.all_visible_tables() {
            // Apply table_name filter if present
            if let Some(filter) = table_filter {
                if table != filter {
                    continue;
                }
            }

            // Get readable columns for this table
            let readable_cols = permissions.get_readable_columns(table);

            if let Some(cols) = readable_cols {
                for (idx, column) in cols.iter().enumerate() {
                    rows.push(vec![
                        Some("postgres".to_string()),        // table_catalog
                        Some(schema_name.clone()),           // table_schema
                        Some(table.to_string()),             // table_name
                        Some(column.clone()),                // column_name
                        Some((idx + 1).to_string()),         // ordinal_position
                        None,                                // column_default
                        Some("YES".to_string()),             // is_nullable
                        Some("text".to_string()),            // data_type (simplified)
                        None,                                // character_maximum_length
                        None,                                // character_octet_length
                        None,                                // numeric_precision
                        None,                                // numeric_scale
                        None,                                // datetime_precision
                        None,                                // character_set_catalog
                        None,                                // character_set_schema
                        None,                                // character_set_name
                        None,                                // collation_catalog
                        None,                                // collation_schema
                        None,                                // collation_name
                        None,                                // domain_catalog
                        None,                                // domain_schema
                        None,                                // domain_name
                        Some("postgres".to_string()),        // udt_catalog
                        Some("pg_catalog".to_string()),      // udt_schema
                        Some("text".to_string()),            // udt_name
                        None,                                // scope_catalog
                        None,                                // scope_schema
                        None,                                // scope_name
                        None,                                // maximum_cardinality
                        Some((idx + 1).to_string()),         // dtd_identifier
                        Some("NO".to_string()),              // is_self_referencing
                        Some("NO".to_string()),              // is_identity
                        None,                                // identity_generation
                        None,                                // identity_start
                        None,                                // identity_increment
                        None,                                // identity_maximum
                        None,                                // identity_minimum
                        Some("NO".to_string()),              // identity_cycle
                        Some("NEVER".to_string()),           // is_generated
                        None,                                // generation_expression
                        Some("YES".to_string()),             // is_updatable
                    ]);
                }
            }
        }

        let row_count = rows.len();
        VirtualSchemaResponse {
            columns,
            rows,
            row_count,
        }
    }

    /// Generate response for information_schema.table_constraints.
    fn generate_information_schema_table_constraints(
        query_info: &SchemaQueryInfo,
        permissions: &SchemaPermissions,
    ) -> VirtualSchemaResponse {
        let columns = vec![
            ColumnDef::text("constraint_catalog"),
            ColumnDef::text("constraint_schema"),
            ColumnDef::text("constraint_name"),
            ColumnDef::text("table_catalog"),
            ColumnDef::text("table_schema"),
            ColumnDef::text("table_name"),
            ColumnDef::text("constraint_type"),
            ColumnDef::text("is_deferrable"),
            ColumnDef::text("initially_deferred"),
            ColumnDef::text("enforced"),
            ColumnDef::text("nulls_distinct"),
        ];

        let mut rows = Vec::new();
        let schema_name = &permissions.schema_name;
        let table_filter = query_info.table_name_filter.as_deref();

        // Generate a simple primary key constraint for each visible table
        for table in permissions.all_visible_tables() {
            if let Some(filter) = table_filter {
                if table != filter {
                    continue;
                }
            }

            // Add a primary key constraint
            rows.push(vec![
                Some("postgres".to_string()),                         // constraint_catalog
                Some(schema_name.clone()),                            // constraint_schema
                Some(format!("{}_pkey", table)),                      // constraint_name
                Some("postgres".to_string()),                         // table_catalog
                Some(schema_name.clone()),                            // table_schema
                Some(table.to_string()),                              // table_name
                Some("PRIMARY KEY".to_string()),                      // constraint_type
                Some("NO".to_string()),                               // is_deferrable
                Some("NO".to_string()),                               // initially_deferred
                Some("YES".to_string()),                              // enforced
                None,                                                 // nulls_distinct
            ]);
        }

        let row_count = rows.len();
        VirtualSchemaResponse {
            columns,
            rows,
            row_count,
        }
    }

    /// Generate response for information_schema.key_column_usage.
    fn generate_information_schema_key_column_usage(
        query_info: &SchemaQueryInfo,
        permissions: &SchemaPermissions,
    ) -> VirtualSchemaResponse {
        let columns = vec![
            ColumnDef::text("constraint_catalog"),
            ColumnDef::text("constraint_schema"),
            ColumnDef::text("constraint_name"),
            ColumnDef::text("table_catalog"),
            ColumnDef::text("table_schema"),
            ColumnDef::text("table_name"),
            ColumnDef::text("column_name"),
            ColumnDef::int("ordinal_position"),
            ColumnDef::int("position_in_unique_constraint"),
        ];

        let mut rows = Vec::new();
        let schema_name = &permissions.schema_name;
        let table_filter = query_info.table_name_filter.as_deref();

        // Generate key column usage for visible tables (assuming 'id' is the primary key)
        for table in permissions.all_visible_tables() {
            if let Some(filter) = table_filter {
                if table != filter {
                    continue;
                }
            }

            // Check if 'id' column is readable
            if let Some(cols) = permissions.get_readable_columns(table) {
                if cols.iter().any(|c| c == "id") {
                    rows.push(vec![
                        Some("postgres".to_string()),    // constraint_catalog
                        Some(schema_name.clone()),       // constraint_schema
                        Some(format!("{}_pkey", table)), // constraint_name
                        Some("postgres".to_string()),    // table_catalog
                        Some(schema_name.clone()),       // table_schema
                        Some(table.to_string()),         // table_name
                        Some("id".to_string()),          // column_name
                        Some("1".to_string()),           // ordinal_position
                        None,                            // position_in_unique_constraint
                    ]);
                }
            }
        }

        let row_count = rows.len();
        VirtualSchemaResponse {
            columns,
            rows,
            row_count,
        }
    }

    /// Generate response for information_schema.schemata.
    fn generate_information_schema_schemata(
        permissions: &SchemaPermissions,
    ) -> VirtualSchemaResponse {
        let columns = vec![
            ColumnDef::text("catalog_name"),
            ColumnDef::text("schema_name"),
            ColumnDef::text("schema_owner"),
            ColumnDef::text("default_character_set_catalog"),
            ColumnDef::text("default_character_set_schema"),
            ColumnDef::text("default_character_set_name"),
            ColumnDef::text("sql_path"),
        ];

        // Return only the schema that contains accessible tables
        let rows = vec![vec![
            Some("postgres".to_string()),
            Some(permissions.schema_name.clone()),
            Some("postgres".to_string()),
            None,
            None,
            None,
            None,
        ]];

        VirtualSchemaResponse {
            columns,
            rows,
            row_count: 1,
        }
    }

    /// Generate response for pg_catalog.pg_tables.
    fn generate_pg_catalog_pg_tables(
        query_info: &SchemaQueryInfo,
        permissions: &SchemaPermissions,
    ) -> VirtualSchemaResponse {
        let columns = vec![
            ColumnDef::text("schemaname"),
            ColumnDef::text("tablename"),
            ColumnDef::text("tableowner"),
            ColumnDef::text("tablespace"),
            ColumnDef::boolean("hasindexes"),
            ColumnDef::boolean("hasrules"),
            ColumnDef::boolean("hastriggers"),
            ColumnDef::boolean("rowsecurity"),
        ];

        let mut rows = Vec::new();
        let schema_name = &permissions.schema_name;
        let table_filter = query_info.table_name_filter.as_deref();

        for table in permissions.all_visible_tables() {
            if let Some(filter) = table_filter {
                if table != filter {
                    continue;
                }
            }

            rows.push(vec![
                Some(schema_name.clone()),        // schemaname
                Some(table.to_string()),          // tablename
                Some("postgres".to_string()),    // tableowner
                None,                             // tablespace
                Some("true".to_string()),        // hasindexes
                Some("false".to_string()),       // hasrules
                Some("false".to_string()),       // hastriggers
                Some("false".to_string()),       // rowsecurity
            ]);
        }

        let row_count = rows.len();
        VirtualSchemaResponse {
            columns,
            rows,
            row_count,
        }
    }

    /// Generate response for pg_catalog.pg_attribute.
    fn generate_pg_catalog_pg_attribute(
        query_info: &SchemaQueryInfo,
        permissions: &SchemaPermissions,
    ) -> VirtualSchemaResponse {
        let columns = vec![
            ColumnDef::int("attrelid"),
            ColumnDef::text("attname"),
            ColumnDef::int("atttypid"),
            ColumnDef::int("attstattarget"),
            ColumnDef::int("attlen"),
            ColumnDef::int("attnum"),
            ColumnDef::int("attndims"),
            ColumnDef::int("attcacheoff"),
            ColumnDef::int("atttypmod"),
            ColumnDef::boolean("attbyval"),
            ColumnDef::text("attalign"),
            ColumnDef::text("attstorage"),
            ColumnDef::text("attcompression"),
            ColumnDef::boolean("attnotnull"),
            ColumnDef::boolean("atthasdef"),
            ColumnDef::boolean("atthasmissing"),
            ColumnDef::text("attidentity"),
            ColumnDef::text("attgenerated"),
            ColumnDef::boolean("attisdropped"),
            ColumnDef::boolean("attislocal"),
            ColumnDef::int("attinhcount"),
            ColumnDef::int("attcollation"),
        ];

        let mut rows = Vec::new();
        let table_filter = query_info.table_name_filter.as_deref();

        // Generate a fake OID for each table (starting at 16384)
        let mut table_oid = 16384u32;

        for table in permissions.all_visible_tables() {
            if let Some(filter) = table_filter {
                if table != filter {
                    table_oid += 1;
                    continue;
                }
            }

            if let Some(cols) = permissions.get_readable_columns(table) {
                for (idx, column) in cols.iter().enumerate() {
                    let attnum = (idx + 1) as i32;
                    rows.push(vec![
                        Some(table_oid.to_string()),     // attrelid
                        Some(column.clone()),            // attname
                        Some("25".to_string()),          // atttypid (TEXT)
                        Some("-1".to_string()),          // attstattarget
                        Some("-1".to_string()),          // attlen
                        Some(attnum.to_string()),        // attnum
                        Some("0".to_string()),           // attndims
                        Some("-1".to_string()),          // attcacheoff
                        Some("-1".to_string()),          // atttypmod
                        Some("false".to_string()),       // attbyval
                        Some("i".to_string()),           // attalign
                        Some("x".to_string()),           // attstorage
                        Some("".to_string()),            // attcompression
                        Some("false".to_string()),       // attnotnull
                        Some("false".to_string()),       // atthasdef
                        Some("false".to_string()),       // atthasmissing
                        Some("".to_string()),            // attidentity
                        Some("".to_string()),            // attgenerated
                        Some("false".to_string()),       // attisdropped
                        Some("true".to_string()),        // attislocal
                        Some("0".to_string()),           // attinhcount
                        Some("0".to_string()),           // attcollation
                    ]);
                }
            }
            table_oid += 1;
        }

        let row_count = rows.len();
        VirtualSchemaResponse {
            columns,
            rows,
            row_count,
        }
    }

    /// Generate response for pg_catalog.pg_class.
    fn generate_pg_catalog_pg_class(
        query_info: &SchemaQueryInfo,
        permissions: &SchemaPermissions,
    ) -> VirtualSchemaResponse {
        let columns = vec![
            ColumnDef::int("oid"),
            ColumnDef::text("relname"),
            ColumnDef::int("relnamespace"),
            ColumnDef::int("reltype"),
            ColumnDef::int("reloftype"),
            ColumnDef::int("relowner"),
            ColumnDef::int("relam"),
            ColumnDef::int("relfilenode"),
            ColumnDef::int("reltablespace"),
            ColumnDef::int("relpages"),
            ColumnDef::int("reltuples"),
            ColumnDef::int("relallvisible"),
            ColumnDef::int("reltoastrelid"),
            ColumnDef::boolean("relhasindex"),
            ColumnDef::boolean("relisshared"),
            ColumnDef::text("relpersistence"),
            ColumnDef::text("relkind"),
            ColumnDef::int("relnatts"),
            ColumnDef::int("relchecks"),
            ColumnDef::boolean("relhasrules"),
            ColumnDef::boolean("relhastriggers"),
            ColumnDef::boolean("relhassubclass"),
            ColumnDef::boolean("relrowsecurity"),
            ColumnDef::boolean("relforcerowsecurity"),
            ColumnDef::boolean("relispopulated"),
            ColumnDef::text("relreplident"),
            ColumnDef::boolean("relispartition"),
        ];

        let mut rows = Vec::new();
        let table_filter = query_info.table_name_filter.as_deref();

        // Generate a fake OID for each table
        let mut table_oid = 16384u32;

        for table in permissions.all_visible_tables() {
            if let Some(filter) = table_filter {
                if table != filter {
                    table_oid += 1;
                    continue;
                }
            }

            let num_cols = permissions
                .get_readable_columns(table)
                .map(|c| c.len())
                .unwrap_or(0);

            rows.push(vec![
                Some(table_oid.to_string()),   // oid
                Some(table.to_string()),       // relname
                Some("2200".to_string()),      // relnamespace (public schema OID)
                Some("0".to_string()),         // reltype
                Some("0".to_string()),         // reloftype
                Some("10".to_string()),        // relowner
                Some("2".to_string()),         // relam (heap)
                Some(table_oid.to_string()),   // relfilenode
                Some("0".to_string()),         // reltablespace
                Some("0".to_string()),         // relpages
                Some("0".to_string()),         // reltuples
                Some("0".to_string()),         // relallvisible
                Some("0".to_string()),         // reltoastrelid
                Some("true".to_string()),      // relhasindex
                Some("false".to_string()),     // relisshared
                Some("p".to_string()),         // relpersistence
                Some("r".to_string()),         // relkind (ordinary table)
                Some(num_cols.to_string()),    // relnatts
                Some("0".to_string()),         // relchecks
                Some("false".to_string()),     // relhasrules
                Some("false".to_string()),     // relhastriggers
                Some("false".to_string()),     // relhassubclass
                Some("false".to_string()),     // relrowsecurity
                Some("false".to_string()),     // relforcerowsecurity
                Some("true".to_string()),      // relispopulated
                Some("d".to_string()),         // relreplident
                Some("false".to_string()),     // relispartition
            ]);

            table_oid += 1;
        }

        let row_count = rows.len();
        VirtualSchemaResponse {
            columns,
            rows,
            row_count,
        }
    }

    /// Generate response for pg_catalog.pg_namespace.
    fn generate_pg_catalog_pg_namespace(permissions: &SchemaPermissions) -> VirtualSchemaResponse {
        let columns = vec![
            ColumnDef::int("oid"),
            ColumnDef::text("nspname"),
            ColumnDef::int("nspowner"),
        ];

        // Only show the schema that contains accessible tables
        let rows = vec![vec![
            Some("2200".to_string()),                // oid (public schema)
            Some(permissions.schema_name.clone()),   // nspname
            Some("10".to_string()),                  // nspowner
        ]];

        VirtualSchemaResponse {
            columns,
            rows,
            row_count: 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_permissions() -> SchemaPermissions {
        SchemaPermissions::new(vec!["customers".to_string(), "orders".to_string()])
            .with_columns(
                "customers",
                vec!["id".to_string(), "name".to_string(), "email".to_string()],
            )
            .with_columns(
                "orders",
                vec![
                    "id".to_string(),
                    "customer_id".to_string(),
                    "total".to_string(),
                ],
            )
    }

    #[test]
    fn test_generate_information_schema_tables() {
        use super::super::detector::SchemaQueryDetector;

        let detector = SchemaQueryDetector::new();
        let query_info = detector.detect("SELECT * FROM information_schema.tables WHERE table_schema = 'public'");
        let permissions = create_test_permissions();

        let response = ResponseGenerator::generate(&query_info, &permissions);

        assert_eq!(response.row_count, 2);
        assert!(response.columns.iter().any(|c| c.name == "table_name"));
    }

    #[test]
    fn test_generate_information_schema_columns() {
        use super::super::detector::SchemaQueryDetector;

        let detector = SchemaQueryDetector::new();
        let query_info =
            detector.detect("SELECT column_name FROM information_schema.columns WHERE table_name = 'customers'");
        let permissions = create_test_permissions();

        let response = ResponseGenerator::generate(&query_info, &permissions);

        // Should have 3 columns for the customers table
        assert_eq!(response.row_count, 3);
    }

    #[test]
    fn test_generate_pg_catalog_pg_tables() {
        use super::super::detector::SchemaQueryDetector;

        let detector = SchemaQueryDetector::new();
        let query_info = detector.detect("SELECT * FROM pg_catalog.pg_tables WHERE schemaname = 'public'");
        let permissions = create_test_permissions();

        let response = ResponseGenerator::generate(&query_info, &permissions);

        assert_eq!(response.row_count, 2);
        assert!(response.columns.iter().any(|c| c.name == "tablename"));
    }

    #[test]
    fn test_filtered_by_table_name() {
        use super::super::detector::SchemaQueryDetector;

        let detector = SchemaQueryDetector::new();
        let query_info =
            detector.detect("SELECT * FROM information_schema.tables WHERE table_name = 'customers'");
        let permissions = create_test_permissions();

        let response = ResponseGenerator::generate(&query_info, &permissions);

        // Should only return the customers table
        assert_eq!(response.row_count, 1);
    }

    #[test]
    fn test_always_visible_tables() {
        use super::super::detector::SchemaQueryDetector;

        let detector = SchemaQueryDetector::new();
        let query_info = detector.detect("SELECT * FROM information_schema.tables");
        let permissions = create_test_permissions()
            .with_always_visible(vec!["countries".to_string()]);

        let response = ResponseGenerator::generate(&query_info, &permissions);

        // Should have 3 tables: customers, orders, and countries
        assert_eq!(response.row_count, 3);
    }
}

