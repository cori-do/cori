// SPDX-License-Identifier: Apache-2.0
//! What a role and the rules allow on a table: its tenant column, the columns the role reads, its primary key and
//! foreign keys, and the filter hiding soft-deleted rows.

use super::sql::{ident, ident_list};
use super::ToolExecutor;
use cori_store_core::config::rules_definition::TenantConfig;

impl ToolExecutor {
    /// Native type of a column, from the schema.
    pub(super) fn column_type(&self, table: &str, column: &str) -> Option<String> {
        self.schema
            .as_ref()
            .and_then(|s| s.get_table(table))
            .and_then(|t| t.get_column(column))
            .map(|c| c.data_type())
    }

    /// Predicate hiding soft-deleted rows (`soft_delete` in rules.yaml), if the table has one.
    pub(super) fn active_condition(&self, table: &str) -> Option<String> {
        let sd = self.rules.as_ref()?.get_table_rules(table)?.soft_delete.as_ref()?;
        Some(sd.active_predicate(&ident(&sd.column), self.column_type(table, &sd.column).as_deref()))
    }

    /// Select list of the columns the role reads (`*` when it reads every column), plus `extra` columns.
    pub(super) fn select_list(&self, table: &str, extra: &[String]) -> String {
        let mut cols = self.get_readable_columns(table);
        if cols.is_empty() && self.role.can_read(table) {
            return "*".to_string();
        }
        for c in extra {
            if !cols.contains(c) {
                cols.push(c.clone());
            }
        }
        ident_list(&cols)
    }

    /// RETURNING list of a write: the readable columns, else the primary key; None when neither exists.
    pub(super) fn returning_list(&self, table: &str) -> Option<String> {
        let list = self.select_list(table, &[]);
        if !list.is_empty() {
            return Some(list);
        }
        let pk = self.get_primary_key_columns(table);
        if pk.is_empty() { None } else { Some(ident_list(&pk)) }
    }

    pub(super) fn tenant_column_for_table(&self, table: &str) -> Option<String> {
        // a role with `scope: all` reaches every row: no tenant filter for it
        if !self.role.row_scope(table).is_own() {
            return None;
        }
        self.direct_tenant_column(table)
    }

    /// The column a create sets to the caller's id. For a role with the default scope, the tenant column (a value in
    /// the input is ignored); for a role with `scope: all`, the tenant column too unless the role names its owner
    /// itself (the column is `creatable` and the input has a value): a row it files is otherwise its own.
    pub(super) fn owner_column_for_create(&self, table: &str, arguments: &serde_json::Map<String, serde_json::Value>, tenant_id: &str) -> Option<String> {
        if let Some(tc) = self.tenant_column_for_table(table) {
            return Some(tc);
        }
        let tc = self.direct_tenant_column(table)?;
        let named = self.role.can_create_column(table, &tc) && arguments.get(&tc).is_some_and(|v| !v.is_null());
        let known = tenant_id != "unknown" && !tenant_id.trim().is_empty();
        (!named && known).then_some(tc)
    }

    /// The table's own tenant column (`tenant: <column>`), whatever the role's scope; None for a global table or an
    /// inherited tenancy.
    fn direct_tenant_column(&self, table: &str) -> Option<String> {
        let rules = self.rules.as_ref()?;
        let table_rules = rules.get_table_rules(table)?;

        // Check if global table
        if table_rules.global.unwrap_or(false) {
            return None;
        }

        // Get tenant config
        match &table_rules.tenant {
            Some(TenantConfig::Direct(column)) => Some(column.clone()),
            Some(TenantConfig::Inherited(_)) => {
                // For inherited tenancy, we need to JOIN through FK
                // For now, return None (handled separately in query building)
                None
            }
            None => None,
        }
    }

    pub(super) fn validate_tenant_for_table(&self, table: &str, tenant_id: &str) -> Result<(), String> {
        let tenant_column = self.tenant_column_for_table(table);
        if tenant_column.is_none() {
            // Global / unscoped table or inherited tenancy: tenant not required at this level.
            return Ok(());
        }

        if tenant_id == "unknown" || tenant_id.trim().is_empty() {
            return Err(format!(
                "Missing tenant id for tenant-scoped table '{}'. Provide a tenant-scoped Biscuit token (with a tenant(...) attenuation block) via --token or CORI_TOKEN.",
                table
            ));
        }

        // No type validation here - database will validate
        Ok(())
    }

    /// Get the primary key columns for a table from schema.
    /// Returns empty vector if no primary key is defined in the schema.
    pub(super) fn get_primary_key_columns(&self, table: &str) -> Vec<String> {
        if let Some(schema) = &self.schema
            && let Some(table_schema) = schema.get_table(table) {
                return table_schema.primary_key.clone();
            }
        // No schema or table not found - return empty (no PK)
        Vec::new()
    }

    /// The table and column a foreign key column references, from the schema. None if the schema has no such key.
    pub(super) fn get_fk_reference(&self, table: &str, column: &str) -> Option<(String, String)> {
        let (t, c) = self.schema.as_ref()?.get_table(table)?.foreign_key_reference(column)?;
        Some((t.to_string(), c.to_string()))
    }

    /// Whether the role's only_when conditions on the table read the current row: an `old.<col>` key, or an `old.<col>`
    /// reference inside a comparison (`new.quantity: { lower_than: old.quantity }`, `starts_with: old.notes`).
    pub(super) fn role_has_old_constraints(&self, table: &str) -> bool {
        !self.role.condition_columns(table).is_empty()
    }

    /// Columns a list may filter on: the role's readable columns (all schema columns when the role reads every column).
    pub(super) fn filterable_columns(&self, table: &str) -> Vec<String> {
        let listed = self.get_readable_columns(table);
        if !listed.is_empty() {
            return listed;
        }
        if !self.role.can_read(table) {
            return Vec::new();
        }
        self.schema
            .as_ref()
            .and_then(|s| s.get_table(table))
            .map(|t| t.columns.iter().map(|c| c.name.clone()).collect())
            .unwrap_or_default()
    }

    /// Get readable columns for a table from role.
    pub(super) fn get_readable_columns(&self, table: &str) -> Vec<String> {
        self.role
            .get_readable_columns(table)
            .and_then(|cols| cols.as_list())
            .map(|cols| cols.to_vec())
            .unwrap_or_default()
    }
}
