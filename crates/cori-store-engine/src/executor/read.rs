// SPDX-License-Identifier: Apache-2.0
//! The reads: get (one row by its primary key) and list (a page of rows, filtered by equality on readable columns).

use super::sql::{Binds, ident, ident_list, row_to_json};
use super::{ExecutionContext, ExecutionResult, ToolExecutor};
use cori_store_core::protocol::{CallToolOptions, DryRunResult};
use serde_json::{Value, json};
use sqlx::PgConnection;

impl ToolExecutor {
    /// Execute a GET operation.
    pub(super) async fn execute_get(
        &self,
        db: &mut PgConnection,
        table: &str,
        arguments: &Value,
        options: &CallToolOptions,
        context: &ExecutionContext,
    ) -> ExecutionResult {
        if let Err(e) = self.validate_tenant_for_table(table, &context.tenant_id) {
            return ExecutionResult::error(e);
        }

        // Get primary key columns from schema (supports composite keys)
        let pk_columns = self.get_primary_key_columns(table);

        // Cannot execute GET without a primary key
        if pk_columns.is_empty() {
            return ExecutionResult::error(format!(
                "Cannot get record from table '{}': no primary key defined in schema",
                table
            ));
        }

        let tenant_column = self.tenant_column_for_table(table);

        let keys = match self.key_values(table, arguments) {
            Ok(keys) => keys,
            Err(e) => return ExecutionResult::error(e),
        };

        if options.dry_run {
            let pk_conditions: Vec<String> = pk_columns
                .iter()
                .enumerate()
                .map(|(i, col)| format!("{} = ${}", col, i + 1))
                .collect();
            let where_clause = if let Some(tc) = &tenant_column {
                format!(
                    " WHERE {} AND {} = ${}",
                    pk_conditions.join(" AND "),
                    tc,
                    pk_columns.len() + 1
                )
            } else {
                format!(" WHERE {}", pk_conditions.join(" AND "))
            };
            return ExecutionResult::dry_run(DryRunResult {
                dry_run: true,
                would_affect: json!({
                    table: { "select": 1 }
                }),
                preview: Some(json!({
                    "query": format!("SELECT * FROM {}{}", table, where_clause),
                    "pk_columns": pk_columns,
                    "tenant_id": context.tenant_id
                })),
            });
        }


        // Get the readable columns for this table (or use * if not specified)
        let columns = self.get_readable_columns(table);
        let column_list = if columns.is_empty() {
            "*".to_string()
        } else {
            ident_list(&columns)
        };

        let mut binds = Binds::default();
        let conditions = self.row_conditions(&mut binds, table, &keys, &context.tenant_id);
        let query = format!("SELECT {} FROM {} WHERE {}", column_list, ident(table), conditions.join(" AND "));

        tracing::debug!("Executing GET query: {}", query);

        let result = binds.query(&query).fetch_optional(&mut *db).await;

        match result {
            Ok(Some(row)) => {
                let data = row_to_json(&row, &columns);
                ExecutionResult::success_with_sql(data, &query)
            }
            Ok(None) => ExecutionResult::success_with_sql(
                json!({
                    "data": null,
                    "message": "Record not found"
                }),
                &query,
            ),
            Err(e) => ExecutionResult::error(format!("Database error: {}", e)),
        }
    }

    /// Execute a LIST operation.
    pub(super) async fn execute_list(
        &self,
        db: &mut PgConnection,
        table: &str,
        arguments: &Value,
        options: &CallToolOptions,
        context: &ExecutionContext,
    ) -> ExecutionResult {
        if let Err(e) = self.validate_tenant_for_table(table, &context.tenant_id) {
            return ExecutionResult::error(e);
        }
        let limit = arguments
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(50);
        let offset = arguments
            .get("offset")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        // Apply max rows limit from role (per-table max_per_page)
        let max_rows = self.role.get_max_per_page(table).unwrap_or(1000);
        let effective_limit = limit.min(max_rows);
        let tenant_column = self.tenant_column_for_table(table);
        // pages follow the primary key: without an order, LIMIT/OFFSET pages may repeat or skip rows
        let pk = self.get_primary_key_columns(table);
        let order_by = if pk.is_empty() { String::new() } else { format!(" ORDER BY {}", ident_list(&pk)) };

        if options.dry_run {
            let where_clause = if let Some(tc) = &tenant_column {
                format!(" WHERE {} = $1", tc)
            } else {
                "".to_string()
            };
            return ExecutionResult::dry_run(DryRunResult {
                dry_run: true,
                would_affect: json!({
                    table: { "select": "unknown" }
                }),
                preview: Some(json!({
                    "query": format!("SELECT * FROM {}{}{} LIMIT {} OFFSET {}", table, where_clause, order_by, effective_limit, offset),
                    "params": if tenant_column.is_some() { json!([context.tenant_id]) } else { json!([]) }
                })),
            });
        }


        // Get the readable columns for this table (or use * if not specified)
        let columns = self.get_readable_columns(table);
        let column_list = if columns.is_empty() {
            "*".to_string()
        } else {
            ident_list(&columns)
        };

        // Build filter conditions from arguments (excluding limit/offset); the tenant comes from the caller's token
        let mut binds = Binds::default();
        let mut conditions: Vec<String> = Vec::new();
        if let Some(tc) = &tenant_column {
            conditions.push(self.eq(&mut binds, table, tc, &Value::String(context.tenant_id.clone())));
        }
        conditions.extend(self.active_condition(table));

        // Add user-provided filter conditions: equality on columns the role can read, nothing else. An unreadable
        // column would leak its values one guess at a time; an unknown key or a non-scalar value is an error rather
        // than a filter silently dropped.
        let filterable = self.filterable_columns(table);
        let empty_map = serde_json::Map::new();
        let args_map = arguments.as_object().unwrap_or(&empty_map);
        for (key, value) in args_map {
            if key == "limit" || key == "offset" || key == "dryRun" {
                continue;
            }
            if !filterable.iter().any(|c| c == key) {
                return ExecutionResult::error(format!(
                    "Unknown filter '{}' for {}: filter by one of the readable columns {:?} (equality only), plus limit and offset",
                    key, table, filterable
                ));
            }
            match value {
                Value::Null => conditions.push(format!("{} IS NULL", ident(key))),
                Value::String(_) | Value::Number(_) | Value::Bool(_) => conditions.push(self.eq(&mut binds, table, key, value)),
                _ => {
                    return ExecutionResult::error(format!(
                        "Filter '{}' must be a string, number, boolean or null (equality only)",
                        key
                    ));
                }
            }
        }

        let where_clause = if conditions.is_empty() { String::new() } else { format!(" WHERE {}", conditions.join(" AND ")) };
        let query = format!("SELECT {column_list} FROM {}{where_clause}{order_by} LIMIT {effective_limit} OFFSET {offset}", ident(table));

        tracing::debug!("Executing LIST query: {}", query);

        let result = binds.query(&query).fetch_all(&mut *db).await;

        match result {
            Ok(rows) => {
                let data: Vec<Value> = rows.iter().map(|r| row_to_json(r, &columns)).collect();
                ExecutionResult::success_with_sql(
                    json!({
                        "data": data,
                        "count": data.len(),
                        "limit": effective_limit,
                        "offset": offset
                    }),
                    &query,
                )
            }
            Err(e) => ExecutionResult::error(format!("Database error: {}", e)),
        }
    }
}
