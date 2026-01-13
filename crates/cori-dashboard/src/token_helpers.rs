//! Token helper utilities.

use cori_biscuit::claims::{ColumnConstraints, RoleClaims, TablePermissions as ClaimTablePermissions};
use cori_core::config::role_definition::{
    CreatableColumns, ReadableConfig, RoleDefinition, UpdatableColumns,
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
                ReadableConfig::All(_) => vec!["*".to_string()],
                ReadableConfig::List(cols) => cols.clone(),
                ReadableConfig::Config(cfg) => {
                    if cfg.columns.is_all() {
                        vec!["*".to_string()]
                    } else {
                        cfg.columns.as_list().map(|s| s.to_vec()).unwrap_or_default()
                    }
                }
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
                    // Extract allowed values from only_when if it's a simple new.<col>: [values] pattern
                    let allowed_values: Option<Vec<String>> = constraints.only_when.as_ref()
                        .and_then(|ow| ow.get_new_value_restriction(col_name))
                        .map(|v| v.iter().filter_map(|val| val.as_str().map(String::from)).collect::<Vec<_>>());
                    
                    let claim_constraints = ColumnConstraints {
                        allowed_values,
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
        max_rows_per_query: None, // Per-table max_per_page is now in readable config
        max_affected_rows: None,
        blocked_operations: Vec::new(), // Not used in new model
        description: role.description.clone(),
        minted_at: Some(chrono::Utc::now()),
    }
}
