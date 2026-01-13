use chrono::Utc;
use serde_json::json;
use sqlx::{PgPool, Row};
use std::collections::BTreeMap;

/// Schema version for the generated output.
const SCHEMA_VERSION: &str = "1.0.0";

/// Introspect a Postgres database schema following SchemaDefinition.schema.json.
/// Excludes system schemas (pg_catalog, information_schema).
///
/// The output follows the SchemaDefinition JSON schema with:
/// - `version`: Schema version (semver)
/// - `captured_at`: ISO 8601 timestamp
/// - `database.engine`: Database engine type
/// - `database.version`: Database version string (optional)
/// - `extensions`: List of enabled extensions
/// - `enums`: Custom enum types
/// - `tables`: Table definitions with columns, primary keys, foreign keys, indexes
pub async fn introspect_schema_json(database_url: &str) -> anyhow::Result<serde_json::Value> {
    let pool = PgPool::connect(database_url).await?;

    let (version,): (String,) = sqlx::query_as("select version()").fetch_one(&pool).await?;

    // Get enabled extensions
    let extension_rows = sqlx::query(
        r#"
        select extname
        from pg_extension
        where extname not in ('plpgsql')
        order by extname
        "#,
    )
    .fetch_all(&pool)
    .await?;

    let extensions: Vec<String> = extension_rows
        .into_iter()
        .map(|r| r.get::<String, _>("extname"))
        .collect();

    // Get custom enum types
    let enum_rows = sqlx::query(
        r#"
        select t.typname as name, n.nspname as schema,
               array_agg(e.enumlabel order by e.enumsortorder) as values
        from pg_type t
        join pg_enum e on t.oid = e.enumtypid
        join pg_namespace n on t.typnamespace = n.oid
        where n.nspname not in ('pg_catalog', 'information_schema')
        group by t.typname, n.nspname
        order by n.nspname, t.typname
        "#,
    )
    .fetch_all(&pool)
    .await?;

    let enums: Vec<serde_json::Value> = enum_rows
        .into_iter()
        .map(|r| {
            let name: String = r.get("name");
            let schema: String = r.get("schema");
            let values: Vec<String> = r.get("values");
            json!({
                "name": name,
                "schema": schema,
                "values": values
            })
        })
        .collect();

    // Get tables
    let table_rows = sqlx::query(
        r#"
        select table_schema, table_name
        from information_schema.tables
        where table_type = 'BASE TABLE'
          and table_schema not in ('pg_catalog', 'information_schema')
        order by table_schema, table_name
        "#,
    )
    .fetch_all(&pool)
    .await?;

    let mut tables_json = Vec::new();

    for row in table_rows {
        let table_schema: String = row.get("table_schema");
        let table_name: String = row.get("table_name");

        // Columns with extended information
        let col_rows = sqlx::query(
            r#"
            select 
                c.column_name,
                c.data_type,
                c.udt_name,
                c.is_nullable,
                c.column_default,
                c.character_maximum_length
            from information_schema.columns c
            where c.table_schema = $1 and c.table_name = $2
            order by c.ordinal_position
            "#,
        )
        .bind(&table_schema)
        .bind(&table_name)
        .fetch_all(&pool)
        .await?;

        let mut columns = Vec::new();
        for c in col_rows {
            let column_name: String = c.get("column_name");
            let data_type: String = c.get("data_type");
            let udt_name: String = c.get("udt_name");
            let is_nullable: String = c.get("is_nullable");
            let column_default: Option<String> = c.get("column_default");
            let max_length: Option<i32> = c.get("character_maximum_length");

            // Map PostgreSQL data types to generic types
            let (generic_type, enum_name) = map_pg_type_to_generic(&data_type, &udt_name);

            let mut col_json = json!({
                "name": column_name,
                "type": generic_type,
                "native_type": data_type,
                "nullable": is_nullable == "YES"
            });

            // Add optional fields
            if let Some(def) = column_default {
                col_json["default"] = json!(def);
            }
            if let Some(len) = max_length {
                col_json["max_length"] = json!(len);
            }
            if let Some(enum_ref) = enum_name {
                col_json["enum_name"] = json!(enum_ref);
            }

            columns.push(col_json);
        }

        // Primary key columns
        let pk_rows = sqlx::query(
            r#"
            select kcu.column_name
            from information_schema.table_constraints tc
            join information_schema.key_column_usage kcu
              on tc.constraint_name = kcu.constraint_name
             and tc.table_schema = kcu.table_schema
            where tc.constraint_type = 'PRIMARY KEY'
              and tc.table_schema = $1
              and tc.table_name = $2
            order by kcu.ordinal_position
            "#,
        )
        .bind(&table_schema)
        .bind(&table_name)
        .fetch_all(&pool)
        .await?;

        let primary_key: Vec<String> = pk_rows
            .into_iter()
            .map(|r| r.get::<String, _>("column_name"))
            .collect();

        // Foreign keys with on_delete/on_update actions
        let fk_rows = sqlx::query(
            r#"
            select
              tc.constraint_name,
              kcu.column_name as column_name,
              ccu.table_schema as foreign_table_schema,
              ccu.table_name as foreign_table_name,
              ccu.column_name as foreign_column_name,
              rc.delete_rule,
              rc.update_rule
            from information_schema.table_constraints tc
            join information_schema.key_column_usage kcu
              on tc.constraint_name = kcu.constraint_name
             and tc.table_schema = kcu.table_schema
            join information_schema.constraint_column_usage ccu
              on ccu.constraint_name = tc.constraint_name
             and ccu.table_schema = tc.table_schema
            join information_schema.referential_constraints rc
              on rc.constraint_name = tc.constraint_name
             and rc.constraint_schema = tc.table_schema
            where tc.constraint_type = 'FOREIGN KEY'
              and tc.table_schema = $1
              and tc.table_name = $2
            order by tc.constraint_name, kcu.ordinal_position
            "#,
        )
        .bind(&table_schema)
        .bind(&table_name)
        .fetch_all(&pool)
        .await?;

        // Group FK rows by constraint name
        #[derive(Default)]
        struct FkInfo {
            columns: Vec<String>,
            ref_schema: String,
            ref_table: String,
            ref_columns: Vec<String>,
            on_delete: Option<String>,
            on_update: Option<String>,
        }

        let mut fk_map: BTreeMap<String, FkInfo> = BTreeMap::new();
        for fk in fk_rows {
            let constraint_name: String = fk.get("constraint_name");
            let column_name: String = fk.get("column_name");
            let foreign_table_schema: String = fk.get("foreign_table_schema");
            let foreign_table_name: String = fk.get("foreign_table_name");
            let foreign_column_name: String = fk.get("foreign_column_name");
            let delete_rule: String = fk.get("delete_rule");
            let update_rule: String = fk.get("update_rule");

            let entry = fk_map.entry(constraint_name).or_default();
            entry.columns.push(column_name);
            entry.ref_schema = foreign_table_schema;
            entry.ref_table = foreign_table_name;
            entry.ref_columns.push(foreign_column_name);
            entry.on_delete = map_fk_action(&delete_rule);
            entry.on_update = map_fk_action(&update_rule);
        }

        let foreign_keys: Vec<serde_json::Value> = fk_map
            .into_iter()
            .map(|(name, info)| {
                let mut fk_json = json!({
                    "name": name,
                    "columns": info.columns,
                    "references": {
                        "table": info.ref_table,
                        "schema": info.ref_schema,
                        "columns": info.ref_columns
                    }
                });

                if let Some(on_delete) = info.on_delete {
                    fk_json["on_delete"] = json!(on_delete);
                }
                if let Some(on_update) = info.on_update {
                    fk_json["on_update"] = json!(on_update);
                }

                fk_json
            })
            .collect();

        // Get indexes (excluding primary key indexes)
        let idx_rows = sqlx::query(
            r#"
            select
                i.relname as index_name,
                ix.indisunique as is_unique,
                array_agg(a.attname order by array_position(ix.indkey, a.attnum)) as columns
            from pg_class t
            join pg_index ix on t.oid = ix.indrelid
            join pg_class i on i.oid = ix.indexrelid
            join pg_namespace n on n.oid = t.relnamespace
            join pg_attribute a on a.attrelid = t.oid and a.attnum = any(ix.indkey)
            where n.nspname = $1
              and t.relname = $2
              and not ix.indisprimary
            group by i.relname, ix.indisunique
            order by i.relname
            "#,
        )
        .bind(&table_schema)
        .bind(&table_name)
        .fetch_all(&pool)
        .await?;

        let indexes: Vec<serde_json::Value> = idx_rows
            .into_iter()
            .map(|r| {
                let name: String = r.get("index_name");
                let unique: bool = r.get("is_unique");
                let columns: Vec<String> = r.get("columns");
                json!({
                    "name": name,
                    "columns": columns,
                    "unique": unique
                })
            })
            .collect();

        let mut table_json = json!({
            "name": table_name,
            "schema": table_schema,
            "columns": columns
        });

        // Add optional fields only if non-empty
        if !primary_key.is_empty() {
            table_json["primary_key"] = json!(primary_key);
        }
        if !foreign_keys.is_empty() {
            table_json["foreign_keys"] = json!(foreign_keys);
        }
        if !indexes.is_empty() {
            table_json["indexes"] = json!(indexes);
        }

        tables_json.push(table_json);
    }

    let mut result = json!({
        "version": SCHEMA_VERSION,
        "captured_at": Utc::now().to_rfc3339(),
        "database": {
            "engine": "postgres",
            "version": version
        },
        "tables": tables_json
    });

    // Add optional fields only if non-empty
    if !extensions.is_empty() {
        result["extensions"] = json!(extensions);
    }
    if !enums.is_empty() {
        result["enums"] = json!(enums);
    }

    Ok(result)
}

/// Map PostgreSQL data types to generic schema types.
/// Returns (generic_type, optional_enum_name).
fn map_pg_type_to_generic(data_type: &str, udt_name: &str) -> (&'static str, Option<String>) {
    match data_type {
        "character varying" | "character" | "char" | "varchar" | "bpchar" => ("string", None),
        "text" => ("text", None),
        "integer" | "int" | "int4" | "serial" => ("integer", None),
        "bigint" | "int8" | "bigserial" => ("bigint", None),
        "smallint" | "int2" | "smallserial" => ("smallint", None),
        "numeric" | "decimal" => ("decimal", None),
        "real" | "float4" => ("float", None),
        "double precision" | "float8" => ("double", None),
        "boolean" | "bool" => ("boolean", None),
        "date" => ("date", None),
        "time" | "time without time zone" | "time with time zone" => ("time", None),
        "timestamp without time zone" => ("datetime", None),
        "timestamp with time zone" => ("timestamp", None),
        "uuid" => ("uuid", None),
        "json" => ("json", None),
        "jsonb" => ("jsonb", None),
        "bytea" => ("binary", None),
        "ARRAY" => ("array", None),
        "USER-DEFINED" => {
            // This is likely an enum type - the actual enum name is in udt_name
            ("enum", Some(udt_name.to_string()))
        }
        _ => ("unknown", None),
    }
}

/// Map PostgreSQL referential action to schema action name.
fn map_fk_action(action: &str) -> Option<String> {
    match action {
        "NO ACTION" => Some("no_action".to_string()),
        "RESTRICT" => Some("restrict".to_string()),
        "CASCADE" => Some("cascade".to_string()),
        "SET NULL" => Some("set_null".to_string()),
        "SET DEFAULT" => Some("set_default".to_string()),
        _ => None,
    }
}
