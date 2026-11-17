// SPDX-License-Identifier: Apache-2.0
//! Approval workflow tests for Cori MCP.
//!
//! Tests the human-in-the-loop approval system:
//! - ApprovalRequirement (Simple, Detailed)
//! - ApprovalConfig (global role configuration)
//! - Parked changes: a gated update is returned (not executed) with its row snapshot, then applied once approved

use super::common::*;
use cori_store_core::protocol::CallToolOptions;
use cori_store_core::config::role_definition::{
    ApprovalConfig, ApprovalRequirement, ColumnCondition, CreatableColumnConstraints,
    CreatableColumns, DeletableConstraints, DeletablePermission, OnlyWhen, ReadableConfig,
    RoleDefinition, TablePermissions, UpdatableColumnConstraints, UpdatableColumns,
};
use serde_json::json;
use std::collections::HashMap;

// =============================================================================
// APPROVAL REQUIREMENT - SIMPLE
// =============================================================================

pub async fn test_approval_requirement_simple_true(_ctx: &TestContext) {
    println!("  🧪 test_approval_requirement_simple_true");

    let requirement = ApprovalRequirement::Simple(true);
    assert!(
        requirement.is_required(),
        "Simple(true) should require approval"
    );

    println!("     ✓ ApprovalRequirement::Simple(true) requires approval");
}

pub async fn test_approval_requirement_simple_false(_ctx: &TestContext) {
    println!("  🧪 test_approval_requirement_simple_false");

    let requirement = ApprovalRequirement::Simple(false);
    assert!(
        !requirement.is_required(),
        "Simple(false) should not require approval"
    );

    println!("     ✓ ApprovalRequirement::Simple(false) does not require approval");
}

// =============================================================================
// APPROVAL REQUIREMENT - DETAILED
// =============================================================================

pub async fn test_approval_requirement_detailed_with_group(_ctx: &TestContext) {
    println!("  🧪 test_approval_requirement_detailed_with_group");

    let requirement = ApprovalRequirement::Detailed(ApprovalConfig {
        group: "managers".to_string(),
        notify_on_pending: true,
        message: Some("Manager approval required".to_string()),
    });

    assert!(
        requirement.is_required(),
        "Detailed with group should require approval"
    );

    if let ApprovalRequirement::Detailed(config) = &requirement {
        assert_eq!(config.group, "managers");
        assert!(config.message.as_ref().unwrap().contains("Manager"));
    }

    // Test get_group helper
    assert_eq!(requirement.get_group(), Some("managers"));

    println!("     ✓ ApprovalRequirement::Detailed with group configured correctly");
}

pub async fn test_approval_requirement_detailed_empty(_ctx: &TestContext) {
    println!("  🧪 test_approval_requirement_detailed_empty");

    let requirement = ApprovalRequirement::Detailed(ApprovalConfig {
        group: "default_group".to_string(),
        notify_on_pending: false,
        message: None,
    });

    // Detailed without message still requires approval
    assert!(
        requirement.is_required(),
        "Detailed should require approval"
    );

    println!("     ✓ ApprovalRequirement::Detailed requires approval");
}

// =============================================================================
// APPROVAL CONFIG (ROLE-LEVEL)
// =============================================================================

pub async fn test_approval_config_defaults(_ctx: &TestContext) {
    println!("  🧪 test_approval_config_defaults");

    let config = ApprovalConfig {
        group: "support_managers".to_string(),
        notify_on_pending: true,
        message: Some("Action requires manager approval".to_string()),
    };

    assert_eq!(config.group, "support_managers");
    assert!(config.notify_on_pending);
    assert!(config.message.as_ref().unwrap().contains("manager"));

    println!("     ✓ ApprovalConfig defaults configured correctly");
}

pub async fn test_role_with_approvals(_ctx: &TestContext) {
    println!("  🧪 test_role_with_approvals");

    let role = create_support_agent_role();

    // Support agent role should have approvals config
    let approvals = role
        .approvals
        .as_ref()
        .expect("Should have approvals config");
    assert_eq!(approvals.group, "support_managers");
    assert!(approvals.notify_on_pending);

    println!("     ✓ Role with approvals configured correctly");
}

// =============================================================================
// CREATABLE REQUIRES APPROVAL
// =============================================================================

pub async fn test_creatable_requires_approval(_ctx: &TestContext) {
    println!("  🧪 test_creatable_requires_approval");

    let mut creatable = HashMap::new();
    creatable.insert(
        "sensitive_data".to_string(),
        CreatableColumnConstraints {
            required: false,
            default: None,
            restrict_to: None,
            requires_approval: Some(ApprovalRequirement::Simple(true)),
            guidance: Some("Creating sensitive data requires approval".to_string()),
            foreign_key: None,
        },
    );

    let role = create_role_with_creatable(
        "secrets",
        creatable,
        vec!["id".to_string(), "sensitive_data".to_string()],
    );

    assert!(
        role.table_requires_approval("secrets"),
        "secrets table should require approval"
    );

    let approval_cols = role.get_approval_columns("secrets");
    assert!(
        approval_cols.contains(&"sensitive_data"),
        "sensitive_data should be in approval columns"
    );

    println!("     ✓ Creatable with requires_approval configured correctly");
}

// =============================================================================
// UPDATABLE REQUIRES APPROVAL
// =============================================================================

pub async fn test_updatable_requires_approval(_ctx: &TestContext) {
    println!("  🧪 test_updatable_requires_approval");

    let mut updatable = HashMap::new();
    updatable.insert(
        "priority".to_string(),
        UpdatableColumnConstraints {
            only_when: Some(OnlyWhen::Single(HashMap::from([(
                "new.priority".to_string(),
                ColumnCondition::In(vec![
                    json!("low"),
                    json!("medium"),
                    json!("high"),
                    json!("critical"),
                ]),
            )]))),
            requires_approval: Some(ApprovalRequirement::Detailed(ApprovalConfig {
                group: "escalation_team".to_string(),
                notify_on_pending: true,
                message: Some("Priority escalation requires team approval".to_string()),
            })),
            ..Default::default()
        },
    );

    let role = create_role_with_updatable(
        "tickets",
        updatable,
        vec![
            "id".to_string(),
            "priority".to_string(),
            "tenant_id".to_string(),
        ],
    );

    assert!(role.table_requires_approval("tickets"));

    // Get constraints and verify
    let constraints = role.get_updatable_constraints("tickets", "priority");
    assert!(constraints.is_some());
    assert!(constraints.unwrap().requires_approval.is_some());

    println!("     ✓ Updatable with requires_approval configured correctly");
}

// =============================================================================
// DELETABLE REQUIRES APPROVAL
// =============================================================================

pub async fn test_deletable_requires_approval(_ctx: &TestContext) {
    println!("  🧪 test_deletable_requires_approval");

    let mut tables = HashMap::new();
    tables.insert(
        "critical_records".to_string(),
        TablePermissions {
            scope: Default::default(),
            readable: ReadableConfig::List(vec!["id".to_string()]),
            creatable: CreatableColumns::default(),
            updatable: UpdatableColumns::default(),
            deletable: DeletablePermission::WithConstraints(DeletableConstraints {
                requires_approval: Some(ApprovalRequirement::Detailed(ApprovalConfig {
                    group: "security_team".to_string(),
                    notify_on_pending: true,
                    message: Some(
                        "Deletion of critical records requires security approval".to_string(),
                    ),
                })),
                soft_delete: false,
                verify_with: vec![],
            }),
        },
    );

    let role = RoleDefinition {
        name: "delete_approval_role".to_string(),
        description: None,
        approvals: None,
        tables,
    };

    let perms = role.tables.get("critical_records").unwrap();
    assert!(perms.deletable.requires_approval());

    println!("     ✓ Deletable with requires_approval configured correctly");
}

// =============================================================================
// PARKED CHANGES
// =============================================================================

async fn priority_of(ctx: &TestContext, id: i32) -> String {
    sqlx::query_scalar("SELECT priority FROM tickets WHERE ticket_id = $1").bind(id).fetch_one(&ctx.pool).await.unwrap()
}

/// A gated update is returned as a parked change (with the row as the role read it) and writes nothing; a dry run parks
/// nothing; approving applies it; a change whose row changed since it was parked is refused, without echoing the row.
pub async fn test_gated_update_parks_then_applies(ctx: &TestContext) {
    println!("  🧪 test_gated_update_parks_then_applies");

    let id: i32 = sqlx::query_scalar("INSERT INTO tickets (organization_id, customer_id, ticket_number, subject, priority) VALUES (1, 1, 'ACME-TKT-PARKED', 'Parked change', 'medium') RETURNING ticket_id")
        .fetch_one(&ctx.pool).await.unwrap();
    let executor = ctx.executor_with(create_role_with_approval_columns(), create_default_rules());
    let tool = update_tool("Ticket", json!({ "ticket_id": { "type": "integer" }, "priority": { "type": "string" } }));
    let context = create_context_with_role("1", "mixed_approval_role");

    let parked = executor.execute(&tool, json!({ "ticket_id": id, "priority": "urgent" }), &CallToolOptions::default(), &context).await;
    assert_success(&parked, "a gated update is accepted");
    assert_eq!(extract_json(&parked).unwrap()["status"], "pending_approval");
    let change = parked.parked.clone().expect("the change is returned");
    assert_eq!((change.tool_name.as_str(), change.tenant_id.as_str(), change.target_table.as_deref()), ("updateTicket", "1", Some("tickets")));
    assert_eq!(change.approval_fields, vec!["priority".to_string()]);
    assert_eq!(priority_of(ctx, id).await, "medium", "nothing is written while it waits");

    let dry = executor.execute(&tool, json!({ "ticket_id": id, "priority": "urgent" }), &CallToolOptions { dry_run: true }, &context).await;
    assert!(dry.is_dry_run && dry.parked.is_none(), "a dry run parks nothing: {dry:?}");

    let applied = executor.execute_approved(&tool, &change, &context).await;
    assert_success(&applied, "the approved change applies");
    assert_eq!(priority_of(ctx, id).await, "urgent");

    let stale = executor.execute(&tool, json!({ "ticket_id": id, "priority": "low" }), &CallToolOptions::default(), &context).await.parked.expect("parked");
    sqlx::query("UPDATE tickets SET subject = 'changed meanwhile' WHERE ticket_id = $1").bind(id).execute(&ctx.pool).await.unwrap();
    let refused = executor.execute_approved(&tool, &stale, &context).await;
    assert_failure(&refused, "a change whose row changed is refused");
    assert!(!refused.error.clone().unwrap_or_default().contains("changed meanwhile"), "{refused:?}");
    assert_eq!(priority_of(ctx, id).await, "urgent");

    sqlx::query("DELETE FROM tickets WHERE ticket_id = $1").bind(id).execute(&ctx.pool).await.unwrap();
    println!("     ✓ A gated update parks, applies once approved, and is refused once its row changed");
}

// =============================================================================
// ROLE HELPERS FOR APPROVALS
// =============================================================================

pub async fn test_role_table_requires_approval_helper(_ctx: &TestContext) {
    println!("  🧪 test_role_table_requires_approval_helper");

    let role = create_role_with_approval_columns();

    assert!(
        role.table_requires_approval("tickets"),
        "tickets should require approval (has priority with requires_approval)"
    );
    assert!(
        !role.table_requires_approval("customers"),
        "customers should not require approval"
    );

    println!("     ✓ table_requires_approval helper works correctly");
}

pub async fn test_role_get_approval_columns_helper(_ctx: &TestContext) {
    println!("  🧪 test_role_get_approval_columns_helper");

    let role = create_role_with_approval_columns();

    let approval_cols = role.get_approval_columns("tickets");
    assert!(!approval_cols.is_empty(), "Should have approval columns");
    assert!(
        approval_cols.contains(&"priority"),
        "priority should require approval"
    );

    let no_approval_cols = role.get_approval_columns("customers");
    assert!(
        no_approval_cols.is_empty(),
        "customers should have no approval columns"
    );

    println!("     ✓ get_approval_columns helper works correctly");
}

// =============================================================================
// TEST RUNNER
// =============================================================================

/// Run all approval workflow tests
pub async fn run_all_tests(ctx: &TestContext) {
    println!("\n✋ Running Approval Workflow Tests\n");

    // Approval requirement
    test_approval_requirement_simple_true(ctx).await;
    test_approval_requirement_simple_false(ctx).await;
    test_approval_requirement_detailed_with_group(ctx).await;
    test_approval_requirement_detailed_empty(ctx).await;

    // Approval config
    test_approval_config_defaults(ctx).await;
    test_role_with_approvals(ctx).await;

    // Creatable requires approval
    test_creatable_requires_approval(ctx).await;

    // Updatable requires approval
    test_updatable_requires_approval(ctx).await;

    // Deletable requires approval
    test_deletable_requires_approval(ctx).await;

    // Parked changes
    test_gated_update_parks_then_applies(ctx).await;

    // Role helpers
    test_role_table_requires_approval_helper(ctx).await;
    test_role_get_approval_columns_helper(ctx).await;

    println!("\n✅ All Approval Workflow tests passed!\n");
}
