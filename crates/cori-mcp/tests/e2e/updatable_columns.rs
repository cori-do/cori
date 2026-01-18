//! Updatable columns tests for Cori MCP.
//!
//! Tests the `updatable` field in TablePermissions:
//! - UpdatableColumns::All ("*") - all columns updatable
//! - UpdatableColumns::Map({...}) - specific columns with constraints
//! - UpdatableColumnConstraints:
//!   - only_when: preconditions on current column values using old.col/new.col syntax
//!   - requires_approval: human approval needed
//!   - guidance: instructions for AI agents
//!
//! The only_when field supports:
//! - Single condition (AND logic): HashMap of column conditions
//! - Multiple conditions (OR logic): Vec of HashMaps

use super::common::*;
use cori_core::config::role_definition::{
    ApprovalRequirement, ColumnCondition, ComparisonCondition, CreatableColumns,
    DeletablePermission, NumberOrColumnRef, OnlyWhen, ReadableConfig, RoleDefinition,
    TablePermissions, UpdatableColumnConstraints, UpdatableColumns,
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
            json!({ "ticket_id": 1 }),
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
            "ticket_id": { "type": "integer" },
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
            json!({ "ticket_id": 1, "status": new_status }),
            &CallToolOptions::default(),
            &create_context("1"),
        )
        .await;

    assert_success(&result, "UPDATE should succeed");

    // Verify update
    let after = executor
        .execute(
            &get_tool,
            json!({ "ticket_id": 1 }),
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
// ONLY_WHEN WITH NEW VALUE RESTRICTION (replaces restrict_to)
// =============================================================================

pub async fn test_update_with_value_restriction_valid(ctx: &TestContext) {
    println!("  🧪 test_update_with_value_restriction_valid");

    let executor = ctx.executor();

    // Verify the constraint is defined correctly in role
    let role = create_support_agent_role();
    let ticket_perms = role.tables.get("tickets").unwrap();

    let updatable_map = match &ticket_perms.updatable {
        UpdatableColumns::Map(m) => m,
        _ => panic!("Expected updatable map"),
    };

    let status_constraints = updatable_map
        .get("status")
        .expect("status should be updatable");

    // Get the new value restriction from only_when
    let allowed = status_constraints
        .only_when
        .as_ref()
        .and_then(|ow| ow.get_new_value_restriction("status"))
        .expect("should have value restriction via only_when");

    // Verify allowed values
    assert!(allowed.contains(&json!("open")));
    assert!(allowed.contains(&json!("in_progress")));
    assert!(allowed.contains(&json!("resolved")));
    assert!(allowed.contains(&json!("closed")));

    // Update with valid value
    let update_tool = update_tool(
        "Ticket",
        json!({
            "ticket_id": { "type": "integer" },
            "status": { "type": "string" }
        }),
    );

    let result = executor
        .execute(
            &update_tool,
            json!({ "ticket_id": 1, "status": "in_progress" }),
            &CallToolOptions::default(),
            &create_context("1"),
        )
        .await;

    assert_success(&result, "UPDATE with valid value should succeed");

    println!("     ✓ Update with valid value restriction succeeds");
}

pub async fn test_update_with_value_restriction_invalid(ctx: &TestContext) {
    println!("  🧪 test_update_with_value_restriction_invalid");

    let executor = ctx.executor();
    let update_tool = update_tool(
        "Ticket",
        json!({
            "ticket_id": { "type": "integer" },
            "status": { "type": "string" }
        }),
    );

    // Update with invalid value
    let result = executor
        .execute(
            &update_tool,
            json!({ "ticket_id": 1, "status": "invalid_status" }),
            &CallToolOptions::default(),
            &create_context("1"),
        )
        .await;

    assert_failure(&result, "UPDATE with invalid status should fail");

    println!("     ✓ Update with invalid value restriction rejected");
}

// =============================================================================
// ONLY_WHEN STATE TRANSITIONS (replaces transitions)
// =============================================================================

pub async fn test_state_transitions_with_only_when(_ctx: &TestContext) {
    println!("  🧪 test_state_transitions_with_only_when");

    // Create a role with state transition using only_when
    // This defines: open -> in_progress, in_progress -> resolved|open, resolved -> closed
    let updatable = HashMap::from([(
        "status".to_string(),
        UpdatableColumnConstraints {
            only_when: Some(OnlyWhen::Multiple(vec![
                // open -> in_progress
                HashMap::from([
                    (
                        "old.status".to_string(),
                        ColumnCondition::Equals(json!("open")),
                    ),
                    (
                        "new.status".to_string(),
                        ColumnCondition::In(vec![json!("in_progress")]),
                    ),
                ]),
                // in_progress -> resolved or open
                HashMap::from([
                    (
                        "old.status".to_string(),
                        ColumnCondition::Equals(json!("in_progress")),
                    ),
                    (
                        "new.status".to_string(),
                        ColumnCondition::In(vec![json!("resolved"), json!("open")]),
                    ),
                ]),
                // resolved -> closed
                HashMap::from([
                    (
                        "old.status".to_string(),
                        ColumnCondition::Equals(json!("resolved")),
                    ),
                    (
                        "new.status".to_string(),
                        ColumnCondition::In(vec![json!("closed")]),
                    ),
                ]),
            ])),
            ..Default::default()
        },
    )]);

    let constraints = updatable.get("status").unwrap();
    assert!(
        constraints.only_when.is_some(),
        "should have only_when condition"
    );

    // Verify it's Multiple (OR logic)
    match constraints.only_when.as_ref().unwrap() {
        OnlyWhen::Multiple(conditions) => {
            assert_eq!(conditions.len(), 3, "should have 3 transition rules");
        }
        _ => panic!("Expected Multiple variant for state transitions"),
    }

    println!("     ✓ State machine transitions defined via only_when with OR logic");
}

pub async fn test_state_transitions_single_condition(_ctx: &TestContext) {
    println!("  🧪 test_state_transitions_single_condition");

    // Simple case: only allow updating when current status is 'draft'
    let updatable = HashMap::from([(
        "status".to_string(),
        UpdatableColumnConstraints {
            only_when: Some(OnlyWhen::Single(HashMap::from([(
                "old.status".to_string(),
                ColumnCondition::Equals(json!("draft")),
            )]))),
            ..Default::default()
        },
    )]);

    let constraints = updatable.get("status").unwrap();
    assert!(
        constraints.only_when.is_some(),
        "should have only_when condition"
    );

    match constraints.only_when.as_ref().unwrap() {
        OnlyWhen::Single(conditions) => {
            assert!(
                conditions.contains_key("old.status"),
                "should reference old.status"
            );
        }
        _ => panic!("Expected Single variant"),
    }

    println!("     ✓ Single transition condition with old. prefix works");
}

// =============================================================================
// ONLY_WHEN PRECONDITIONS
// =============================================================================

pub async fn test_only_when_equals(_ctx: &TestContext) {
    println!("  🧪 test_only_when_equals");

    // Can only update status when is_active is true
    let updatable = HashMap::from([(
        "status".to_string(),
        UpdatableColumnConstraints {
            only_when: Some(OnlyWhen::Single(HashMap::from([(
                "old.is_active".to_string(),
                ColumnCondition::Equals(json!(true)),
            )]))),
            ..Default::default()
        },
    )]);

    let constraints = updatable.get("status").unwrap();
    assert!(
        constraints.only_when.is_some(),
        "should have only_when condition"
    );

    println!("     ✓ only_when with equals condition on old column");
}

pub async fn test_only_when_in_values(_ctx: &TestContext) {
    println!("  🧪 test_only_when_in_values");

    // Can only update priority when current status is open or in_progress
    let updatable = HashMap::from([(
        "priority".to_string(),
        UpdatableColumnConstraints {
            only_when: Some(OnlyWhen::Single(HashMap::from([(
                "old.status".to_string(),
                ColumnCondition::In(vec![json!("open"), json!("in_progress")]),
            )]))),
            ..Default::default()
        },
    )]);

    let constraints = updatable.get("priority").unwrap();
    assert!(
        constraints.only_when.is_some(),
        "should have only_when condition"
    );

    println!("     ✓ only_when with IN condition on old column");
}

pub async fn test_only_when_comparison_with_old_column(_ctx: &TestContext) {
    println!("  🧪 test_only_when_comparison_with_old_column");

    // Can only apply discount when quantity > 10
    let updatable = HashMap::from([(
        "discount".to_string(),
        UpdatableColumnConstraints {
            only_when: Some(OnlyWhen::Single(HashMap::from([(
                "old.quantity".to_string(),
                ColumnCondition::Comparison(ComparisonCondition {
                    greater_than: Some(NumberOrColumnRef::Number(10.0)),
                    ..Default::default()
                }),
            )]))),
            ..Default::default()
        },
    )]);

    let constraints = updatable.get("discount").unwrap();
    let only_when = constraints.only_when.as_ref().unwrap();

    match only_when {
        OnlyWhen::Single(conditions) => {
            let quantity_condition = conditions.get("old.quantity").unwrap();
            if let ColumnCondition::Comparison(cmp) = quantity_condition {
                assert_eq!(cmp.greater_than, Some(NumberOrColumnRef::Number(10.0)));
            } else {
                panic!("Expected comparison condition");
            }
        }
        _ => panic!("Expected Single variant"),
    }

    println!("     ✓ only_when with comparison condition on old column");
}

pub async fn test_only_when_not_null(_ctx: &TestContext) {
    println!("  🧪 test_only_when_not_null");

    // Can only set ship_date when shipping_address is not null
    let updatable = HashMap::from([(
        "ship_date".to_string(),
        UpdatableColumnConstraints {
            only_when: Some(OnlyWhen::Single(HashMap::from([(
                "old.shipping_address".to_string(),
                ColumnCondition::Comparison(ComparisonCondition {
                    not_null: Some(true),
                    ..Default::default()
                }),
            )]))),
            ..Default::default()
        },
    )]);

    let constraints = updatable.get("ship_date").unwrap();
    assert!(
        constraints.only_when.is_some(),
        "should have only_when not_null condition"
    );

    println!("     ✓ only_when with not_null condition on old column");
}

// =============================================================================
// INCREMENT_ONLY PATTERN (using only_when with new >= old)
// =============================================================================

pub async fn test_increment_only_pattern(_ctx: &TestContext) {
    println!("  🧪 test_increment_only_pattern");

    // stock_quantity can only increase: new.stock_quantity >= old.stock_quantity
    let updatable = HashMap::from([(
        "stock_quantity".to_string(),
        UpdatableColumnConstraints {
            only_when: Some(OnlyWhen::Single(HashMap::from([(
                "new.stock_quantity".to_string(),
                ColumnCondition::Comparison(ComparisonCondition {
                    greater_than_or_equal: Some(NumberOrColumnRef::ColumnRef(
                        "old.stock_quantity".to_string(),
                    )),
                    ..Default::default()
                }),
            )]))),
            guidance: Some(
                "Add received stock quantities - use separate adjustment tool for corrections"
                    .to_string(),
            ),
            ..Default::default()
        },
    )]);

    let constraints = updatable.get("stock_quantity").unwrap();
    assert!(
        constraints.only_when.is_some(),
        "should have increment_only pattern"
    );

    match constraints.only_when.as_ref().unwrap() {
        OnlyWhen::Single(conditions) => {
            let condition = conditions.get("new.stock_quantity").unwrap();
            if let ColumnCondition::Comparison(cmp) = condition {
                match &cmp.greater_than_or_equal {
                    Some(NumberOrColumnRef::ColumnRef(col)) => {
                        assert_eq!(col, "old.stock_quantity");
                    }
                    _ => panic!("Expected column reference"),
                }
            } else {
                panic!("Expected comparison condition");
            }
        }
        _ => panic!("Expected Single variant"),
    }

    println!("     ✓ increment_only pattern using new >= old column reference");
}

// =============================================================================
// APPEND_ONLY PATTERN (using only_when with starts_with)
// =============================================================================

pub async fn test_append_only_pattern(_ctx: &TestContext) {
    println!("  🧪 test_append_only_pattern");

    // notes can only be appended: new.notes must start with old.notes
    let updatable = HashMap::from([(
        "notes".to_string(),
        UpdatableColumnConstraints {
            only_when: Some(OnlyWhen::Single(HashMap::from([(
                "new.notes".to_string(),
                ColumnCondition::Comparison(ComparisonCondition {
                    starts_with: Some("old.notes".to_string()),
                    ..Default::default()
                }),
            )]))),
            guidance: Some("Append resolution notes with timestamp and action taken".to_string()),
            ..Default::default()
        },
    )]);

    let constraints = updatable.get("notes").unwrap();
    assert!(
        constraints.only_when.is_some(),
        "should have append_only pattern"
    );

    match constraints.only_when.as_ref().unwrap() {
        OnlyWhen::Single(conditions) => {
            let condition = conditions.get("new.notes").unwrap();
            if let ColumnCondition::Comparison(cmp) = condition {
                assert_eq!(cmp.starts_with, Some("old.notes".to_string()));
            } else {
                panic!("Expected comparison condition");
            }
        }
        _ => panic!("Expected Single variant"),
    }

    println!("     ✓ append_only pattern using starts_with old column reference");
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
            "ticket_id": { "type": "integer" },
            "subject": { "type": "string" }
        }),
    );

    // Try to update subject (not in updatable columns)
    let _result = executor
        .execute(
            &update_tool,
            json!({ "ticket_id": 1, "subject": "Hacked subject" }),
            &CallToolOptions::default(),
            &create_context("1"),
        )
        .await;

    // Check that the subject wasn't actually changed
    let get_tool = get_tool("Ticket");
    let verify = executor
        .execute(
            &get_tool,
            json!({ "ticket_id": 1 }),
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
            readable: ReadableConfig::All(cori_core::config::role_definition::AllColumns),
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
        constraints.unwrap().only_when.is_some(),
        "status should have only_when"
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

    assert!(
        role.can_update("tickets"),
        "Should be able to update tickets"
    );
    assert!(
        !role.can_update("customers"),
        "Should not be able to update customers"
    );
    assert!(
        !role.can_update("orders"),
        "Should not be able to update orders"
    );

    println!("     ✓ can_update helper works correctly");
}

// =============================================================================
// NEW VALUE RESTRICTION HELPER
// =============================================================================

pub async fn test_get_new_value_restriction_helper(_ctx: &TestContext) {
    println!("  🧪 test_get_new_value_restriction_helper");

    // With new.column In constraint
    let constraints = UpdatableColumnConstraints {
        only_when: Some(OnlyWhen::Single(HashMap::from([(
            "new.status".to_string(),
            ColumnCondition::In(vec![json!("open"), json!("closed"), json!("pending")]),
        )]))),
        ..Default::default()
    };

    let allowed = constraints
        .only_when
        .as_ref()
        .and_then(|ow| ow.get_new_value_restriction("status"));
    assert!(allowed.is_some(), "Should extract allowed values");
    let values = allowed.unwrap();
    assert!(values.contains(&json!("open")));
    assert!(values.contains(&json!("closed")));
    assert!(!values.contains(&json!("invalid")));

    // Without new.column constraint
    let no_restrict = UpdatableColumnConstraints::default();
    let no_values = no_restrict
        .only_when
        .as_ref()
        .and_then(|ow| ow.get_new_value_restriction("status"));
    assert!(no_values.is_none(), "Should return None when no only_when");

    println!("     ✓ get_new_value_restriction helper works correctly");
}

// =============================================================================
// COMPLEX COMBINED CONDITIONS
// =============================================================================

pub async fn test_combined_old_and_new_conditions(_ctx: &TestContext) {
    println!("  🧪 test_combined_old_and_new_conditions");

    // Can only change status from 'open' to specific values
    let updatable = HashMap::from([(
        "status".to_string(),
        UpdatableColumnConstraints {
            only_when: Some(OnlyWhen::Single(HashMap::from([
                // old.status must be 'open'
                (
                    "old.status".to_string(),
                    ColumnCondition::Equals(json!("open")),
                ),
                // new.status must be one of these
                (
                    "new.status".to_string(),
                    ColumnCondition::In(vec![json!("in_progress"), json!("cancelled")]),
                ),
            ]))),
            ..Default::default()
        },
    )]);

    let constraints = updatable.get("status").unwrap();
    match constraints.only_when.as_ref().unwrap() {
        OnlyWhen::Single(conditions) => {
            assert!(
                conditions.contains_key("old.status"),
                "should have old.status"
            );
            assert!(
                conditions.contains_key("new.status"),
                "should have new.status"
            );
        }
        _ => panic!("Expected Single variant"),
    }

    println!("     ✓ Combined old and new conditions in single only_when");
}

pub async fn test_or_logic_with_multiple_conditions(_ctx: &TestContext) {
    println!("  🧪 test_or_logic_with_multiple_conditions");

    // Multiple transition rules using OR logic
    let constraints = UpdatableColumnConstraints {
        only_when: Some(OnlyWhen::Multiple(vec![
            // Rule 1: open -> in_progress
            HashMap::from([
                (
                    "old.status".to_string(),
                    ColumnCondition::Equals(json!("open")),
                ),
                (
                    "new.status".to_string(),
                    ColumnCondition::Equals(json!("in_progress")),
                ),
            ]),
            // Rule 2: in_progress -> resolved
            HashMap::from([
                (
                    "old.status".to_string(),
                    ColumnCondition::Equals(json!("in_progress")),
                ),
                (
                    "new.status".to_string(),
                    ColumnCondition::Equals(json!("resolved")),
                ),
            ]),
            // Rule 3: admin can close from any state
            HashMap::from([(
                "new.status".to_string(),
                ColumnCondition::Equals(json!("closed")),
            )]),
        ])),
        ..Default::default()
    };

    match constraints.only_when.as_ref().unwrap() {
        OnlyWhen::Multiple(rules) => {
            assert_eq!(rules.len(), 3, "should have 3 transition rules");
        }
        _ => panic!("Expected Multiple variant"),
    }

    println!("     ✓ OR logic with multiple condition groups");
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
            "ticket_id": { "type": "integer" },
            "status": { "type": "string" }
        }),
    );

    let result = executor
        .execute(
            &update_tool,
            json!({ "ticket_id": 1, "status": "resolved" }),
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
// TEST RUNNER
// =============================================================================

/// Run all updatable columns tests
pub async fn run_all_tests(ctx: &TestContext) {
    println!("\n🔄 Running Updatable Columns Tests\n");

    // Basic update
    test_update_ticket_status(ctx).await;

    // Value restriction (replaces restrict_to)
    test_update_with_value_restriction_valid(ctx).await;
    test_update_with_value_restriction_invalid(ctx).await;

    // State transitions (replaces transitions)
    test_state_transitions_with_only_when(ctx).await;
    test_state_transitions_single_condition(ctx).await;

    // Only_when preconditions
    test_only_when_equals(ctx).await;
    test_only_when_in_values(ctx).await;
    test_only_when_comparison_with_old_column(ctx).await;
    test_only_when_not_null(ctx).await;

    // Increment_only pattern
    test_increment_only_pattern(ctx).await;

    // Append_only pattern
    test_append_only_pattern(ctx).await;

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
    test_get_new_value_restriction_helper(ctx).await;

    // Complex conditions
    test_combined_old_and_new_conditions(ctx).await;
    test_or_logic_with_multiple_conditions(ctx).await;

    // Dry-run
    test_update_dry_run(ctx).await;

    println!("\n✅ All Updatable Columns tests passed!\n");
}
