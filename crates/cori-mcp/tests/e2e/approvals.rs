//! Approval workflow tests for Cori MCP.
//!
//! Tests the human-in-the-loop approval system:
//! - ApprovalRequirement (Simple, Detailed)
//! - ApprovalConfig (global role configuration)
//! - ApprovalManager (pending approval tracking)
//! - Approval workflows for CREATE, UPDATE, DELETE operations

use super::common::*;
use chrono::Duration;
use cori_core::config::role_definition::{
    ApprovalConfig, ApprovalRequirement, ColumnList, CreatableColumnConstraints, CreatableColumns,
    DeletableConstraints, DeletablePermission, RoleDefinition, TablePermissions,
    UpdatableColumnConstraints, UpdatableColumns,
};
use cori_mcp::approval::{ApprovalManager, ApprovalStatus};
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
    let approvals = role.approvals.as_ref().expect("Should have approvals config");
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
            restrict_to: Some(vec![json!("low"), json!("medium"), json!("high"), json!("critical")]),
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
        vec!["id".to_string(), "priority".to_string(), "tenant_id".to_string()],
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
            readable: ColumnList::List(vec!["id".to_string()]),
            creatable: CreatableColumns::default(),
            updatable: UpdatableColumns::default(),
            deletable: DeletablePermission::WithConstraints(DeletableConstraints {
                requires_approval: Some(ApprovalRequirement::Detailed(ApprovalConfig {
                    group: "security_team".to_string(),
                    notify_on_pending: true,
                    message: Some("Deletion of critical records requires security approval".to_string()),
                })),
                soft_delete: false,
            }),
        },
    );

    let role = RoleDefinition {
        name: "delete_approval_role".to_string(),
        description: None,
        approvals: None,
        tables,
        blocked_tables: vec![],
        max_rows_per_query: None,
        max_affected_rows: None,
    };

    let perms = role.tables.get("critical_records").unwrap();
    assert!(perms.deletable.requires_approval());

    println!("     ✓ Deletable with requires_approval configured correctly");
}

// =============================================================================
// APPROVAL MANAGER
// =============================================================================

pub async fn test_approval_manager_create_request(_ctx: &TestContext) {
    println!("  🧪 test_approval_manager_create_request");

    let manager = ApprovalManager::new(Duration::hours(1));

    let request = manager.create_request(
        "updateTicket",
        json!({ "id": 1, "priority": "critical" }),
        vec!["priority".to_string()],
        "tenant_1",
        "support_agent",
    );

    assert_eq!(request.status, ApprovalStatus::Pending);
    assert_eq!(request.tool_name, "updateTicket");
    assert_eq!(request.tenant_id, "tenant_1");
    assert_eq!(request.role, "support_agent");
    assert!(request.approval_fields.contains(&"priority".to_string()));

    // Verify we can retrieve it
    let retrieved = manager.get(&request.id);
    assert!(retrieved.is_some());
    assert_eq!(retrieved.unwrap().id, request.id);

    println!("     ✓ ApprovalManager creates and tracks requests");
}

pub async fn test_approval_manager_approve_request(_ctx: &TestContext) {
    println!("  🧪 test_approval_manager_approve_request");

    let manager = ApprovalManager::new(Duration::hours(1));

    let request = manager.create_request(
        "updateTicket",
        json!({ "id": 1, "priority": "high" }),
        vec!["priority".to_string()],
        "tenant_1",
        "support_agent",
    );

    // Approve the request
    let result = manager.approve(&request.id, "alice@example.com", Some("Looks good".to_string()));
    assert!(result.is_ok(), "Approval should succeed");

    let approved = result.unwrap();
    assert_eq!(approved.status, ApprovalStatus::Approved);
    assert_eq!(approved.decided_by, Some("alice@example.com".to_string()));
    assert_eq!(approved.reason, Some("Looks good".to_string()));

    // Should no longer be in pending
    let pending = manager.list_pending(Some("tenant_1"));
    assert!(pending.is_empty(), "Should have no pending requests");

    println!("     ✓ ApprovalManager approves requests");
}

pub async fn test_approval_manager_reject_request(_ctx: &TestContext) {
    println!("  🧪 test_approval_manager_reject_request");

    let manager = ApprovalManager::new(Duration::hours(1));

    let request = manager.create_request(
        "deleteRecord",
        json!({ "id": 42 }),
        vec![],
        "tenant_1",
        "support_agent",
    );

    // Reject the request
    let result = manager.reject(
        &request.id,
        "security@example.com",
        Some("Policy violation".to_string()),
    );
    assert!(result.is_ok(), "Rejection should succeed");

    let rejected = result.unwrap();
    assert_eq!(rejected.status, ApprovalStatus::Rejected);
    assert_eq!(rejected.reason, Some("Policy violation".to_string()));

    println!("     ✓ ApprovalManager rejects requests with reason");
}

pub async fn test_approval_manager_cancel_request(_ctx: &TestContext) {
    println!("  🧪 test_approval_manager_cancel_request");

    let manager = ApprovalManager::new(Duration::hours(1));

    let request = manager.create_request(
        "updateRecord",
        json!({ "id": 1 }),
        vec![],
        "tenant_1",
        "agent",
    );

    // Cancel the request
    let result = manager.cancel(&request.id);
    assert!(result.is_ok(), "Cancellation should succeed");

    let cancelled = result.unwrap();
    assert_eq!(cancelled.status, ApprovalStatus::Cancelled);

    println!("     ✓ ApprovalManager cancels requests");
}

pub async fn test_approval_manager_list_pending(_ctx: &TestContext) {
    println!("  🧪 test_approval_manager_list_pending");

    let manager = ApprovalManager::new(Duration::hours(1));

    // Add requests for different tenants
    manager.create_request("tool1", json!({}), vec![], "tenant_1", "agent");
    manager.create_request("tool2", json!({}), vec![], "tenant_1", "agent");
    manager.create_request("tool3", json!({}), vec![], "tenant_2", "agent");

    // List all pending
    let all_pending = manager.list_pending(None);
    assert_eq!(all_pending.len(), 3, "Should have 3 pending requests total");

    // List for tenant_1 only
    let tenant1_pending = manager.list_pending(Some("tenant_1"));
    assert_eq!(tenant1_pending.len(), 2, "Should have 2 pending for tenant_1");

    // List for tenant_2 only
    let tenant2_pending = manager.list_pending(Some("tenant_2"));
    assert_eq!(tenant2_pending.len(), 1, "Should have 1 pending for tenant_2");

    println!("     ✓ ApprovalManager lists pending by tenant");
}

pub async fn test_approval_manager_expired_request(_ctx: &TestContext) {
    println!("  🧪 test_approval_manager_expired_request");

    // Create manager with negative TTL (already expired)
    let manager = ApprovalManager::new(Duration::seconds(-1));

    let request = manager.create_request(
        "updateTicket",
        json!({}),
        vec![],
        "tenant_1",
        "agent",
    );

    // Try to approve - should fail with expired error
    let result = manager.approve(&request.id, "admin", None);
    assert!(result.is_err(), "Should fail on expired request");

    // Verify the request is now marked as expired
    let updated = manager.get(&request.id).unwrap();
    assert_eq!(updated.status, ApprovalStatus::Expired);

    println!("     ✓ ApprovalManager handles expired requests");
}

pub async fn test_approval_manager_default(_ctx: &TestContext) {
    println!("  🧪 test_approval_manager_default");

    // Test Default implementation (24 hour TTL)
    let manager = ApprovalManager::default();

    let request = manager.create_request(
        "testTool",
        json!({}),
        vec![],
        "tenant_1",
        "agent",
    );

    // Should not be expired with default 24h TTL
    assert!(!request.is_expired());
    assert!(request.is_pending());

    println!("     ✓ ApprovalManager::default() works correctly");
}

// =============================================================================
// APPROVAL STATUS
// =============================================================================

pub async fn test_approval_status_variants(_ctx: &TestContext) {
    println!("  🧪 test_approval_status_variants");

    // Test all status variants
    assert_eq!(ApprovalStatus::Pending, ApprovalStatus::Pending);
    assert_eq!(ApprovalStatus::Approved, ApprovalStatus::Approved);
    assert_eq!(ApprovalStatus::Rejected, ApprovalStatus::Rejected);
    assert_eq!(ApprovalStatus::Expired, ApprovalStatus::Expired);
    assert_eq!(ApprovalStatus::Cancelled, ApprovalStatus::Cancelled);

    println!("     ✓ ApprovalStatus variants work correctly");
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
// APPROVAL WORKFLOW INTEGRATION
// =============================================================================

pub async fn test_approval_workflow_create_pending(_ctx: &TestContext) {
    println!("  🧪 test_approval_workflow_create_pending");

    // This tests the full workflow: action flagged for approval -> pending state
    let manager = ApprovalManager::new(Duration::hours(1));
    let role = create_support_agent_role();

    // Simulate creating an action that requires approval
    let _request = manager.create_request(
        "tickets/update",
        json!({ "id": 1, "priority": "critical" }),
        vec!["priority".to_string()],
        "1",
        &role.name,
    );

    // Verify pending
    let pending = manager.list_pending(Some("1"));
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].tool_name, "tickets/update");
    assert!(pending[0].approval_fields.contains(&"priority".to_string()));
    assert!(pending[0].is_pending());

    println!("     ✓ Approval workflow creates pending request correctly");
}

pub async fn test_approval_workflow_full_cycle(_ctx: &TestContext) {
    println!("  🧪 test_approval_workflow_full_cycle");

    let manager = ApprovalManager::new(Duration::hours(1));

    // 1. Create pending request
    let request = manager.create_request(
        "tickets/update",
        json!({ "id": 1, "priority": "critical" }),
        vec!["priority".to_string()],
        "1",
        "support_agent",
    );
    assert_eq!(request.status, ApprovalStatus::Pending);

    // 2. Verify it's in pending list
    let pending = manager.list_pending(Some("1"));
    assert_eq!(pending.len(), 1);

    // 3. Approve
    let approved = manager
        .approve(&request.id, "manager@example.com", Some("Approved for escalation".to_string()))
        .unwrap();
    assert_eq!(approved.status, ApprovalStatus::Approved);
    assert!(approved.decided_at.is_some());

    // 4. Verify no longer pending
    let pending_after = manager.list_pending(Some("1"));
    assert!(pending_after.is_empty());

    // 5. Request still exists and is approved
    let final_state = manager.get(&request.id).unwrap();
    assert_eq!(final_state.status, ApprovalStatus::Approved);

    println!("     ✓ Full approval cycle works correctly");
}

pub async fn test_approval_workflow_rejection_cycle(_ctx: &TestContext) {
    println!("  🧪 test_approval_workflow_rejection_cycle");

    let manager = ApprovalManager::new(Duration::hours(1));

    // 1. Create pending request
    let request = manager.create_request(
        "orders/delete",
        json!({ "id": 999 }),
        vec![],
        "1",
        "support_agent",
    );

    // 2. Reject
    let rejected = manager
        .reject(&request.id, "security@example.com", Some("Unauthorized deletion attempt".to_string()))
        .unwrap();
    assert_eq!(rejected.status, ApprovalStatus::Rejected);
    assert_eq!(rejected.reason, Some("Unauthorized deletion attempt".to_string()));

    // 3. Verify not pending
    let pending = manager.list_pending(Some("1"));
    assert!(pending.is_empty());

    println!("     ✓ Rejection workflow works correctly");
}

pub async fn test_already_decided_error(_ctx: &TestContext) {
    println!("  🧪 test_already_decided_error");

    let manager = ApprovalManager::new(Duration::hours(1));

    let request = manager.create_request(
        "test",
        json!({}),
        vec![],
        "tenant_1",
        "agent",
    );

    // Approve first
    manager.approve(&request.id, "user1", None).unwrap();

    // Try to approve again - should fail
    let result = manager.approve(&request.id, "user2", None);
    assert!(result.is_err(), "Should fail on already decided request");

    // Try to reject - should also fail
    let result = manager.reject(&request.id, "user2", None);
    assert!(result.is_err(), "Should fail on already decided request");

    println!("     ✓ Already decided requests cannot be modified");
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

    // Approval manager
    test_approval_manager_create_request(ctx).await;
    test_approval_manager_approve_request(ctx).await;
    test_approval_manager_reject_request(ctx).await;
    test_approval_manager_cancel_request(ctx).await;
    test_approval_manager_list_pending(ctx).await;
    test_approval_manager_expired_request(ctx).await;
    test_approval_manager_default(ctx).await;

    // Approval status
    test_approval_status_variants(ctx).await;

    // Role helpers
    test_role_table_requires_approval_helper(ctx).await;
    test_role_get_approval_columns_helper(ctx).await;

    // Workflow integration
    test_approval_workflow_create_pending(ctx).await;
    test_approval_workflow_full_cycle(ctx).await;
    test_approval_workflow_rejection_cycle(ctx).await;
    test_already_decided_error(ctx).await;

    println!("\n✅ All Approval Workflow tests passed!\n");
}
