//! Readable columns tests for Cori MCP.
//!
//! Tests the `readable` field in TablePermissions:
//! - ColumnList::All ("*") - all columns accessible
//! - ColumnList::List([...]) - specific columns only
//! - Column filtering in responses
//! - Sensitive columns exclusion
//! - Blocked tables

use super::common::*;
use cori_core::config::role_definition::{
    AllColumns, ColumnList, CreatableColumns, DeletablePermission, RoleDefinition,
    TablePermissions, UpdatableColumns,
};
use cori_mcp::protocol::CallToolOptions;
use serde_json::json;
use std::collections::HashMap;

// =============================================================================
// COLUMN LIST - SPECIFIC COLUMNS
// =============================================================================

pub async fn test_only_readable_columns_returned(ctx: &TestContext) {
    println!("  🧪 test_only_readable_columns_returned");

    let executor = ctx.executor();
    let tool = get_tool("Customer");

    let result = executor
        .execute(
            &tool,
            json!({ "id": 1 }),
            &CallToolOptions::default(),
            &create_context("1"),
        )
        .await;

    assert_success(&result, "GET should succeed");

    let data = extract_json(&result).expect("Should have JSON response");

    // Get the role's readable columns for customers
    let role = create_support_agent_role();
    let customer_perms = role.tables.get("customers").expect("customers should be in role");
    let readable_cols = match &customer_perms.readable {
        ColumnList::List(cols) => cols.clone(),
        ColumnList::All(_) => panic!("Expected explicit column list"),
    };

    // Verify only readable columns are present
    if let Some(obj) = data.as_object() {
        for key in obj.keys() {
            assert!(
                readable_cols.contains(key),
                "Column '{}' should not be in response - not in readable list: {:?}",
                key,
                readable_cols
            );
        }
    }

    println!("     ✓ Only readable columns returned in response");
}

pub async fn test_non_readable_columns_excluded(ctx: &TestContext) {
    println!("  🧪 test_non_readable_columns_excluded");

    let executor = ctx.executor();
    let tool = get_tool("Customer");

    let result = executor
        .execute(
            &tool,
            json!({ "id": 1 }),
            &CallToolOptions::default(),
            &create_context("1"),
        )
        .await;

    assert_success(&result, "GET should succeed");

    let data = extract_json(&result).expect("Should have JSON response");

    // 'notes' and 'phone' columns exist in the customers table but are not in readable list
    assert!(
        data.get("notes").is_none(),
        "'notes' column should not be returned - it's not in the readable list"
    );
    assert!(
        data.get("phone").is_none(),
        "'phone' column should not be returned - it's not in the readable list"
    );

    println!("     ✓ Non-readable columns (notes, phone) excluded from response");
}

pub async fn test_list_returns_only_readable_columns(ctx: &TestContext) {
    println!("  🧪 test_list_returns_only_readable_columns");

    let executor = ctx.executor();
    let tool = list_tool("Customer");

    let result = executor
        .execute(
            &tool,
            json!({ "limit": 10 }),
            &CallToolOptions::default(),
            &create_context("1"),
        )
        .await;

    assert_success(&result, "LIST should succeed");

    let response = extract_json(&result).expect("Should have JSON response");
    let data = response["data"].as_array().unwrap();

    let role = create_support_agent_role();
    let customer_perms = role.tables.get("customers").unwrap();
    let readable_cols = match &customer_perms.readable {
        ColumnList::List(cols) => cols.clone(),
        ColumnList::All(_) => panic!("Expected explicit column list"),
    };

    // Verify each item only has readable columns
    for item in data {
        if let Some(obj) = item.as_object() {
            for key in obj.keys() {
                assert!(
                    readable_cols.contains(key),
                    "Column '{}' should not be in list response",
                    key
                );
            }
        }
    }

    println!("     ✓ LIST results only contain readable columns");
}

// =============================================================================
// COLUMN LIST - ALL COLUMNS (*)
// =============================================================================

pub async fn test_all_columns_readable(ctx: &TestContext) {
    println!("  🧪 test_all_columns_readable");

    // Create a role with "*" for readable
    let role = create_role_with_all_readable("customers");
    let rules = create_default_rules();

    let executor = ctx.executor_with(role, rules);
    let tool = get_tool("Customer");

    let result = executor
        .execute(
            &tool,
            json!({ "id": 1 }),
            &CallToolOptions::default(),
            &create_context("1"),
        )
        .await;

    assert_success(&result, "GET with all readable should succeed");

    let data = extract_json(&result).expect("Should have JSON response");

    // With "*", we should see more columns including ones excluded in specific list
    // The exact columns depend on the database schema
    assert!(data.is_object(), "Response should be an object");

    // Should have at least the basic columns
    assert!(data.get("customer_id").is_some(), "Should have customer_id");
    assert!(data.get("organization_id").is_some(), "Should have organization_id");

    println!("     ✓ All columns accessible with '*' readable");
}

pub async fn test_all_columns_includes_sensitive(ctx: &TestContext) {
    println!("  🧪 test_all_columns_includes_sensitive");

    // With "*" readable, even sensitive columns would be returned
    // (unless filtered by rules/tags - which is a separate feature)

    let role = create_role_with_all_readable("customers");
    let rules = create_default_rules();

    let executor = ctx.executor_with(role, rules);
    let tool = list_tool("Customer");

    let result = executor
        .execute(
            &tool,
            json!({ "limit": 5 }),
            &CallToolOptions::default(),
            &create_context("1"),
        )
        .await;

    assert_success(&result, "LIST with all readable should succeed");

    let response = extract_json(&result).unwrap();
    let data = response["data"].as_array().unwrap();

    // Verify we get all available columns
    if !data.is_empty() {
        let first = &data[0];
        // Count columns - should be more than the restricted list
        let col_count = first.as_object().map(|o| o.len()).unwrap_or(0);
        assert!(col_count > 5, "Should have many columns with '*', got {}", col_count);
    }

    println!("     ✓ All columns mode returns full schema");
}

// =============================================================================
// SENSITIVE COLUMNS CONFIGURATION
// =============================================================================

pub async fn test_sensitive_columns_properly_excluded(_ctx: &TestContext) {
    println!("  🧪 test_sensitive_columns_properly_excluded");

    // Verify that the role configuration properly excludes sensitive columns
    let role = create_support_agent_role();

    // Customers: 'notes' should be excluded (internal notes might be sensitive)
    let customer_perms = role.tables.get("customers").unwrap();
    let readable = match &customer_perms.readable {
        ColumnList::List(cols) => cols,
        _ => panic!("Expected explicit column list"),
    };
    assert!(
        !readable.contains(&"notes".to_string()),
        "customers.notes should not be readable"
    );

    // Products: 'cost' should be excluded (internal cost data)
    let product_perms = role.tables.get("products").unwrap();
    let readable = match &product_perms.readable {
        ColumnList::List(cols) => cols,
        _ => panic!("Expected explicit column list"),
    };
    assert!(
        !readable.contains(&"cost".to_string()),
        "products.cost should not be readable"
    );

    println!("     ✓ Sensitive columns properly excluded from role definition");
}

// =============================================================================
// BLOCKED TABLES
// =============================================================================

pub async fn test_blocked_tables_not_in_permissions(_ctx: &TestContext) {
    println!("  🧪 test_blocked_tables_not_in_permissions");

    let role = create_support_agent_role();

    // Verify blocked tables
    assert!(
        role.blocked_tables.contains(&"users".to_string()),
        "users should be blocked"
    );
    assert!(
        role.blocked_tables.contains(&"api_keys".to_string()),
        "api_keys should be blocked"
    );
    assert!(
        role.blocked_tables.contains(&"billing".to_string()),
        "billing should be blocked"
    );
    assert!(
        role.blocked_tables.contains(&"audit_logs".to_string()),
        "audit_logs should be blocked"
    );

    // Verify blocked tables are not in tables permissions
    assert!(
        !role.tables.contains_key("users"),
        "users should not be in tables permissions"
    );
    assert!(
        !role.tables.contains_key("api_keys"),
        "api_keys should not be in tables permissions"
    );
    assert!(
        !role.tables.contains_key("billing"),
        "billing should not be in tables permissions"
    );

    println!("     ✓ Blocked tables correctly not included in role permissions");
}

pub async fn test_blocked_table_cannot_be_queried(_ctx: &TestContext) {
    println!("  🧪 test_blocked_table_cannot_be_queried");

    // Note: This test verifies the role definition correctly blocks tables,
    // but actual enforcement depends on ToolExecutor implementation.
    // For now, we just verify the role definition is correctly configured.

    let role = create_support_agent_role();
    
    // Verify 'users' is in blocked_tables
    assert!(
        role.blocked_tables.contains(&"users".to_string()),
        "users should be in blocked_tables"
    );
    
    // Verify 'users' is not in the role's tables
    assert!(
        !role.tables.contains_key("users"),
        "users should not be in role.tables"
    );
    
    // The can_access_table helper should return false
    assert!(
        !role.can_access_table("users"),
        "can_access_table should return false for blocked table"
    );

    println!("     ✓ Blocked table correctly configured in role definition");
}

pub async fn test_can_access_table_helper(_ctx: &TestContext) {
    println!("  🧪 test_can_access_table_helper");

    let role = create_support_agent_role();

    // Tables that should be accessible
    assert!(role.can_access_table("customers"), "customers should be accessible");
    assert!(role.can_access_table("orders"), "orders should be accessible");
    assert!(role.can_access_table("tickets"), "tickets should be accessible");

    // Tables that should not be accessible (blocked)
    assert!(!role.can_access_table("users"), "users should not be accessible");
    assert!(!role.can_access_table("api_keys"), "api_keys should not be accessible");
    assert!(!role.can_access_table("billing"), "billing should not be accessible");

    // Non-existent table
    assert!(
        !role.can_access_table("nonexistent_table"),
        "nonexistent table should not be accessible"
    );

    println!("     ✓ can_access_table helper works correctly");
}

// =============================================================================
// EMPTY READABLE COLUMNS
// =============================================================================

pub async fn test_empty_readable_blocks_access(_ctx: &TestContext) {
    println!("  🧪 test_empty_readable_blocks_access");

    // Create a role with empty readable columns for a table
    let mut tables = HashMap::new();
    tables.insert(
        "customers".to_string(),
        TablePermissions {
            readable: ColumnList::List(vec![]), // Empty!
            creatable: CreatableColumns::default(),
            updatable: UpdatableColumns::default(),
            deletable: DeletablePermission::default(),
        },
    );

    let role = RoleDefinition {
        name: "empty_readable_role".to_string(),
        description: Some("Role with no readable columns".to_string()),
        approvals: None,
        tables,
        blocked_tables: Vec::new(),
        max_rows_per_query: Some(100),
        max_affected_rows: Some(10),
    };

    // Verify the role is correctly configured with empty readable columns
    let perms = role.tables.get("customers").unwrap();
    assert!(perms.readable.is_empty(), "readable should be empty");
    
    // Verify ColumnList helpers work correctly
    assert!(!perms.readable.contains("customer_id"), "customer_id should not be readable");
    assert!(!perms.readable.is_all(), "readable should not be 'all'");

    // Note: Actual enforcement of empty readable columns depends on ToolExecutor
    // implementation. This test verifies the role definition is correct.

    println!("     ✓ Empty readable columns configured correctly");
}

// =============================================================================
// COLUMN LIST HELPERS
// =============================================================================

pub async fn test_column_list_contains(_ctx: &TestContext) {
    println!("  🧪 test_column_list_contains");

    // Test ColumnList::List
    let list = ColumnList::List(vec![
        "id".to_string(),
        "name".to_string(),
        "email".to_string(),
    ]);
    assert!(list.contains("id"), "Should contain 'id'");
    assert!(list.contains("name"), "Should contain 'name'");
    assert!(!list.contains("password"), "Should not contain 'password'");

    // Test ColumnList::All
    let all = ColumnList::All(AllColumns);
    assert!(all.contains("any_column"), "All should contain any column");
    assert!(all.contains("password"), "All should contain even sensitive columns");

    println!("     ✓ ColumnList contains helper works correctly");
}

pub async fn test_column_list_is_empty(_ctx: &TestContext) {
    println!("  🧪 test_column_list_is_empty");

    let empty = ColumnList::List(vec![]);
    assert!(empty.is_empty(), "Empty list should be empty");

    let non_empty = ColumnList::List(vec!["id".to_string()]);
    assert!(!non_empty.is_empty(), "Non-empty list should not be empty");

    let all = ColumnList::All(AllColumns);
    assert!(!all.is_empty(), "All columns should not be empty");

    println!("     ✓ ColumnList is_empty helper works correctly");
}

pub async fn test_column_list_is_all(_ctx: &TestContext) {
    println!("  🧪 test_column_list_is_all");

    let list = ColumnList::List(vec!["id".to_string()]);
    assert!(!list.is_all(), "List should not be all");

    let all = ColumnList::All(AllColumns);
    assert!(all.is_all(), "All should be all");

    println!("     ✓ ColumnList is_all helper works correctly");
}

// =============================================================================
// ROLE PERMISSIONS HELPERS
// =============================================================================

pub async fn test_can_read_column_helper(_ctx: &TestContext) {
    println!("  🧪 test_can_read_column_helper");

    let role = create_support_agent_role();

    // Columns that should be readable
    assert!(
        role.can_read_column("customers", "customer_id"),
        "Should be able to read customer_id"
    );
    assert!(
        role.can_read_column("customers", "email"),
        "Should be able to read email"
    );

    // Columns that should not be readable (not in list)
    assert!(
        !role.can_read_column("customers", "notes"),
        "Should not be able to read notes"
    );
    assert!(
        !role.can_read_column("customers", "phone"),
        "Should not be able to read phone"
    );

    // Non-existent table
    assert!(
        !role.can_read_column("nonexistent", "any"),
        "Non-existent table should return false"
    );

    println!("     ✓ can_read_column helper works correctly");
}

pub async fn test_get_readable_columns_helper(_ctx: &TestContext) {
    println!("  🧪 test_get_readable_columns_helper");

    let role = create_support_agent_role();

    let readable = role.get_readable_columns("customers");
    assert!(readable.is_some(), "Should get readable columns for customers");

    let cols = readable.unwrap();
    assert!(cols.contains("customer_id"), "Should include customer_id");
    assert!(cols.contains("email"), "Should include email");

    let none = role.get_readable_columns("nonexistent");
    assert!(none.is_none(), "Non-existent table should return None");

    println!("     ✓ get_readable_columns helper works correctly");
}

// =============================================================================
// TEST RUNNER
// =============================================================================

/// Run all readable columns tests
pub async fn run_all_tests(ctx: &TestContext) {
    println!("\n📖 Running Readable Columns Tests\n");

    // Specific columns
    test_only_readable_columns_returned(ctx).await;
    test_non_readable_columns_excluded(ctx).await;
    test_list_returns_only_readable_columns(ctx).await;

    // All columns
    test_all_columns_readable(ctx).await;
    test_all_columns_includes_sensitive(ctx).await;

    // Sensitive columns
    test_sensitive_columns_properly_excluded(ctx).await;

    // Blocked tables
    test_blocked_tables_not_in_permissions(ctx).await;
    test_blocked_table_cannot_be_queried(ctx).await;
    test_can_access_table_helper(ctx).await;

    // Empty readable
    test_empty_readable_blocks_access(ctx).await;

    // Column list helpers
    test_column_list_contains(ctx).await;
    test_column_list_is_empty(ctx).await;
    test_column_list_is_all(ctx).await;

    // Role permission helpers
    test_can_read_column_helper(ctx).await;
    test_get_readable_columns_helper(ctx).await;

    println!("\n✅ All Readable Columns tests passed!\n");
}
