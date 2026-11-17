// SPDX-License-Identifier: Apache-2.0
//! The checks around a call: the tool's argument schema, the foreign keys a write sets, the current row an
//! `only_when` or an approval reads, and the applier role those reads run under.

use super::sql::{Binds, ident, row_to_json};
use super::{ExecutionContext, ToolExecutor};
use cori_store_core::protocol::ToolDefinition;
use serde_json::Value;
use sqlx::{AssertSqlSafe, PgConnection};

impl ToolExecutor {
    /// Under database enforcement, switches the connection to the applier role for a read or write of Cori's own
    /// (current row for `only_when`, approval snapshots, foreign key checks, soft deletes): the data role is not
    /// granted what those read. Returns the role to restore with `leave_applier`.
    pub(super) async fn enter_applier(&self, db: &mut PgConnection) -> Result<Option<String>, String> {
        let applier = &self.applier;
        let (current,): (String,) = sqlx::query_as("SELECT current_user::text")
            .fetch_one(&mut *db)
            .await
            .map_err(|e| format!("Database error: {}", e))?;
        if &current == applier {
            return Ok(None);
        }
        sqlx::query(AssertSqlSafe(format!("SET LOCAL ROLE {}", ident(applier))))
            .execute(&mut *db)
            .await
            .map_err(|e| format!("Database error: {}", e))?;
        Ok(Some(current))
    }

    /// Restores the role `enter_applier` switched away from.
    pub(super) async fn leave_applier(&self, db: &mut PgConnection, previous: Option<String>) -> Result<(), String> {
        if let Some(role) = previous {
            sqlx::query(AssertSqlSafe(format!("SET LOCAL ROLE {}", ident(&role))))
                .execute(&mut *db)
                .await
                .map_err(|e| format!("Database error: {}", e))?;
        }
        Ok(())
    }

    /// Checks the foreign keys a create or an update sets: the referenced row exists for the caller's tenant and matches
    /// the `verify_with` values given (inputs named by `cori_store_core::fk_verify_key`). Runs as the applier: the check
    /// reads the referenced table's key and verify_with columns, which the caller's role may not read. A key the schema
    /// does not know is left to the database's constraints. Returns the verification inputs, which are not columns, or
    /// the refusal.
    pub(super) async fn check_foreign_keys(
        &self,
        db: &mut PgConnection,
        table: &str,
        fks: Vec<(String, cori_store_core::config::ForeignKeyConstraint)>,
        args: &serde_json::Map<String, Value>,
        tenant_id: &str,
    ) -> Result<Vec<String>, String> {
        let mut verify_keys = Vec::new();
        let mut checks = Vec::new();
        for (column, fk) in &fks {
            verify_keys.extend(fk.verify_with.iter().map(|v| cori_store_core::fk_verify_key(column, v)));
            // no value, or null: no reference to check
            let Some(value) = args.get(column).filter(|v| !v.is_null()) else { continue };
            let Some((ref_table, ref_column)) = self.get_fk_reference(table, column) else { continue };
            let mut binds = Binds::default();
            let mut conditions = vec![self.eq(&mut binds, &ref_table, &ref_column, value)];
            if let Some(tc) = self.tenant_column_for_table(&ref_table) {
                conditions.push(self.eq(&mut binds, &ref_table, &tc, &Value::String(tenant_id.to_string())));
            }
            for v in &fk.verify_with {
                if let Some(given) = args.get(&cori_store_core::fk_verify_key(column, v)) {
                    conditions.push(self.eq(&mut binds, &ref_table, v, given));
                }
            }
            let hint = if fk.verify_with.is_empty() { String::new() } else { format!(" with {} matching", fk.verify_with.join(", ")) };
            let refused = format!("Referenced {} not found{hint} (or belongs to a different tenant)", column.strip_suffix("_id").unwrap_or(column));
            checks.push((format!("SELECT 1 FROM {} WHERE {}", ident(&ref_table), conditions.join(" AND ")), binds, refused));
        }
        if checks.is_empty() {
            return Ok(verify_keys);
        }
        let previous = self.enter_applier(db).await?;
        let mut outcome = Ok(());
        for (sql, binds, refused) in checks {
            tracing::debug!("Validating FK: {}", sql);
            match binds.query(&sql).fetch_optional(&mut *db).await {
                Ok(Some(_)) => {}
                Ok(None) => { outcome = Err(refused); break; }
                Err(e) => { outcome = Err(format!("Failed to validate foreign key: {e}")); break; }
            }
        }
        self.leave_applier(db, previous).await?;
        outcome.map(|()| verify_keys)
    }

    /// Fetch the current row from database for UPDATE validation.
    pub(super) async fn fetch_current_row(
        &self,
        db: &mut PgConnection,
        table: &str,
        arguments: &Value,
        context: &ExecutionContext,
    ) -> Result<Option<Value>, String> {
        if self.get_primary_key_columns(table).is_empty() {
            return Err(format!("No primary key defined for table '{}'", table));
        }
        let keys = self.key_values(table, arguments)?;
        let mut binds = Binds::default();
        let conditions = self.row_conditions(&mut binds, table, &keys, &context.tenant_id);

        // the columns only_when reads besides the readable ones: the data role is granted them (policy compiler)
        let mut extra: Vec<String> = keys.iter().map(|(c, _)| c.clone()).collect();
        extra.extend(self.role.condition_columns(table));
        let query = format!("SELECT {} FROM {} WHERE {}", self.select_list(table, &extra), ident(table), conditions.join(" AND "));

        tracing::debug!("Fetching current row for validation: {}", query);

        let previous = self.enter_applier(&mut *db).await?;
        let fetched = binds.query(&query).fetch_optional(&mut *db).await;
        self.leave_applier(&mut *db, previous).await?;
        match fetched {
            Ok(Some(row)) => {
                let data = row_to_json(&row, &[]);
                Ok(Some(data))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(format!("Database error: {}", e)),
        }
    }

    /// Fetch current row values by ID for snapshot validation.
    /// Used when creating approval requests for updates/deletes.
    pub(super) async fn fetch_row_snapshot(
        &self,
        db: &mut PgConnection,
        table: &str,
        pk_value: &Value,
        tenant_id: &str,
    ) -> Option<Value> {
        let pk_column = self.get_primary_key_columns(table).into_iter().next()?;
        let mut binds = Binds::default();
        let conditions = self.row_conditions(&mut binds, table, &[(pk_column, pk_value.clone())], tenant_id);
        let query = format!("SELECT {} FROM {} WHERE {}", self.select_list(table, &[]), ident(table), conditions.join(" AND "));

        let previous = self.enter_applier(&mut *db).await.ok()?;
        let fetched = binds.query(&query).fetch_optional(&mut *db).await;
        self.leave_applier(&mut *db, previous).await.ok()?;
        match fetched {
            Ok(Some(row)) => Some(row_to_json(&row, &[])),
            _ => None,
        }
    }

    /// Validate arguments against tool constraints.
    pub(super) fn validate_arguments(&self, tool: &ToolDefinition, arguments: &Value) -> Result<(), String> {
        let schema = &tool.input_schema;

        // Check required fields
        if let Some(required) = schema["required"].as_array() {
            for req in required {
                if let Some(field) = req.as_str()
                    && arguments.get(field).is_none() {
                        return Err(format!("Missing required field: {}", field));
                    }
            }
        }

        // Check enum constraints
        if let Some(props) = schema["properties"].as_object() {
            for (field, prop_schema) in props {
                if let Some(value) = arguments.get(field) {
                    // Check enum
                    if let Some(allowed) = prop_schema["enum"].as_array() {
                        let allowed_values: Vec<&Value> = allowed.iter().collect();
                        if !allowed_values.contains(&value) {
                            return Err(format!(
                                "Invalid value for '{}': {:?}. Allowed: {:?}",
                                field, value, allowed
                            ));
                        }
                    }

                    // Check type (null clears a column: the database refuses it on a NOT NULL one)
                    if let Some(expected_type) = prop_schema["type"].as_str()
                        && !value.is_null()
                        && !self.check_type(value, expected_type) {
                            return Err(format!(
                                "Invalid type for '{}': expected {}, got {:?}",
                                field, expected_type, value
                            ));
                        }

                    // Check min/max
                    if let Some(min) = prop_schema["minimum"].as_f64()
                        && let Some(v) = value.as_f64()
                            && v < min {
                                return Err(format!(
                                    "Value for '{}' must be at least {}",
                                    field, min
                                ));
                            }
                    if let Some(max) = prop_schema["maximum"].as_f64()
                        && let Some(v) = value.as_f64()
                            && v > max {
                                return Err(format!(
                                    "Value for '{}' must be at most {}",
                                    field, max
                                ));
                            }

                    // Check pattern
                    if let Some(pattern) = prop_schema["pattern"].as_str()
                        && let Some(s) = value.as_str()
                            && let Ok(re) = regex::Regex::new(pattern)
                                && !re.is_match(s) {
                                    return Err(format!(
                                        "Value for '{}' does not match pattern: {}",
                                        field, pattern
                                    ));
                                }
                }
            }
        }

        Ok(())
    }

    /// Check if a value matches an expected type.
    pub(super) fn check_type(&self, value: &Value, expected: &str) -> bool {
        match expected {
            "string" => value.is_string(),
            "integer" => value.is_i64() || value.is_u64(),
            "number" => value.is_number(),
            "boolean" => value.is_boolean(),
            "object" => value.is_object(),
            "array" => value.is_array(),
            "null" => value.is_null(),
            _ => true,
        }
    }
}
