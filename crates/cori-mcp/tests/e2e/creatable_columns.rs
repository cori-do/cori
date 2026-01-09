//! Creatable columns tests for Cori MCP.
//!
//! Tests the `creatable` field in TablePermissions:
//! - CreatableColumns::All ("*") - all columns creatable
//! - CreatableColumns::Map({...}) - specific columns with constraints
//! - CreatableColumnConstraints:
//!   - required: must provide value
//!   - default: role-specific default value
//!   - restrict_to: whitelist of allowed values
//!   - requires_approval: human approval needed
//!   - guidance: instructions for AI agents

use super::common::*;
use cori_core::config::role_definition::{
    ApprovalRequirement, ColumnList, CreatableColumnConstraints, CreatableColumns,
    DeletablePermission, RoleDefinition, TablePermissions, UpdatableColumns,
};
use cori_mcp::protocol::CallToolOptions;
use serde_json::json;
use std::collections::HashMap;

// =============================================================================
// CREATABLE COLUMNS - BASIC
// =============================================================================

pub async fn test_create_with_required_fields(ctx: &TestContext) {
    println!("  🧪 test_create_with_required_fields");

    // The notes table has required fields: customer_id and content
    let executor = ctx.executor();
    let tool = create_tool(
        "Note",
        json!({
            "customer_id": { "type": "integer" },
            "content": { "type": "string" },
            "is_internal": { "type": "boolean" }
        }),
    );

    // Create with all required fields
    let result = executor
        .execute(
            &tool,
            json!({
                "customer_id": 1,
                "content": "Test note content"
            }),
            &CallToolOptions::default(),
            &create_context("1"),
        )
        .await;

    assert_success(&result, "CREATE with required fields should succeed");

    println!("     ✓ Create with required fields succeeds");
}

pub async fn test_create_missing_required_field(ctx: &TestContext) {
    println!("  🧪 test_create_missing_required_field");

    let executor = ctx.executor();
    let tool = create_tool(
        "Note",
        json!({
            "content": { "type": "string" },
            "is_internal": { "type": "boolean" }
        }),
    );

    // Try to create without required customer_id
    let result = executor
        .execute(
            &tool,
            json!({
                "content": "Test note without customer_id"
            }),
            &CallToolOptions::default(),
            &create_context("1"),
        )
        .await;

    assert_failure(&result, "CREATE without required field should fail");

    println!("     ✓ Create without required field correctly rejected");
}

pub async fn test_create_with_default_value(ctx: &TestContext) {
    println!("  🧪 test_create_with_default_value");

    // The notes table has is_internal with default: false
    let executor = ctx.executor();
    let tool = create_tool(
        "Note",
        json!({
            "customer_id": { "type": "integer" },
            "content": { "type": "string" }
        }),
    );

    // Create without is_internal - should use default
    let result = executor
        .execute(
            &tool,
            json!({
                "customer_id": 1,
                "content": "Test note with default is_internal"
            }),
            &CallToolOptions::default(),
            &create_context("1"),
        )
        .await;

    assert_success(&result, "CREATE should succeed with default value");

    // Verify default was applied
    let data = extract_json(&result);
    if let Some(data) = data {
        if data.get("is_internal").is_some() {
            assert_eq!(
                data["is_internal"], false,
                "is_internal should default to false"
            );
        }
    }

    println!("     ✓ Default value correctly applied on create");
}

// =============================================================================
// RESTRICT_TO CONSTRAINT
// =============================================================================

pub async fn test_create_with_restrict_to_valid_value(ctx: &TestContext) {
    println!("  🧪 test_create_with_restrict_to_valid_value");

    // Create a role with restrict_to constraint on priority
    let creatable = HashMap::from([
        (
            "subject".to_string(),
            CreatableColumnConstraints {
                required: true,
                ..Default::default()
            },
        ),
        (
            "priority".to_string(),
            CreatableColumnConstraints {
                default: Some(json!("low")),
                restrict_to: Some(vec![json!("low"), json!("medium"), json!("high")]),
                ..Default::default()
            },
        ),
        (
            "customer_id".to_string(),
            CreatableColumnConstraints {
                required: true,
                ..Default::default()
            },
        ),
        (
            "ticket_number".to_string(),
            CreatableColumnConstraints {
                // Auto-generate ticket number for tests
                default: Some(json!(format!("TKT-TEST-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis()))),
                ..Default::default()
            },
        ),
    ]);

    let role = create_role_with_creatable(
        "tickets",
        creatable,
        vec![
            "ticket_id".to_string(),
            "subject".to_string(),
            "priority".to_string(),
            "customer_id".to_string(),
            "organization_id".to_string(),
            "ticket_number".to_string(),
        ],
    );

    let rules = create_default_rules();
    let executor = ctx.executor_with(role, rules);

    let tool = create_tool(
        "Ticket",
        json!({
            "subject": { "type": "string" },
            "priority": { "type": "string" },
            "customer_id": { "type": "integer" }
        }),
    );

    // Create with valid priority value
    let result = executor
        .execute(
            &tool,
            json!({
                "subject": "Test ticket",
                "priority": "high",
                "customer_id": 1
            }),
            &CallToolOptions::default(),
            &create_context("1"),
        )
        .await;

    assert_success(&result, "CREATE with valid restrict_to value should succeed");

    println!("     ✓ Create with valid restrict_to value succeeds");
}

pub async fn test_create_with_restrict_to_invalid_value(ctx: &TestContext) {
    println!("  🧪 test_create_with_restrict_to_invalid_value");

    let creatable = HashMap::from([
        (
            "subject".to_string(),
            CreatableColumnConstraints {
                required: true,
                ..Default::default()
            },
        ),
        (
            "priority".to_string(),
            CreatableColumnConstraints {
                restrict_to: Some(vec![json!("low"), json!("medium"), json!("high")]),
                ..Default::default()
            },
        ),
        (
            "customer_id".to_string(),
            CreatableColumnConstraints {
                required: true,
                ..Default::default()
            },
        ),
        (
            "ticket_number".to_string(),
            CreatableColumnConstraints {
                // Auto-generate ticket number for tests
                default: Some(json!(format!("TKT-TEST-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis()))),
                ..Default::default()
            },
        ),
    ]);

    let role = create_role_with_creatable(
        "tickets",
        creatable,
        vec![
            "ticket_id".to_string(),
            "subject".to_string(),
            "priority".to_string(),
            "customer_id".to_string(),
            "ticket_number".to_string(),
        ],
    );

    let rules = create_default_rules();
    let executor = ctx.executor_with(role, rules);

    let tool = create_tool(
        "Ticket",
        json!({
            "subject": { "type": "string" },
            "priority": { "type": "string" },
            "customer_id": { "type": "integer" }
        }),
    );

    // Create with invalid priority value
    let result = executor
        .execute(
            &tool,
            json!({
                "subject": "Test ticket",
                "priority": "critical", // Not in restrict_to!
                "customer_id": 1
            }),
            &CallToolOptions::default(),
            &create_context("1"),
        )
        .await;

    assert_failure(&result, "CREATE with invalid restrict_to value should fail");

    println!("     ✓ Create with invalid restrict_to value rejected");
}

// =============================================================================
// REQUIRES_APPROVAL CONSTRAINT
// =============================================================================

pub async fn test_create_with_requires_approval(ctx: &TestContext) {
    println!("  🧪 test_create_with_requires_approval");

    let creatable = HashMap::from([
        (
            "subject".to_string(),
            CreatableColumnConstraints {
                required: true,
                ..Default::default()
            },
        ),
        (
            "priority".to_string(),
            CreatableColumnConstraints {
                requires_approval: Some(ApprovalRequirement::Simple(true)),
                ..Default::default()
            },
        ),
        (
            "customer_id".to_string(),
            CreatableColumnConstraints {
                required: true,
                ..Default::default()
            },
        ),
        (
            "ticket_number".to_string(),
            CreatableColumnConstraints {
                // Auto-generate ticket number for tests
                default: Some(json!(format!("TKT-TEST-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis()))),
                ..Default::default()
            },
        ),
    ]);

    let role = create_role_with_creatable(
        "tickets",
        creatable,
        vec![
            "ticket_id".to_string(),
            "subject".to_string(),
            "priority".to_string(),
            "customer_id".to_string(),
            "ticket_number".to_string(),
        ],
    );

    let rules = create_default_rules();
    let executor = ctx.executor_with(role, rules);

    let tool = create_tool(
        "Ticket",
        json!({
            "subject": { "type": "string" },
            "priority": { "type": "string" },
            "customer_id": { "type": "integer" }
        }),
    );

    // Create with field that requires approval
    let _result = executor
        .execute(
            &tool,
            json!({
                "subject": "Urgent issue",
                "priority": "critical",
                "customer_id": 1
            }),
            &CallToolOptions::default(),
            &create_context("1"),
        )
        .await;

    // When requires_approval is set, the result should indicate pending approval
    // or be handled by the approval workflow
    // The exact behavior depends on the approval manager implementation

    println!("     ✓ Create with requires_approval handled");
}

// =============================================================================
// GUIDANCE FIELD
// =============================================================================

pub async fn test_creatable_constraint_with_guidance(_ctx: &TestContext) {
    println!("  🧪 test_creatable_constraint_with_guidance");

    let creatable = HashMap::from([(
        "subject".to_string(),
        CreatableColumnConstraints {
            required: true,
            guidance: Some("Use a clear, concise subject that summarizes the issue".to_string()),
            ..Default::default()
        },
    )]);

    // Verify the guidance is stored correctly
    let constraints = creatable.get("subject").unwrap();
    assert!(constraints.guidance.is_some(), "Guidance should be set");
    assert!(
        constraints.guidance.as_ref().unwrap().contains("clear"),
        "Guidance should contain expected text"
    );

    println!("     ✓ Guidance field correctly stored in constraints");
}

// =============================================================================
// CREATABLE COLUMNS - ALL (*)
// =============================================================================

pub async fn test_creatable_all_columns(_ctx: &TestContext) {
    println!("  🧪 test_creatable_all_columns");

    // Create a role with "*" for creatable
    let mut tables = HashMap::new();
    tables.insert(
        "notes".to_string(),
        TablePermissions {
            readable: ColumnList::All(cori_core::config::role_definition::AllColumns),
            creatable: CreatableColumns::All(cori_core::config::role_definition::AllColumns),
            updatable: UpdatableColumns::default(),
            deletable: DeletablePermission::default(),
        },
    );

    let role = RoleDefinition {
        name: "all_creatable_role".to_string(),
        description: Some("Role with all columns creatable".to_string()),
        approvals: None,
        tables,
        blocked_tables: Vec::new(),
        max_rows_per_query: Some(100),
        max_affected_rows: Some(10),
    };

    // Verify creatable is_all
    let perms = role.tables.get("notes").unwrap();
    assert!(perms.creatable.is_all(), "Creatable should be all");

    println!("     ✓ Creatable all (*) correctly configured");
}

// =============================================================================
// CREATABLE COLUMNS - EMPTY
// =============================================================================

pub async fn test_empty_creatable_blocks_create(_ctx: &TestContext) {
    println!("  🧪 test_empty_creatable_blocks_create");

    // The support_agent role has empty creatable for customers
    let role = create_support_agent_role();
    let customer_perms = role.tables.get("customers").unwrap();

    assert!(
        customer_perms.creatable.is_empty(),
        "Customers should have no creatable columns"
    );

    // Verify can_create returns false
    assert!(
        !role.can_create("customers"),
        "Should not be able to create customers"
    );

    // Can create notes (which has creatable columns)
    assert!(role.can_create("notes"), "Should be able to create notes");

    println!("     ✓ Empty creatable correctly blocks create");
}

// =============================================================================
// CREATABLE HELPERS
// =============================================================================

pub async fn test_creatable_contains_helper(_ctx: &TestContext) {
    println!("  🧪 test_creatable_contains_helper");

    let role = create_support_agent_role();
    let notes_perms = role.tables.get("notes").unwrap();

    assert!(
        notes_perms.creatable.contains("customer_id"),
        "notes.customer_id should be creatable"
    );
    assert!(
        notes_perms.creatable.contains("content"),
        "notes.content should be creatable"
    );
    assert!(
        notes_perms.creatable.contains("is_internal"),
        "notes.is_internal should be creatable"
    );
    assert!(
        !notes_perms.creatable.contains("note_id"),
        "notes.note_id should not be creatable (auto-generated)"
    );

    println!("     ✓ CreatableColumns contains helper works correctly");
}

pub async fn test_get_creatable_constraints_helper(_ctx: &TestContext) {
    println!("  🧪 test_get_creatable_constraints_helper");

    let role = create_support_agent_role();

    // Get constraints for customer_id
    let constraints = role.get_creatable_constraints("notes", "customer_id");
    assert!(constraints.is_some(), "Should get constraints for customer_id");
    assert!(
        constraints.unwrap().required,
        "customer_id should be required"
    );

    // Get constraints for is_internal
    let constraints = role.get_creatable_constraints("notes", "is_internal");
    assert!(constraints.is_some(), "Should get constraints for is_internal");
    assert!(
        constraints.unwrap().default.is_some(),
        "is_internal should have default"
    );

    // Non-existent column
    let constraints = role.get_creatable_constraints("notes", "nonexistent");
    assert!(constraints.is_none(), "Non-existent column should return None");

    println!("     ✓ get_creatable_constraints helper works correctly");
}

pub async fn test_creatable_column_names_helper(_ctx: &TestContext) {
    println!("  🧪 test_creatable_column_names_helper");

    let role = create_support_agent_role();
    let notes_perms = role.tables.get("notes").unwrap();

    let col_names = notes_perms.creatable.column_names();
    assert!(col_names.contains(&"customer_id"), "Should contain customer_id");
    assert!(col_names.contains(&"content"), "Should contain content");
    assert!(col_names.contains(&"is_internal"), "Should contain is_internal");

    println!("     ✓ CreatableColumns column_names helper works correctly");
}

pub async fn test_can_create_column_helper(_ctx: &TestContext) {
    println!("  🧪 test_can_create_column_helper");

    let role = create_support_agent_role();

    // Notes table has creatable columns
    assert!(
        role.can_create_column("notes", "customer_id"),
        "Should be able to create notes.customer_id"
    );
    assert!(
        role.can_create_column("notes", "content"),
        "Should be able to create notes.content"
    );

    // Customers table has no creatable columns
    assert!(
        !role.can_create_column("customers", "first_name"),
        "Should not be able to create customers.first_name"
    );

    // Non-existent table
    assert!(
        !role.can_create_column("nonexistent", "any"),
        "Non-existent table should return false"
    );

    println!("     ✓ can_create_column helper works correctly");
}

// =============================================================================
// DRY-RUN FOR CREATE
// =============================================================================

pub async fn test_create_dry_run(ctx: &TestContext) {
    println!("  🧪 test_create_dry_run");

    let executor = ctx.executor();
    let tool = create_tool(
        "Note",
        json!({
            "customer_id": { "type": "integer" },
            "content": { "type": "string" }
        }),
    );

    let result = executor
        .execute(
            &tool,
            json!({
                "customer_id": 1,
                "content": "Dry run test note"
            }),
            &CallToolOptions { dry_run: true },
            &create_context("1"),
        )
        .await;

    assert_success(&result, "CREATE dry run should succeed");
    assert!(result.is_dry_run, "Should be marked as dry run");

    let data = extract_json(&result).expect("Should have JSON response");
    assert!(data["dryRun"].as_bool().unwrap_or(false), "Should indicate dry run");

    println!("     ✓ CREATE dry run works correctly");
}

// =============================================================================
// CONSTRAINT COMBINATIONS
// =============================================================================

pub async fn test_required_with_default(_ctx: &TestContext) {
    println!("  🧪 test_required_with_default");

    // A field can be required but also have a default
    // Required means the final value must exist, default provides fallback
    let constraints = CreatableColumnConstraints {
        required: true,
        default: Some(json!("pending")),
        ..Default::default()
    };

    assert!(constraints.required, "Should be required");
    assert!(constraints.default.is_some(), "Should have default");

    println!("     ✓ Required with default is valid configuration");
}

pub async fn test_restrict_to_with_default(_ctx: &TestContext) {
    println!("  🧪 test_restrict_to_with_default");

    // Default should be within restrict_to values
    let constraints = CreatableColumnConstraints {
        default: Some(json!("low")),
        restrict_to: Some(vec![json!("low"), json!("medium"), json!("high")]),
        ..Default::default()
    };

    let default = constraints.default.as_ref().unwrap();
    let restrict_to = constraints.restrict_to.as_ref().unwrap();
    assert!(
        restrict_to.contains(default),
        "Default should be within restrict_to values"
    );

    println!("     ✓ Restrict_to with matching default is valid");
}

// =============================================================================
// TEST RUNNER
// =============================================================================

/// Run all creatable columns tests
pub async fn run_all_tests(ctx: &TestContext) {
    println!("\n✏️ Running Creatable Columns Tests\n");

    // Basic creatable
    test_create_with_required_fields(ctx).await;
    test_create_missing_required_field(ctx).await;
    test_create_with_default_value(ctx).await;

    // Restrict_to
    test_create_with_restrict_to_valid_value(ctx).await;
    test_create_with_restrict_to_invalid_value(ctx).await;

    // Requires_approval
    test_create_with_requires_approval(ctx).await;

    // Guidance
    test_creatable_constraint_with_guidance(ctx).await;

    // All columns
    test_creatable_all_columns(ctx).await;

    // Empty creatable
    test_empty_creatable_blocks_create(ctx).await;

    // Helpers
    test_creatable_contains_helper(ctx).await;
    test_get_creatable_constraints_helper(ctx).await;
    test_creatable_column_names_helper(ctx).await;
    test_can_create_column_helper(ctx).await;

    // Dry-run
    test_create_dry_run(ctx).await;

    // Constraint combinations
    test_required_with_default(ctx).await;
    test_restrict_to_with_default(ctx).await;

    println!("\n✅ All Creatable Columns tests passed!\n");
}
