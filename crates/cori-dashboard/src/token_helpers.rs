//! Token helper utilities.

use cori_biscuit::claims::{RoleClaims, TablePermissions as ClaimTablePermissions};
use cori_core::config::role::{RoleConfig, ReadableColumns, EditableColumns};
use std::collections::HashMap;

/// Convert RoleConfig to RoleClaims for token minting.
pub fn role_config_to_claims(role: &RoleConfig) -> RoleClaims {
    let tables: HashMap<String, ClaimTablePermissions> = role.tables.iter().map(|(table_name, table_perms)| {
        let readable = match &table_perms.readable {
            ReadableColumns::All(_) => vec!["*".to_string()],
            ReadableColumns::List(cols) => cols.clone(),
        };
        
        let editable = match &table_perms.editable {
            EditableColumns::All(_) => HashMap::new(), // "*" means all columns
            EditableColumns::Map(cols) => {
                cols.iter().map(|(col_name, constraints)| {
                    let claim_constraints = cori_biscuit::claims::ColumnConstraints {
                        allowed_values: constraints.allowed_values.clone(),
                        pattern: constraints.pattern.clone(),
                        min: constraints.min,
                        max: constraints.max,
                        requires_approval: constraints.requires_approval,
                    };
                    (col_name.clone(), claim_constraints)
                }).collect()
            }
        };
        
        let claim_perms = ClaimTablePermissions {
            readable,
            editable,
            tenant_column: table_perms.tenant_column.clone(),
        };
        
        (table_name.clone(), claim_perms)
    }).collect();
    
    RoleClaims {
        role: role.name.clone(),
        tables,
        blocked_tables: role.blocked_tables.clone(),
        max_rows_per_query: role.max_rows_per_query,
        max_affected_rows: role.max_affected_rows,
        blocked_operations: role.blocked_operations.clone(),
        description: role.description.clone(),
        minted_at: Some(chrono::Utc::now()),
    }
}

/// Extract operation names from a role config for a specific table.
pub fn get_table_operations(role: &RoleConfig, table_name: &str) -> Vec<String> {
    role.tables.get(table_name)
        .and_then(|table_perms| table_perms.operations.as_ref())
        .map(|ops| {
            ops.iter().map(|op| format!("{:?}", op).to_uppercase()).collect()
        })
        .unwrap_or_default()
}

/// Format permissions for display.
pub fn format_permissions(role: &RoleConfig) -> String {
    role.tables.iter()
        .map(|(table, _tp)| {
            let ops = get_table_operations(role, table);
            if ops.is_empty() {
                format!("{}: READ", table)
            } else {
                format!("{}: {}", table, ops.join(", "))
            }
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// Check if role has any write permissions (create, update, delete).
pub fn has_write_permissions(role: &RoleConfig) -> bool {
    use cori_core::config::role::Operation;
    
    role.tables.values().any(|tp| {
        tp.operations.as_ref().map_or(false, |ops| {
            ops.iter().any(|op| matches!(op, Operation::Create | Operation::Update | Operation::Delete))
        })
    })
}

/// Count total tables with permissions.
pub fn count_tables(role: &RoleConfig) -> usize {
    role.tables.len()
}

/// Get all table names from role permissions.
pub fn get_permission_tables(role: &RoleConfig) -> Vec<String> {
    role.tables.keys().cloned().collect()
}
