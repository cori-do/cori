//! Token helper utilities.

use cori_biscuit::claims::{ColumnConstraints, RoleClaims, TablePermissions as ClaimTablePermissions};
use cori_core::config::role_definition::{
    ColumnList, CreatableColumns, RoleDefinition, UpdatableColumns,
};
use std::collections::HashMap;

/// Helper to convert Value vec to String vec
fn values_to_strings(values: &Option<Vec<serde_json::Value>>) -> Option<Vec<String>> {
    values.as_ref().map(|vals| {
        vals.iter()
            .filter_map(|v| v.as_str().map(String::from).or_else(|| Some(v.to_string())))
            .collect()
    })
}

/// Convert RoleDefinition to RoleClaims for token minting.
pub fn role_definition_to_claims(role: &RoleDefinition) -> RoleClaims {
    let tables: HashMap<String, ClaimTablePermissions> = role
        .tables
        .iter()
        .map(|(table_name, table_perms)| {
            // Readable columns
            let readable = match &table_perms.readable {
                ColumnList::All(_) => vec!["*".to_string()],
                ColumnList::List(cols) => cols.clone(),
            };

            // Editable = creatable + updatable columns with constraints
            let mut editable: HashMap<String, ColumnConstraints> = HashMap::new();

            // Add creatable columns
            if let CreatableColumns::Map(cols) = &table_perms.creatable {
                for (col_name, constraints) in cols {
                    let claim_constraints = ColumnConstraints {
                        allowed_values: values_to_strings(&constraints.restrict_to),
                        pattern: None, // Not in current creatable constraints
                        min: None,
                        max: None,
                        requires_approval: constraints.requires_approval.is_some(),
                    };
                    editable.insert(col_name.clone(), claim_constraints);
                }
            }

            // Add/merge updatable columns
            if let UpdatableColumns::Map(cols) = &table_perms.updatable {
                for (col_name, constraints) in cols {
                    let claim_constraints = ColumnConstraints {
                        allowed_values: values_to_strings(&constraints.restrict_to),
                        pattern: None, // Not in current updatable constraints
                        min: None,
                        max: None,
                        requires_approval: constraints.requires_approval.is_some(),
                    };
                    // Merge with existing (from creatable) or insert
                    editable.insert(col_name.clone(), claim_constraints);
                }
            }

            let claim_perms = ClaimTablePermissions {
                readable,
                editable,
                tenant_column: None, // Tenant column comes from rules, not role definition
            };

            (table_name.clone(), claim_perms)
        })
        .collect();

    RoleClaims {
        role: role.name.clone(),
        tables,
        blocked_tables: role.blocked_tables.clone(),
        max_rows_per_query: role.max_rows_per_query,
        max_affected_rows: role.max_affected_rows,
        blocked_operations: Vec::new(), // Not used in new model
        description: role.description.clone(),
        minted_at: Some(chrono::Utc::now()),
    }
}
