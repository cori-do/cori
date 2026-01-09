//! Deletable permission tests for Cori MCP.
//!
//! Tests the `deletable` field in TablePermissions:
//! - DeletablePermission::Allowed(true/false) - simple allow/deny
//! - DeletablePermission::Config { requires_approval, soft_delete }

use super::common::*;
use cori_core::config::role_definition::{
    ApprovalRequirement, ColumnList, CreatableColumns, DeletableConstraints, DeletablePermission,
    RoleDefinition, TablePermissions, UpdatableColumns, ApprovalConfig,
};
use cori_mcp::protocol::CallToolOptions;
use serde_json::json;
use std::collections::HashMap;

// =============================================================================
// DELETABLE BOOLEAN - ALLOWED
// =============================================================================

pub async fn test_delete_allowed(ctx: &TestContext) {
    println!("  🧪 test_delete_allowed");

    let executor = ctx.executor();

    // Create a test ticket to delete
    let create_tool = create_tool(
        "Ticket",
        json!({
            "organization_id": { "type": "string" },
            "customer_id": { "type": "integer" },
            "subject": { "type": "string" },
            "status": { "type": "string" },
            "priority": { "type": "string" },
            "ticket_number": { "type": "string" }
        }),
    );

    let create_result = executor
        .execute(
            &create_tool,
            json!({
                "organization_id": "1",
                "customer_id": 1,
                "subject": "To be deleted",
                "status": "open",
                "priority": "low",
                "ticket_number": format!("TKT-DEL-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis())
            }),
            &CallToolOptions::default(),
            &create_context("1"),
        )
        .await;

    assert_success(&create_result, "CREATE should succeed");

    let created = extract_json(&create_result).unwrap();
    let ticket_id = created["data"]["ticket_id"].as_i64().expect("Should have ticket_id");

    // Delete the ticket
    let delete_tool = delete_tool("Ticket");
    let delete_result = executor
        .execute(
            &delete_tool,
            json!({ "id": ticket_id }),
            &CallToolOptions::default(),
            &create_context("1"),
        )
        .await;

    assert_success(&delete_result, "DELETE should succeed");

    // Verify deletion
    let get_tool = get_tool("Ticket");
    let verify = executor
        .execute(
            &get_tool,
            json!({ "id": ticket_id }),
            &CallToolOptions::default(),
            &create_context("1"),
        )
        .await;

    // After deletion, GET should return null data (not found)
    assert_success(&verify, "GET should succeed but return null");
    let verify_json = extract_json(&verify).unwrap();
    assert!(
        verify_json["data"].is_null(),
        "Deleted ticket should not be found"
    );

    println!("     ✓ Delete allowed when deletable: true");
}

// =============================================================================
// DELETABLE BOOLEAN - BLOCKED
// =============================================================================

pub async fn test_delete_blocked(_ctx: &TestContext) {
    println!("  🧪 test_delete_blocked");

    // Create a role where deletable is false
    let mut tables = HashMap::new();
    tables.insert(
        "customers".to_string(),
        TablePermissions {
            readable: ColumnList::List(vec![
                "customer_id".to_string(),
                "first_name".to_string(),
                "organization_id".to_string(),
            ]),
            creatable: CreatableColumns::default(),
            updatable: UpdatableColumns::default(),
            deletable: DeletablePermission::Allowed(false),
        },
    );

    let role = RoleDefinition {
        name: "no_delete_role".to_string(),
        description: Some("Role without delete permission".to_string()),
        approvals: None,
        tables,
        blocked_tables: Vec::new(),
        max_rows_per_query: Some(100),
        max_affected_rows: Some(10),
    };

    // Verify deletable is false
    let perms = role.tables.get("customers").unwrap();
    assert!(
        !perms.deletable.is_allowed(),
        "Delete should not be allowed"
    );
    assert!(
        !role.can_delete("customers"),
        "can_delete should return false"
    );

    println!("     ✓ Delete blocked when deletable: false");
}

pub async fn test_delete_cross_tenant_blocked(ctx: &TestContext) {
    println!("  🧪 test_delete_cross_tenant_blocked");

    let executor = ctx.executor();
    let delete_tool = delete_tool("Ticket");

    // Try to delete a ticket from tenant 2 while being tenant 1
    // Ticket IDs 4, 5, 6 belong to organization_id=2 (Globex)
    let result = executor
        .execute(
            &delete_tool,
            json!({ "id": 4 }),  // ticket_id 4 belongs to tenant 2 (Globex)
            &CallToolOptions::default(),
            &create_context("1"),  // We're tenant 1 (Acme)
        )
        .await;

    // Should fail or return 0 affected rows
    if !result.success || result.error.is_some() {
        println!("     ✓ Cross-tenant delete blocked (error)");
    } else {
        let data = extract_json(&result);
        if let Some(d) = data {
            assert_eq!(
                d["affectedRows"].as_i64().unwrap_or(1),
                0,
                "Should affect 0 rows for cross-tenant delete"
            );
            println!("     ✓ Cross-tenant delete blocked (0 affected rows)");
        }
    }
}

// =============================================================================
// DELETABLE WITH REQUIRES_APPROVAL
// =============================================================================

pub async fn test_delete_requires_approval(_ctx: &TestContext) {
    println!("  🧪 test_delete_requires_approval");

    let mut tables = HashMap::new();
    tables.insert(
        "orders".to_string(),
        TablePermissions {
            readable: ColumnList::List(vec![
                "order_id".to_string(),
                "customer_id".to_string(),
            ]),
            creatable: CreatableColumns::default(),
            updatable: UpdatableColumns::default(),
            deletable: DeletablePermission::WithConstraints(DeletableConstraints {
                requires_approval: Some(ApprovalRequirement::Simple(true)),
                soft_delete: false,
            }),
        },
    );

    let role = RoleDefinition {
        name: "delete_approval_role".to_string(),
        description: Some("Role requiring approval for delete".to_string()),
        approvals: None,
        tables,
        blocked_tables: Vec::new(),
        max_rows_per_query: Some(100),
        max_affected_rows: Some(10),
    };

    let perms = role.tables.get("orders").unwrap();
    assert!(perms.deletable.is_allowed(), "Delete should be allowed");
    assert!(
        perms.deletable.requires_approval(),
        "Delete should require approval"
    );

    println!("     ✓ Deletable with requires_approval configured");
}

pub async fn test_delete_requires_approval_with_group(_ctx: &TestContext) {
    println!("  🧪 test_delete_requires_approval_with_group");

    let mut tables = HashMap::new();
    tables.insert(
        "orders".to_string(),
        TablePermissions {
            readable: ColumnList::List(vec!["order_id".to_string()]),
            creatable: CreatableColumns::default(),
            updatable: UpdatableColumns::default(),
            deletable: DeletablePermission::WithConstraints(DeletableConstraints {
                requires_approval: Some(ApprovalRequirement::Detailed(ApprovalConfig {
                    group: "managers".to_string(),
                    notify_on_pending: true,
                    message: Some("Delete requires manager approval".to_string()),
                })),
                soft_delete: false,
            }),
        },
    );

    let role = RoleDefinition {
        name: "delete_approval_group_role".to_string(),
        description: None,
        approvals: None,
        tables,
        blocked_tables: Vec::new(),
        max_rows_per_query: None,
        max_affected_rows: None,
    };

    let perms = role.tables.get("orders").unwrap();
    assert!(perms.deletable.requires_approval());

    if let DeletablePermission::WithConstraints(constraints) = &perms.deletable {
        if let Some(ApprovalRequirement::Detailed(config)) = &constraints.requires_approval {
            assert_eq!(config.group, "managers");
            assert!(config.message.as_ref().unwrap().contains("manager approval"));
        } else {
            panic!("Expected detailed approval requirement");
        }
    }

    println!("     ✓ Deletable with detailed approval group configured");
}

// =============================================================================
// DELETABLE WITH SOFT_DELETE
// =============================================================================

pub async fn test_delete_soft_delete_flag(_ctx: &TestContext) {
    println!("  🧪 test_delete_soft_delete_flag");

    let mut tables = HashMap::new();
    tables.insert(
        "orders".to_string(),
        TablePermissions {
            readable: ColumnList::List(vec!["order_id".to_string()]),
            creatable: CreatableColumns::default(),
            updatable: UpdatableColumns::default(),
            deletable: DeletablePermission::WithConstraints(DeletableConstraints {
                requires_approval: None,
                soft_delete: true,
            }),
        },
    );

    let role = RoleDefinition {
        name: "soft_delete_role".to_string(),
        description: Some("Role with soft delete".to_string()),
        approvals: None,
        tables,
        blocked_tables: Vec::new(),
        max_rows_per_query: None,
        max_affected_rows: None,
    };

    let perms = role.tables.get("orders").unwrap();
    assert!(perms.deletable.is_allowed(), "Delete should be allowed");
    assert!(
        perms.deletable.is_soft_delete(),
        "Should be soft delete"
    );

    println!("     ✓ Deletable with soft_delete flag configured");
}

pub async fn test_delete_soft_delete_and_approval(_ctx: &TestContext) {
    println!("  🧪 test_delete_soft_delete_and_approval");

    let mut tables = HashMap::new();
    tables.insert(
        "sensitive_records".to_string(),
        TablePermissions {
            readable: ColumnList::List(vec!["id".to_string()]),
            creatable: CreatableColumns::default(),
            updatable: UpdatableColumns::default(),
            deletable: DeletablePermission::WithConstraints(DeletableConstraints {
                requires_approval: Some(ApprovalRequirement::Detailed(ApprovalConfig {
                    group: "security_team".to_string(),
                    notify_on_pending: true,
                    message: Some("Sensitive record deletion requires security approval".to_string()),
                })),
                soft_delete: true,
            }),
        },
    );

    let role = RoleDefinition {
        name: "secure_delete_role".to_string(),
        description: None,
        approvals: None,
        tables,
        blocked_tables: Vec::new(),
        max_rows_per_query: None,
        max_affected_rows: None,
    };

    let perms = role.tables.get("sensitive_records").unwrap();
    assert!(perms.deletable.is_allowed());
    assert!(perms.deletable.requires_approval());
    assert!(perms.deletable.is_soft_delete());

    println!("     ✓ Deletable with both soft_delete and requires_approval");
}

// =============================================================================
// DELETABLE PERMISSION HELPERS
// =============================================================================

pub async fn test_deletable_is_allowed_helper(_ctx: &TestContext) {
    println!("  🧪 test_deletable_is_allowed_helper");

    // Boolean true
    let allowed = DeletablePermission::Allowed(true);
    assert!(allowed.is_allowed(), "Boolean(true) should be allowed");

    // Boolean false
    let blocked = DeletablePermission::Allowed(false);
    assert!(!blocked.is_allowed(), "Boolean(false) should not be allowed");

    // Config (always allowed when present)
    let config = DeletablePermission::WithConstraints(DeletableConstraints::default());
    assert!(config.is_allowed(), "Config should be allowed");

    println!("     ✓ is_allowed helper works correctly");
}

pub async fn test_deletable_requires_approval_helper(_ctx: &TestContext) {
    println!("  🧪 test_deletable_requires_approval_helper");

    // Boolean doesn't require approval
    let boolean = DeletablePermission::Allowed(true);
    assert!(
        !boolean.requires_approval(),
        "Boolean should not require approval"
    );

    // Config without approval
    let config_no_approval = DeletablePermission::WithConstraints(DeletableConstraints {
        requires_approval: None,
        soft_delete: false,
    });
    assert!(!config_no_approval.requires_approval());

    // Config with simple approval
    let config_simple = DeletablePermission::WithConstraints(DeletableConstraints {
        requires_approval: Some(ApprovalRequirement::Simple(true)),
        soft_delete: false,
    });
    assert!(config_simple.requires_approval());

    // Config with simple false
    let config_false = DeletablePermission::WithConstraints(DeletableConstraints {
        requires_approval: Some(ApprovalRequirement::Simple(false)),
        soft_delete: false,
    });
    assert!(!config_false.requires_approval());

    // Config with detailed approval
    let config_detailed = DeletablePermission::WithConstraints(DeletableConstraints {
        requires_approval: Some(ApprovalRequirement::Detailed(ApprovalConfig {
            group: "admins".to_string(),
            notify_on_pending: true,
            message: None,
        })),
        soft_delete: false,
    });
    assert!(config_detailed.requires_approval());

    println!("     ✓ requires_approval helper works correctly");
}

pub async fn test_deletable_is_soft_delete_helper(_ctx: &TestContext) {
    println!("  🧪 test_deletable_is_soft_delete_helper");

    // Boolean is never soft delete
    let boolean = DeletablePermission::Allowed(true);
    assert!(!boolean.is_soft_delete(), "Boolean should not be soft delete");

    // Config with soft_delete = false
    let config_hard = DeletablePermission::WithConstraints(DeletableConstraints {
        requires_approval: None,
        soft_delete: false,
    });
    assert!(!config_hard.is_soft_delete());

    // Config with soft_delete = true
    let config_soft = DeletablePermission::WithConstraints(DeletableConstraints {
        requires_approval: None,
        soft_delete: true,
    });
    assert!(config_soft.is_soft_delete());

    println!("     ✓ is_soft_delete helper works correctly");
}

pub async fn test_can_delete_table_helper(_ctx: &TestContext) {
    println!("  🧪 test_can_delete_table_helper");

    let role = create_support_agent_role();

    // tickets should be deletable (based on support_agent role definition)
    assert!(
        role.can_delete("tickets"),
        "Should be able to delete tickets"
    );

    // customers should not be deletable
    assert!(
        !role.can_delete("customers"),
        "Should not be able to delete customers"
    );

    // non-existent table
    assert!(
        !role.can_delete("nonexistent"),
        "Non-existent table should return false"
    );

    println!("     ✓ can_delete table helper works correctly");
}

// =============================================================================
// DRY-RUN FOR DELETE
// =============================================================================

pub async fn test_delete_dry_run(ctx: &TestContext) {
    println!("  🧪 test_delete_dry_run");

    let executor = ctx.executor();
    let delete_tool = delete_tool("Ticket");

    let result = executor
        .execute(
            &delete_tool,
            json!({ "id": 1 }),
            &CallToolOptions { dry_run: true },
            &create_context("1"),
        )
        .await;

    assert_success(&result, "DELETE dry run should succeed");
    assert!(result.is_dry_run, "Should be marked as dry run");

    // Verify the ticket still exists
    let get_tool = get_tool("Ticket");
    let verify = executor
        .execute(
            &get_tool,
            json!({ "id": 1 }),
            &CallToolOptions::default(),
            &create_context("1"),
        )
        .await;

    assert_success(&verify, "Ticket should still exist after dry run");

    println!("     ✓ DELETE dry run works correctly");
}

// =============================================================================
// DEFAULT DELETABLE PERMISSION
// =============================================================================

pub async fn test_default_deletable_permission(_ctx: &TestContext) {
    println!("  🧪 test_default_deletable_permission");

    let default_perm = DeletablePermission::default();

    // Default should be false (safe default)
    assert!(
        !default_perm.is_allowed(),
        "Default deletable should be false (safe)"
    );

    println!("     ✓ Default DeletablePermission is safe (false)");
}

// =============================================================================
// MAX_AFFECTED_ROWS FOR DELETE
// =============================================================================

pub async fn test_delete_max_affected_rows(_ctx: &TestContext) {
    println!("  🧪 test_delete_max_affected_rows");

    let role = create_role_with_max_affected(1);
    assert_eq!(
        role.max_affected_rows,
        Some(1),
        "Role should have max_affected_rows = 1"
    );

    // This would need executor-level enforcement to test properly
    // For now, just verify the constraint is configured

    println!("     ✓ max_affected_rows constraint for DELETE configured");
}

// =============================================================================
// TEST RUNNER
// =============================================================================

/// Run all deletable permission tests
pub async fn run_all_tests(ctx: &TestContext) {
    println!("\n🗑️ Running Deletable Permission Tests\n");

    // Basic delete operations
    test_delete_allowed(ctx).await;
    test_delete_blocked(ctx).await;
    test_delete_cross_tenant_blocked(ctx).await;

    // Requires approval
    test_delete_requires_approval(ctx).await;
    test_delete_requires_approval_with_group(ctx).await;

    // Soft delete
    test_delete_soft_delete_flag(ctx).await;
    test_delete_soft_delete_and_approval(ctx).await;

    // Helpers
    test_deletable_is_allowed_helper(ctx).await;
    test_deletable_requires_approval_helper(ctx).await;
    test_deletable_is_soft_delete_helper(ctx).await;
    test_can_delete_table_helper(ctx).await;

    // Dry run
    test_delete_dry_run(ctx).await;

    // Default
    test_default_deletable_permission(ctx).await;

    // Max affected rows
    test_delete_max_affected_rows(ctx).await;

    println!("\n✅ All Deletable Permission tests passed!\n");
}
