// SPDX-License-Identifier: Apache-2.0
//! The writes: create, update and delete (soft or not) of one row, under the role's column constraints.

use super::sql::{Binds, describe_keys, ident, ident_list, row_to_json};
use super::{ExecutionContext, ExecutionResult, ToolExecutor};
use cori_store_core::protocol::{CallToolOptions, DryRunResult};
use serde_json::{Value, json};
use sqlx::PgConnection;

impl ToolExecutor {
    /// Execute a CREATE operation.
    pub(super) async fn execute_create(
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
        let empty_map = serde_json::Map::new();
        let obj = arguments.as_object().unwrap_or(&empty_map);
        let tenant_column = self.owner_column_for_create(table, obj, &context.tenant_id);
        if options.dry_run {
            return ExecutionResult::dry_run(DryRunResult {
                dry_run: true,
                would_affect: json!({
                    table: { "insert": 1 }
                }),
                preview: Some(json!({
                    "operation": "INSERT",
                    "table": table,
                    "data": arguments,
                    "tenant_id": context.tenant_id
                })),
            });
        }

        // SECURITY: Remove tenant column from user input - tenant is ALWAYS set from Biscuit token
        // This prevents any attempt by users to override tenant isolation
        let mut final_args = obj.clone();
        if let Some(tc) = &tenant_column {
            final_args.remove(tc);
        }

        // foreign key columns to check once the row is complete
        let mut fk_columns: Vec<(String, cori_store_core::config::ForeignKeyConstraint)> = Vec::new();

        // Validate required fields and apply default values from role's creatable column constraints
        if let Some(creatable) = self.role.get_creatable_columns(table)
            && let Some(constraints_map) = creatable.as_map() {
                for (col_name, constraints) in constraints_map {
                    // Skip tenant column - it's handled separately from the token
                    if let Some(tc) = &tenant_column
                        && col_name == tc {
                            continue;
                        }

                    if let Some(fk_constraint) = &constraints.foreign_key {
                        fk_columns.push((col_name.clone(), fk_constraint.clone()));
                    }

                    let has_value = final_args.contains_key(col_name);

                    // Check required constraint
                    if constraints.required && !has_value {
                        return ExecutionResult::error(format!(
                            "Missing required field: '{}'",
                            col_name
                        ));
                    }

                    // If column not provided and has a default, apply it
                    if !has_value
                        && let Some(default_val) = &constraints.default {
                            final_args.insert(col_name.clone(), default_val.clone());
                        }
                }
            }

        // verification inputs are not columns of the table
        match self.check_foreign_keys(&mut *db, table, fk_columns, &final_args, &context.tenant_id).await {
            Ok(verify_keys) => for key in verify_keys { final_args.remove(&key); },
            Err(refused) => return ExecutionResult::error(refused),
        }

        // Start with tenant column (if table is tenant-scoped)
        // SECURITY: Tenant value ALWAYS comes from the Biscuit token, never from user input
        let mut binds = Binds::default();
        let mut columns: Vec<String> = Vec::new();
        let mut values: Vec<String> = Vec::new();
        if let Some(tc) = &tenant_column {
            columns.push(ident(tc));
            values.push(binds.value(&Value::String(context.tenant_id.clone()), self.cast(table, tc).as_deref()));
        }

        for (key, value) in &final_args {
            // Validate column name (alphanumeric and underscore only)
            if !key.chars().all(|c| c.is_alphanumeric() || c == '_') {
                continue;
            }
            columns.push(ident(key));
            values.push(binds.value(value, self.cast(table, key).as_deref()));
        }

        // RETURNING only what the role reads: the data role is granted nothing else
        let returning = self.returning_list(table);
        let query = format!(
            "INSERT INTO {} ({}) VALUES ({}){}",
            ident(table),
            columns.join(", "),
            values.join(", "),
            returning.as_ref().map(|r| format!(" RETURNING {}", r)).unwrap_or_default()
        );

        tracing::debug!("Executing CREATE query: {}", query);

        let inserted = if returning.is_some() {
            binds.query(&query).fetch_one(&mut *db).await.map(Some)
        } else {
            binds.query(&query).execute(&mut *db).await.map(|_| None)
        };
        match inserted {
            Ok(row) => {
                let after_state = row.map(|r| row_to_json(&r, &[])).unwrap_or_else(|| json!({}));
                // For create, before_state is None (record didn't exist)
                ExecutionResult::mutation_success(
                    json!({
                        "data": after_state.clone(),
                        "message": "Record created successfully"
                    }),
                    &query,
                    None, // No before state for CREATE
                    Some(after_state),
                )
            }
            Err(e) => ExecutionResult::error(format!("Database error: {}", e)),
        }
    }

    /// Execute an UPDATE operation.
    pub(super) async fn execute_update(
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

        // Cannot execute UPDATE without a primary key
        if pk_columns.is_empty() {
            return ExecutionResult::error(format!(
                "Cannot update record in table '{}': no primary key defined in schema",
                table
            ));
        }

        let tenant_column = self.tenant_column_for_table(table);

        let keys = match self.key_values(table, arguments) {
            Ok(keys) => keys,
            Err(e) => return ExecutionResult::error(e),
        };

        if options.dry_run {
            return ExecutionResult::dry_run(DryRunResult {
                dry_run: true,
                would_affect: json!({
                    table: { "update": 1 }
                }),
                preview: Some(json!({
                    "operation": "UPDATE",
                    "table": table,
                    "pk_columns": pk_columns,
                    "changes": arguments,
                    "tenant_id": context.tenant_id
                })),
            });
        }


        // Get updatable columns for this table from role definition
        let updatable_columns = self.role.tables.get(table).map(|p| &p.updatable);

        // Build UPDATE statement from arguments (excluding primary key columns)
        let empty_map = serde_json::Map::new();
        let obj = arguments.as_object().unwrap_or(&empty_map);
        let mut set_clauses = Vec::new();
        let mut binds = Binds::default();
        let mut rejected_columns = Vec::new();

        // a foreign key the update sets is checked like on create; its verification inputs are not columns
        let fk_columns: Vec<(String, cori_store_core::config::ForeignKeyConstraint)> = updatable_columns
            .and_then(|u| u.as_map())
            .map(|m| m.iter().filter_map(|(c, k)| k.foreign_key.clone().map(|f| (c.clone(), f))).filter(|(c, _)| obj.contains_key(c)).collect())
            .unwrap_or_default();
        let verify_keys = match self.check_foreign_keys(&mut *db, table, fk_columns, obj, &context.tenant_id).await {
            Ok(keys) => keys,
            Err(refused) => return ExecutionResult::error(refused),
        };

        for (key, value) in obj {
            // Skip primary key columns and foreign key verification inputs
            if pk_columns.contains(key) || verify_keys.contains(key) {
                continue;
            }
            // Validate column name (alphanumeric and underscore only)
            if !key.chars().all(|c| c.is_alphanumeric() || c == '_') {
                continue;
            }

            // Check if column is updatable according to role permissions
            let is_updatable = match updatable_columns {
                Some(cols) => cols.contains(key),
                None => false, // If no table permissions, default to not updatable
            };

            if !is_updatable {
                rejected_columns.push(key.clone());
                continue; // Skip non-updatable columns
            }

            // Check only_when constraint from role definition for restrict_to pattern
            if let Some(cols) = updatable_columns
                && let Some(constraints) = cols.get_constraints(key) {
                    // Check if there's a simple new.<col>: [values] restriction
                    if let Some(only_when) = &constraints.only_when
                        && let Some(allowed_values) = only_when.get_new_value_restriction(key)
                            && !allowed_values.contains(value) {
                                return ExecutionResult::error(format!(
                                    "Value '{}' for column '{}' not in allowed values: {:?}",
                                    value, key, allowed_values
                                ));
                            }
                }

            // objects and arrays are bound as their JSON text, for json/jsonb columns
            set_clauses.push(self.eq(&mut binds, table, key, value));
        }

        // Log rejected columns for audit/debugging
        if !rejected_columns.is_empty() {
            tracing::warn!(
                "UPDATE on {} rejected non-updatable columns: {:?}",
                table,
                rejected_columns
            );
        }

        if set_clauses.is_empty() {
            return ExecutionResult::error(format!(
                "No updatable fields provided. Rejected columns: {:?}",
                rejected_columns
            ));
        }

        let pk_conditions = self.row_conditions(&mut binds, table, &keys, &context.tenant_id);

        // Capture before state for audit (using first PK value)
        let before_state = if let Some((_, pk_val)) = keys.first() {
            self.fetch_row_snapshot(&mut *db, table, pk_val, &context.tenant_id)
                .await
        } else {
            None
        };

        // RETURNING only what the role reads (the primary key at least, to tell a missing row apart)
        let returning = self.returning_list(table).unwrap_or_else(|| ident_list(&pk_columns));
        let query = format!(
            "UPDATE {} SET {} WHERE {} RETURNING {}",
            ident(table),
            set_clauses.join(", "),
            pk_conditions.join(" AND "),
            returning
        );

        tracing::debug!("Executing UPDATE query: {}", query);

        match binds.query(&query).fetch_optional(&mut *db).await {
            Ok(Some(row)) => {
                let after_state = row_to_json(&row, &[]);
                ExecutionResult::mutation_success(
                    json!({
                        "data": after_state.clone(),
                        "message": "Record updated successfully"
                    }),
                    &query,
                    before_state,
                    Some(after_state),
                )
            }
            Ok(None) => {
                // Provide more specific error message
                let pk_description = describe_keys(&keys);
                if tenant_column.is_some() {
                    ExecutionResult::error(format!(
                        "Record not found: no {} with {} exists for tenant '{}'. \
                        The record may not exist or may belong to a different tenant.",
                        table, pk_description, context.tenant_id
                    ))
                } else {
                    ExecutionResult::error(format!(
                        "Record not found: no {} with {} exists.",
                        table, pk_description
                    ))
                }
            }
            Err(e) => ExecutionResult::error(format!("Database error: {}", e)),
        }
    }

    /// Execute a DELETE operation.
    pub(super) async fn execute_delete(
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

        // Cannot execute DELETE without a primary key
        if pk_columns.is_empty() {
            return ExecutionResult::error(format!(
                "Cannot delete record from table '{}': no primary key defined in schema",
                table
            ));
        }

        let tenant_column = self.tenant_column_for_table(table);

        let keys = match self.key_values(table, arguments) {
            Ok(keys) => keys,
            Err(e) => return ExecutionResult::error(e),
        };

        if options.dry_run {
            return ExecutionResult::dry_run(DryRunResult {
                dry_run: true,
                would_affect: json!({
                    table: { "delete": 1 }
                }),
                preview: Some(json!({
                    "operation": "DELETE",
                    "table": table,
                    "pk_columns": pk_columns,
                    "tenant_id": context.tenant_id
                })),
            });
        }


        let mut binds = Binds::default();
        let pk_conditions = self.row_conditions(&mut binds, table, &keys, &context.tenant_id);

        // Capture before state for audit (using first PK value)
        let before_state = if let Some((_, pk_val)) = keys.first() {
            self.fetch_row_snapshot(&mut *db, table, pk_val, &context.tenant_id)
                .await
        } else {
            None
        };

        // Build RETURNING clause with all PK columns
        let returning_cols = ident_list(&pk_columns);

        // `deletable: { soft_delete: true }` marks the row deleted (rules.yaml `soft_delete`) instead of removing it
        let soft = self.role.tables.get(table).map(|p| p.deletable.is_soft_delete()).unwrap_or(false);
        let query = if soft {
            let Some(sd) = self.rules.as_ref().and_then(|r| r.get_table_rules(table)).and_then(|t| t.soft_delete.as_ref()) else {
                return ExecutionResult::error(format!(
                    "Role '{}' soft-deletes from '{}', but rules.yaml declares no soft_delete column for it",
                    self.role.name, table
                ));
            };
            format!(
                "UPDATE {} SET {} = {} WHERE {} RETURNING {}",
                ident(table),
                ident(&sd.column),
                sd.deleted_sql(self.column_type(table, &sd.column).as_deref()),
                pk_conditions.join(" AND "),
                returning_cols
            )
        } else {
            format!(
                "DELETE FROM {} WHERE {} RETURNING {}",
                ident(table),
                pk_conditions.join(" AND "),
                returning_cols
            )
        };

        tracing::debug!("Executing DELETE query: {}", query);

        // Under database enforcement the data role cannot write the soft delete column (its rows would stop matching
        // its own visibility policy): the applier role marks the row, then the caller's role is restored.
        let previous = if soft {
            match self.enter_applier(&mut *db).await {
                Ok(p) => p,
                Err(e) => return ExecutionResult::error(e),
            }
        } else {
            None
        };
        let deleted = binds.query(&query).fetch_optional(&mut *db).await;
        if let Err(e) = self.leave_applier(&mut *db, previous).await {
            return ExecutionResult::error(e);
        }

        match deleted {
            Ok(Some(row)) => {
                // Build response with all PK values
                let deleted_keys = row_to_json(&row, &pk_columns);
                // For delete, after_state is null (record is gone)
                ExecutionResult::mutation_success(
                    json!({
                        "message": "Record deleted successfully",
                        "deleted": deleted_keys
                    }),
                    &query,
                    before_state,
                    None, // Record no longer exists
                )
            }
            Ok(None) => {
                let pk_description = describe_keys(&keys);
                if tenant_column.is_some() {
                    ExecutionResult::error(format!(
                        "Record not found: no {} with {} exists for tenant '{}'. \
                        The record may not exist or may belong to a different tenant.",
                        table, pk_description, context.tenant_id
                    ))
                } else {
                    ExecutionResult::error(format!(
                        "Record not found: no {} with {} exists.",
                        table, pk_description
                    ))
                }
            }
            Err(e) => ExecutionResult::error(format!("Database error: {}", e)),
        }
    }
}
