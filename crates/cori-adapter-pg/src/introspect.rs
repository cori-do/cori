use chrono::Utc;
use serde_json::json;
use sqlx::{PgPool, Row};
use std::collections::BTreeMap;

/// Introspect a Postgres database schema into a stable JSON snapshot.
/// Excludes system schemas (pg_catalog, information_schema).
pub async fn introspect_schema_json(database_url: &str) -> anyhow::Result<serde_json::Value> {
    let pool = PgPool::connect(database_url).await?;

    let (version,): (String,) = sqlx::query_as("select version()").fetch_one(&pool).await?;

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

        // Columns
        let col_rows = sqlx::query(
            r#"
            select column_name, data_type, is_nullable, column_default
            from information_schema.columns
            where table_schema = $1 and table_name = $2
            order by ordinal_position
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
            let is_nullable: String = c.get("is_nullable");
            let column_default: Option<String> = c.get("column_default");

            columns.push(json!({
                "name": column_name,
                "data_type": data_type,
                "nullable": is_nullable == "YES",
                "default": column_default
            }));
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

        // Foreign keys (grouped by constraint_name for stability)
        let fk_rows = sqlx::query(
            r#"
            select
              tc.constraint_name,
              kcu.column_name as column_name,
              ccu.table_schema as foreign_table_schema,
              ccu.table_name as foreign_table_name,
              ccu.column_name as foreign_column_name
            from information_schema.table_constraints tc
            join information_schema.key_column_usage kcu
              on tc.constraint_name = kcu.constraint_name
             and tc.table_schema = kcu.table_schema
            join information_schema.constraint_column_usage ccu
              on ccu.constraint_name = tc.constraint_name
             and ccu.table_schema = tc.table_schema
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

        let mut fk_map: BTreeMap<String, Vec<serde_json::Value>> = BTreeMap::new();
        for fk in fk_rows {
            let constraint_name: String = fk.get("constraint_name");
            let column_name: String = fk.get("column_name");
            let foreign_table_schema: String = fk.get("foreign_table_schema");
            let foreign_table_name: String = fk.get("foreign_table_name");
            let foreign_column_name: String = fk.get("foreign_column_name");

            fk_map.entry(constraint_name).or_default().push(json!({
                "column": column_name,
                "references": {
                    "schema": foreign_table_schema,
                    "table": foreign_table_name,
                    "column": foreign_column_name
                }
            }));
        }

        let foreign_keys: Vec<serde_json::Value> = fk_map
            .into_iter()
            .map(|(name, mappings)| {
                json!({
                    "name": name,
                    "mappings": mappings
                })
            })
            .collect();

        tables_json.push(json!({
            "schema": table_schema,
            "name": table_name,
            "columns": columns,
            "primary_key": primary_key,
            "foreign_keys": foreign_keys
        }));
    }

    Ok(json!({
        "captured_at": Utc::now().to_rfc3339(),
        "database": {
            "kind": "postgres",
            "version": version
        },
        "tables": tables_json
    }))
}
