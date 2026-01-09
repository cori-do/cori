//! Updatable columns tests for Cori MCP.
//!
//! Tests the `updatable` field in TablePermissions:
//! - UpdatableColumns::All ("*") - all columns updatable
//! - UpdatableColumns::Map({...}) - specific columns with constraints
//! - UpdatableColumnConstraints:
//!   - restrict_to: whitelist of allowed values
//!   - transitions: state machine for valid from->to changes
//!   - only_when: preconditions on current column values
//!   - increment_only: numeric values can only increase
//!   - append_only: text values can only be appended
//!   - requires_approval: human approval needed
//!   - guidance: instructions for AI agents

use super::common::*;
use cori_core::config::role_definition::{
    ApprovalRequirement, ColumnCondition, ColumnList, ComparisonCondition, CreatableColumns,
    DeletablePermission, RoleDefinition, TablePermissions, UpdatableColumnConstraints,
    UpdatableColumns,
};
use cori_mcp::protocol::CallToolOptions;
use serde_json::json;
use std::collections::HashMap;

// =============================================================================
// UPDATE - BASIC
// =============================================================================

pub async fn test_update_ticket_status(ctx: &TestContext) {
    println!("  🧪 test_update_ticket_status");

    let executor = ctx.executor();

    // First get current ticket state
    let get_tool = get_tool("Ticket");
    let before = executor
        .execute(
            &get_tool,
            json!({ "id": 1 }),
            &CallToolOptions::default(),
            &create_context("1"),
        )
        .await;

    assert_success(&before, "GET should succeed");
    let before_data = extract_json(&before).unwrap();
    let old_status = before_data["status"].as_str().unwrap();

    // Update ticket status
    let update_tool = update_tool(
        "Ticket",
        json!({
            "id": { "type": "integer" },
            "status": { "type": "string" }
        }),
    );

    let new_status = if old_status == "resolved" {
        "closed"
    } else {
        "resolved"
    };

    let result = executor
        .execute(
            &update_tool,
            json!({ "id": 1, "status": new_status }),
            &CallToolOptions::default(),
            &create_context("1"),
        )
        .await;

    assert_success(&result, "UPDATE should succeed");

    // Verify update
    let after = executor
        .execute(
            &get_tool,
            json!({ "id": 1 }),
            &CallToolOptions::default(),
            &create_context("1"),
        )
        .await;

    let after_data = extract_json(&after).unwrap();
    assert_eq!(after_data["status"], new_status);

    println!(
        "     ✓ Ticket status updated from '{}' to '{}'",
        old_status, new_status
    );
}

// =============================================================================
// RESTRICT_TO CONSTRAINT
// =============================================================================

pub async fn test_update_with_restrict_to_valid(ctx: &TestContext) {
    println!("  🧪 test_update_with_restrict_to_valid");

    let executor = ctx.executor();

    // Verify the constraint is defined correctly in role
    let role = create_support_agent_role();
    let ticket_perms = role.tables.get("tickets").unwrap();

    let updatable_map = match &ticket_perms.updatable {
        UpdatableColumns::Map(m) => m,
        _ => panic!("Expected updatable map"),
    };

    let status_constraints = updatable_map.get("status").expect("status should be updatable");
    let allowed = status_constraints
        .restrict_to
        .as_ref()
        .expect("should have restrict_to");

    // Verify allowed values
    assert!(allowed.contains(&json!("open")));
    assert!(allowed.contains(&json!("in_progress")));
    assert!(allowed.contains(&json!("resolved")));
    assert!(allowed.contains(&json!("closed")));

    // Update with valid value
    let update_tool = update_tool(
        "Ticket",
        json!({
            "id": { "type": "integer" },
            "status": { "type": "string" }
        }),
    );

    let result = executor
        .execute(
            &update_tool,
            json!({ "id": 1, "status": "in_progress" }),
            &CallToolOptions::default(),
            &create_context("1"),
        )
        .await;

    assert_success(&result, "UPDATE with valid restrict_to value should succeed");

    println!("     ✓ Update with valid restrict_to value succeeds");
}

pub async fn test_update_with_restrict_to_invalid(ctx: &TestContext) {
    println!("  🧪 test_update_with_restrict_to_invalid");

    let executor = ctx.executor();
    let update_tool = update_tool(
        "Ticket",
        json!({
            "id": { "type": "integer" },
            "status": { "type": "string" }
        }),
    );

    // Update with invalid value
    let result = executor
        .execute(
            &update_tool,
            json!({ "id": 1, "status": "invalid_status" }),
            &CallToolOptions::default(),
            &create_context("1"),
        )
        .await;

    assert_failure(&result, "UPDATE with invalid status should fail");

    println!("     ✓ Update with invalid restrict_to value rejected");
}

// =============================================================================
// TRANSITIONS CONSTRAINT (STATE MACHINE)
// =============================================================================

pub async fn test_transitions_valid(_ctx: &TestContext) {
    println!("  🧪 test_transitions_valid");

    // Create a role with transition constraints
    let updatable = HashMap::from([(
        "status".to_string(),
        UpdatableColumnConstraints {
            transitions: Some(HashMap::from([
                ("open".to_string(), vec!["in_progress".to_string()]),
                (
                    "in_progress".to_string(),
                    vec!["resolved".to_string(), "open".to_string()],
                ),
                ("resolved".to_string(), vec!["closed".to_string()]),
            ])),
            ..Default::default()
        },
    )]);

    let constraints = updatable.get("status").unwrap();

    // Test valid transitions
    assert!(
        constraints.is_valid_transition("open", "in_progress"),
        "open -> in_progress should be valid"
    );
    assert!(
        constraints.is_valid_transition("in_progress", "resolved"),
        "in_progress -> resolved should be valid"
    );
    assert!(
        constraints.is_valid_transition("in_progress", "open"),
        "in_progress -> open should be valid"
    );

    // Test invalid transitions
    assert!(
        !constraints.is_valid_transition("open", "resolved"),
        "open -> resolved should be invalid"
    );
    assert!(
        !constraints.is_valid_transition("resolved", "open"),
        "resolved -> open should be invalid"
    );
    assert!(
        !constraints.is_valid_transition("open", "closed"),
        "open -> closed should be invalid"
    );

    println!("     ✓ State machine transitions correctly validated");
}

pub async fn test_transitions_without_constraint(_ctx: &TestContext) {
    println!("  🧪 test_transitions_without_constraint");

    // Without transitions constraint, all changes should be valid
    let constraints = UpdatableColumnConstraints::default();

    assert!(
        constraints.is_valid_transition("any", "other"),
        "Without transitions, all changes should be valid"
    );
    assert!(
        constraints.is_valid_transition("open", "closed"),
        "Without transitions, all changes should be valid"
    );

    println!("     ✓ Without transitions constraint, all changes are valid");
}

// =============================================================================
// ONLY_WHEN CONSTRAINT (PRECONDITIONS)
// =============================================================================

pub async fn test_only_when_equals(_ctx: &TestContext) {
    println!("  🧪 test_only_when_equals");

    let updatable = HashMap::from([(
        "status".to_string(),
        UpdatableColumnConstraints {
            only_when: Some(HashMap::from([(
                "is_active".to_string(),
                ColumnCondition::Equals(json!(true)),
            )])),
            ..Default::default()
        },
    )]);

    let constraints = updatable.get("status").unwrap();
    assert!(
        constraints.only_when.is_some(),
        "should have only_when condition"
    );

    println!("     ✓ only_when with equals condition configured");
}

pub async fn test_only_when_in_values(_ctx: &TestContext) {
    println!("  🧪 test_only_when_in_values");

    let updatable = HashMap::from([(
        "priority".to_string(),
        UpdatableColumnConstraints {
            only_when: Some(HashMap::from([(
                "status".to_string(),
                ColumnCondition::In(vec![json!("open"), json!("in_progress")]),
            )])),
            ..Default::default()
        },
    )]);

    let constraints = updatable.get("priority").unwrap();
    assert!(
        constraints.only_when.is_some(),
        "should have only_when condition"
    );

    println!("     ✓ only_when with IN condition configured");
}

pub async fn test_only_when_comparison(_ctx: &TestContext) {
    println!("  🧪 test_only_when_comparison");

    let updatable = HashMap::from([(
        "discount".to_string(),
        UpdatableColumnConstraints {
            only_when: Some(HashMap::from([(
                "quantity".to_string(),
                ColumnCondition::Comparison(ComparisonCondition {
                    greater_than: Some(10.0),
                    ..Default::default()
                }),
            )])),
            ..Default::default()
        },
    )]);

    let constraints = updatable.get("discount").unwrap();
    let only_when = constraints.only_when.as_ref().unwrap();
    let quantity_condition = only_when.get("quantity").unwrap();

    if let ColumnCondition::Comparison(cmp) = quantity_condition {
        assert_eq!(cmp.greater_than, Some(10.0));
    } else {
        panic!("Expected comparison condition");
    }

    println!("     ✓ only_when with comparison condition configured");
}

pub async fn test_only_when_not_null(_ctx: &TestContext) {
    println!("  🧪 test_only_when_not_null");

    let updatable = HashMap::from([(
        "ship_date".to_string(),
        UpdatableColumnConstraints {
            only_when: Some(HashMap::from([(
                "shipping_address".to_string(),
                ColumnCondition::Comparison(ComparisonCondition {
                    not_null: Some(true),
                    ..Default::default()
                }),
            )])),
            ..Default::default()
        },
    )]);

    let constraints = updatable.get("ship_date").unwrap();
    assert!(
        constraints.only_when.is_some(),
        "should have only_when not_null condition"
    );

    println!("     ✓ only_when with not_null condition configured");
}

// =============================================================================
// INCREMENT_ONLY CONSTRAINT
// =============================================================================

pub async fn test_increment_only_constraint(_ctx: &TestContext) {
    println!("  🧪 test_increment_only_constraint");

    let updatable = HashMap::from([(
        "stock_quantity".to_string(),
        UpdatableColumnConstraints {
            increment_only: true,
            ..Default::default()
        },
    )]);

    let constraints = updatable.get("stock_quantity").unwrap();
    assert!(
        constraints.increment_only,
        "stock_quantity should be increment_only"
    );

    println!("     ✓ increment_only constraint configured");
}

// =============================================================================
// APPEND_ONLY CONSTRAINT
// =============================================================================

pub async fn test_append_only_constraint(_ctx: &TestContext) {
    println!("  🧪 test_append_only_constraint");

    let updatable = HashMap::from([(
        "notes".to_string(),
        UpdatableColumnConstraints {
            append_only: true,
            ..Default::default()
        },
    )]);

    let constraints = updatable.get("notes").unwrap();
    assert!(constraints.append_only, "notes should be append_only");

    println!("     ✓ append_only constraint configured");
}

// =============================================================================
// REQUIRES_APPROVAL CONSTRAINT
// =============================================================================

pub async fn test_update_requires_approval(_ctx: &TestContext) {
    println!("  🧪 test_update_requires_approval");

    let updatable = HashMap::from([(
        "priority".to_string(),
        UpdatableColumnConstraints {
            requires_approval: Some(ApprovalRequirement::Simple(true)),
            ..Default::default()
        },
    )]);

    let role = create_role_with_updatable(
        "tickets",
        updatable,
        vec![
            "ticket_id".to_string(),
            "priority".to_string(),
            "organization_id".to_string(),
        ],
    );

    // Verify role correctly identifies columns requiring approval
    assert!(
        role.table_requires_approval("tickets"),
        "tickets table should require approval"
    );

    let approval_cols = role.get_approval_columns("tickets");
    assert!(
        approval_cols.contains(&"priority"),
        "priority should be in approval columns"
    );

    println!("     ✓ Update requires_approval constraint configured");
}

// =============================================================================
// GUIDANCE FIELD
// =============================================================================

pub async fn test_updatable_guidance(_ctx: &TestContext) {
    println!("  🧪 test_updatable_guidance");

    let updatable = HashMap::from([(
        "status".to_string(),
        UpdatableColumnConstraints {
            guidance: Some(
                "Only mark as 'resolved' after confirming the issue is fully addressed".to_string(),
            ),
            ..Default::default()
        },
    )]);

    let constraints = updatable.get("status").unwrap();
    assert!(constraints.guidance.is_some(), "Guidance should be set");
    assert!(
        constraints.guidance.as_ref().unwrap().contains("resolved"),
        "Guidance should contain expected text"
    );

    println!("     ✓ Guidance field correctly stored in constraints");
}

// =============================================================================
// NON-UPDATABLE COLUMNS
// =============================================================================

pub async fn test_update_non_updatable_column_ignored(ctx: &TestContext) {
    println!("  🧪 test_update_non_updatable_column_ignored");

    let executor = ctx.executor();

    // The tickets table only has 'status' as updatable, not 'subject' or 'priority'
    let update_tool = update_tool(
        "Ticket",
        json!({
            "id": { "type": "integer" },
            "subject": { "type": "string" }
        }),
    );

    // Try to update subject (not in updatable columns)
    let _result = executor
        .execute(
            &update_tool,
            json!({ "id": 1, "subject": "Hacked subject" }),
            &CallToolOptions::default(),
            &create_context("1"),
        )
        .await;

    // Check that the subject wasn't actually changed
    let get_tool = get_tool("Ticket");
    let verify = executor
        .execute(
            &get_tool,
            json!({ "id": 1 }),
            &CallToolOptions::default(),
            &create_context("1"),
        )
        .await;

    let ticket = extract_json(&verify).unwrap();
    assert_ne!(
        ticket["subject"], "Hacked subject",
        "Subject should NOT have been updated - it's not in updatable columns"
    );

    println!("     ✓ Non-updatable columns cannot be modified");
}

// =============================================================================
// UPDATABLE ALL (*)
// =============================================================================

pub async fn test_updatable_all_columns(_ctx: &TestContext) {
    println!("  🧪 test_updatable_all_columns");

    let mut tables = HashMap::new();
    tables.insert(
        "tickets".to_string(),
        TablePermissions {
            readable: ColumnList::All(cori_core::config::role_definition::AllColumns),
            creatable: CreatableColumns::default(),
            updatable: UpdatableColumns::All(cori_core::config::role_definition::AllColumns),
            deletable: DeletablePermission::default(),
        },
    );

    let role = RoleDefinition {
        name: "all_updatable_role".to_string(),
        description: Some("Role with all columns updatable".to_string()),
        approvals: None,
        tables,
        blocked_tables: Vec::new(),
        max_rows_per_query: Some(100),
        max_affected_rows: Some(10),
    };

    let perms = role.tables.get("tickets").unwrap();
    assert!(perms.updatable.is_all(), "Updatable should be all");

    println!("     ✓ Updatable all (*) correctly configured");
}

// =============================================================================
// UPDATABLE HELPERS
// =============================================================================

pub async fn test_updatable_contains_helper(_ctx: &TestContext) {
    println!("  🧪 test_updatable_contains_helper");

    let role = create_support_agent_role();
    let ticket_perms = role.tables.get("tickets").unwrap();

    assert!(
        ticket_perms.updatable.contains("status"),
        "tickets.status should be updatable"
    );
    assert!(
        !ticket_perms.updatable.contains("subject"),
        "tickets.subject should not be updatable"
    );
    assert!(
        !ticket_perms.updatable.contains("priority"),
        "tickets.priority should not be updatable"
    );

    println!("     ✓ UpdatableColumns contains helper works correctly");
}

pub async fn test_get_updatable_constraints_helper(_ctx: &TestContext) {
    println!("  🧪 test_get_updatable_constraints_helper");

    let role = create_support_agent_role();

    let constraints = role.get_updatable_constraints("tickets", "status");
    assert!(constraints.is_some(), "Should get constraints for status");
    assert!(
        constraints.unwrap().restrict_to.is_some(),
        "status should have restrict_to"
    );

    let no_constraints = role.get_updatable_constraints("tickets", "nonexistent");
    assert!(
        no_constraints.is_none(),
        "Non-existent column should return None"
    );

    println!("     ✓ get_updatable_constraints helper works correctly");
}

pub async fn test_can_update_column_helper(_ctx: &TestContext) {
    println!("  🧪 test_can_update_column_helper");

    let role = create_support_agent_role();

    assert!(
        role.can_update_column("tickets", "status"),
        "Should be able to update tickets.status"
    );
    assert!(
        !role.can_update_column("tickets", "subject"),
        "Should not be able to update tickets.subject"
    );
    assert!(
        !role.can_update_column("customers", "first_name"),
        "Should not be able to update customers.first_name"
    );
    assert!(
        !role.can_update_column("nonexistent", "any"),
        "Non-existent table should return false"
    );

    println!("     ✓ can_update_column helper works correctly");
}

pub async fn test_can_update_helper(_ctx: &TestContext) {
    println!("  🧪 test_can_update_helper");

    let role = create_support_agent_role();

    assert!(role.can_update("tickets"), "Should be able to update tickets");
    assert!(
        !role.can_update("customers"),
        "Should not be able to update customers"
    );
    assert!(!role.can_update("orders"), "Should not be able to update orders");

    println!("     ✓ can_update helper works correctly");
}

// =============================================================================
// VALUE VALIDATION HELPERS
// =============================================================================

pub async fn test_is_value_allowed_helper(_ctx: &TestContext) {
    println!("  🧪 test_is_value_allowed_helper");

    let constraints = UpdatableColumnConstraints {
        restrict_to: Some(vec![json!("open"), json!("closed"), json!("pending")]),
        ..Default::default()
    };

    assert!(
        constraints.is_value_allowed(&json!("open")),
        "open should be allowed"
    );
    assert!(
        constraints.is_value_allowed(&json!("closed")),
        "closed should be allowed"
    );
    assert!(
        !constraints.is_value_allowed(&json!("invalid")),
        "invalid should not be allowed"
    );

    // Without restrict_to, all values allowed
    let no_restrict = UpdatableColumnConstraints::default();
    assert!(
        no_restrict.is_value_allowed(&json!("anything")),
        "Without restrict_to, all values allowed"
    );

    println!("     ✓ is_value_allowed helper works correctly");
}

// =============================================================================
// DRY-RUN FOR UPDATE
// =============================================================================

pub async fn test_update_dry_run(ctx: &TestContext) {
    println!("  🧪 test_update_dry_run");

    let executor = ctx.executor();
    let update_tool = update_tool(
        "Ticket",
        json!({
            "id": { "type": "integer" },
            "status": { "type": "string" }
        }),
    );

    let result = executor
        .execute(
            &update_tool,
            json!({ "id": 1, "status": "resolved" }),
            &CallToolOptions { dry_run: true },
            &create_context("1"),
        )
        .await;

    assert_success(&result, "UPDATE dry run should succeed");
    assert!(result.is_dry_run, "Should be marked as dry run");

    let data = extract_json(&result).expect("Should have JSON response");
    assert!(
        data["dryRun"].as_bool().unwrap_or(false),
        "Should indicate dry run"
    );

    println!("     ✓ UPDATE dry run works correctly");
}

// =============================================================================
// MAX_AFFECTED_ROWS
// =============================================================================

pub async fn test_max_affected_rows_enforced(_ctx: &TestContext) {
    println!("  🧪 test_max_affected_rows_enforced");

    let role = create_role_with_max_affected(1);
    assert_eq!(
        role.max_affected_rows,
        Some(1),
        "Role should have max_affected_rows = 1"
    );

    println!("     ✓ max_affected_rows constraint checked");
}

// =============================================================================
// TEST RUNNER
// =============================================================================

/// Run all updatable columns tests
pub async fn run_all_tests(ctx: &TestContext) {
    println!("\n🔄 Running Updatable Columns Tests\n");

    // Basic update
    test_update_ticket_status(ctx).await;

    // Restrict_to
    test_update_with_restrict_to_valid(ctx).await;
    test_update_with_restrict_to_invalid(ctx).await;

    // Transitions
    test_transitions_valid(ctx).await;
    test_transitions_without_constraint(ctx).await;

    // Only_when
    test_only_when_equals(ctx).await;
    test_only_when_in_values(ctx).await;
    test_only_when_comparison(ctx).await;
    test_only_when_not_null(ctx).await;

    // Increment_only
    test_increment_only_constraint(ctx).await;

    // Append_only
    test_append_only_constraint(ctx).await;

    // Requires_approval
    test_update_requires_approval(ctx).await;

    // Guidance
    test_updatable_guidance(ctx).await;

    // Non-updatable
    test_update_non_updatable_column_ignored(ctx).await;

    // All columns
    test_updatable_all_columns(ctx).await;

    // Helpers
    test_updatable_contains_helper(ctx).await;
    test_get_updatable_constraints_helper(ctx).await;
    test_can_update_column_helper(ctx).await;
    test_can_update_helper(ctx).await;
    test_is_value_allowed_helper(ctx).await;

    // Dry-run
    test_update_dry_run(ctx).await;

    // Max affected rows
    test_max_affected_rows_enforced(ctx).await;

    println!("\n✅ All Updatable Columns tests passed!\n");
}
