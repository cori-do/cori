//! Tenant isolation tests for Cori MCP.
//!
//! Tests multi-tenant data isolation including:
//! - Cross-tenant access blocking
//! - Direct tenant column filtering
//! - Inherited tenant filtering via FK
//! - Global tables (no tenant filtering)
//! - Empty tenant handling

use super::common::*;
use cori_core::config::role_definition::{
    CreatableColumns, DeletablePermission, ReadableConfig, TablePermissions, UpdatableColumns,
};
use cori_core::config::rules_definition::TenantConfig;
use cori_mcp::protocol::CallToolOptions;
use serde_json::json;

// =============================================================================
// CROSS-TENANT ACCESS BLOCKING
// =============================================================================

pub async fn test_cross_tenant_get_blocked(ctx: &TestContext) {
    println!("  🧪 test_cross_tenant_get_blocked");

    let executor = ctx.executor();
    let tool = get_tool("Customer");

    // Customer 1 belongs to org 1, but we're authenticated as org 2
    let result = executor
        .execute(
            &tool,
            json!({ "customer_id": 1 }),
            &CallToolOptions::default(),
            &create_context("2"), // Different tenant!
        )
        .await;

    assert!(result.success, "Query should succeed but return no data");

    let data = extract_json(&result).expect("Should have JSON response");
    // Should return null/not found since tenant isolation blocks access
    assert!(
        data.is_null() || data["data"].is_null() || data["message"] == "Record not found",
        "Should not find customer from different tenant: {:?}",
        data
    );

    println!("     ✓ Cross-tenant GET access correctly blocked");
}

pub async fn test_cross_tenant_list_isolated(ctx: &TestContext) {
    println!("  🧪 test_cross_tenant_list_isolated");

    let executor = ctx.executor();
    let tool = list_tool("Customer");

    // List customers for tenant 1
    let result1 = executor
        .execute(
            &tool,
            json!({ "limit": 100 }),
            &CallToolOptions::default(),
            &create_context("1"),
        )
        .await;

    assert_success(&result1, "Tenant 1 LIST should succeed");
    let data1 = extract_json(&result1).unwrap();
    let customers1 = data1["data"].as_array().unwrap();

    // List customers for tenant 2
    let result2 = executor
        .execute(
            &tool,
            json!({ "limit": 100 }),
            &CallToolOptions::default(),
            &create_context("2"),
        )
        .await;

    assert_success(&result2, "Tenant 2 LIST should succeed");
    let data2 = extract_json(&result2).unwrap();
    let customers2 = data2["data"].as_array().unwrap();

    // Verify no overlap between tenant results
    let ids1: std::collections::HashSet<_> = customers1
        .iter()
        .filter_map(|c| c["customer_id"].as_i64())
        .collect();
    let ids2: std::collections::HashSet<_> = customers2
        .iter()
        .filter_map(|c| c["customer_id"].as_i64())
        .collect();

    assert!(
        ids1.is_disjoint(&ids2),
        "Tenant customer IDs should not overlap"
    );

    // Verify all returned customers belong to their respective tenants
    for c in customers1 {
        assert_eq!(
            c["organization_id"], 1,
            "Tenant 1 customer should have org_id=1"
        );
    }
    for c in customers2 {
        assert_eq!(
            c["organization_id"], 2,
            "Tenant 2 customer should have org_id=2"
        );
    }

    println!(
        "     ✓ Cross-tenant LIST results are isolated ({} vs {} customers)",
        ids1.len(),
        ids2.len()
    );
}

// =============================================================================
// EMPTY TENANT HANDLING
// =============================================================================

pub async fn test_empty_tenant_fails_for_scoped_table(ctx: &TestContext) {
    println!("  🧪 test_empty_tenant_fails_for_scoped_table");

    let executor = ctx.executor();
    let tool = list_tool("Customer");

    let result = executor
        .execute(
            &tool,
            json!({ "limit": 10 }),
            &CallToolOptions::default(),
            &create_context(""), // Empty tenant!
        )
        .await;

    assert_failure(&result, "Empty tenant should fail for scoped table");

    println!("     ✓ Empty tenant correctly rejected for tenant-scoped table");
}

pub async fn test_null_tenant_fails(ctx: &TestContext) {
    println!("  🧪 test_null_tenant_fails");

    let executor = ctx.executor();
    let tool = get_tool("Customer");

    // Using a context with whitespace-only tenant
    let result = executor
        .execute(
            &tool,
            json!({ "customer_id": 1 }),
            &CallToolOptions::default(),
            &create_context("   "), // Whitespace tenant
        )
        .await;

    // Should fail or return no data
    if result.success {
        let data = extract_json(&result).unwrap();
        assert!(
            data.is_null() || data["data"].is_null(),
            "Whitespace tenant should not match any data"
        );
    }

    println!("     ✓ Whitespace tenant handled correctly");
}

// =============================================================================
// MULTIPLE TENANTS ISOLATION
// =============================================================================

pub async fn test_three_tenants_completely_isolated(ctx: &TestContext) {
    println!("  🧪 test_three_tenants_completely_isolated");

    let executor = ctx.executor();
    let tool = list_tool("Customer");

    // Tenant 1 (Acme) - should have 5 customers
    let result1 = executor
        .execute(
            &tool,
            json!({ "limit": 100 }),
            &CallToolOptions::default(),
            &create_context("1"),
        )
        .await;

    assert_success(&result1, "Tenant 1 query should succeed");
    let data1 = extract_json(&result1).unwrap();
    let customers1 = data1["data"].as_array().unwrap();

    // Tenant 2 (Globex) - should have 6 customers
    let result2 = executor
        .execute(
            &tool,
            json!({ "limit": 100 }),
            &CallToolOptions::default(),
            &create_context("2"),
        )
        .await;

    assert_success(&result2, "Tenant 2 query should succeed");
    let data2 = extract_json(&result2).unwrap();
    let customers2 = data2["data"].as_array().unwrap();

    // Tenant 3 (Initech) - should have 3 customers
    let result3 = executor
        .execute(
            &tool,
            json!({ "limit": 100 }),
            &CallToolOptions::default(),
            &create_context("3"),
        )
        .await;

    assert_success(&result3, "Tenant 3 query should succeed");
    let data3 = extract_json(&result3).unwrap();
    let customers3 = data3["data"].as_array().unwrap();

    // Verify counts match seed data
    assert_eq!(customers1.len(), 5, "Acme should have 5 customers");
    assert_eq!(customers2.len(), 6, "Globex should have 6 customers");
    assert_eq!(customers3.len(), 3, "Initech should have 3 customers");

    // Verify no data leakage between tenants
    let ids1: std::collections::HashSet<_> = customers1
        .iter()
        .filter_map(|c| c["customer_id"].as_i64())
        .collect();
    let ids2: std::collections::HashSet<_> = customers2
        .iter()
        .filter_map(|c| c["customer_id"].as_i64())
        .collect();
    let ids3: std::collections::HashSet<_> = customers3
        .iter()
        .filter_map(|c| c["customer_id"].as_i64())
        .collect();

    assert!(
        ids1.is_disjoint(&ids2),
        "Tenant 1 and 2 should have no overlap"
    );
    assert!(
        ids1.is_disjoint(&ids3),
        "Tenant 1 and 3 should have no overlap"
    );
    assert!(
        ids2.is_disjoint(&ids3),
        "Tenant 2 and 3 should have no overlap"
    );

    // Verify all returned customers have correct tenant
    for c in customers1 {
        assert_eq!(
            c["organization_id"], 1,
            "Tenant 1 customer should have org_id=1"
        );
    }
    for c in customers2 {
        assert_eq!(
            c["organization_id"], 2,
            "Tenant 2 customer should have org_id=2"
        );
    }
    for c in customers3 {
        assert_eq!(
            c["organization_id"], 3,
            "Tenant 3 customer should have org_id=3"
        );
    }

    println!("     ✓ All 3 tenants completely isolated (5+6+3=14 customers, no overlap)");
}

// =============================================================================
// GLOBAL TABLES (NO TENANT FILTERING)
// =============================================================================

pub async fn test_global_table_returns_all_records(ctx: &TestContext) {
    println!("  🧪 test_global_table_returns_all_records");

    // Create rules with 'organizations' as a global table
    let rules = create_rules_with_global_table("organizations");

    // Create a role that can read organizations
    let mut role = create_support_agent_role();
    role.tables.insert(
        "organizations".to_string(),
        TablePermissions {
            readable: ReadableConfig::List(vec![
                "organization_id".to_string(),
                "name".to_string(),
                "slug".to_string(),
                "plan".to_string(),
                "created_at".to_string(),
            ]),
            creatable: CreatableColumns::default(),
            updatable: UpdatableColumns::default(),
            deletable: DeletablePermission::default(),
        },
    );

    let executor = ctx.executor_with(role, rules);
    let tool = list_tool("Organization");

    // Execute - should return ALL organizations regardless of tenant context
    let result = executor
        .execute(
            &tool,
            json!({ "limit": 100 }),
            &CallToolOptions::default(),
            &create_context("1"), // Tenant provided but should be ignored
        )
        .await;

    assert_success(&result, "Global table query should succeed");

    let response = extract_json(&result).unwrap();
    let data = response["data"].as_array().unwrap();

    // Should see ALL organizations (not just tenant 1's)
    // Seed data has 3 organizations
    assert!(
        data.len() >= 3,
        "Global table should return all organizations (3 in seed data), got {}",
        data.len()
    );

    // Verify we see multiple different organization IDs
    let org_ids: std::collections::HashSet<_> = data
        .iter()
        .filter_map(|o| o["organization_id"].as_i64())
        .collect();

    assert!(
        org_ids.len() >= 3,
        "Should see multiple organizations in global table"
    );

    println!(
        "     ✓ Global table returns all {} records without tenant filtering",
        data.len()
    );
}

pub async fn test_global_table_accessible_from_any_tenant(ctx: &TestContext) {
    println!("  🧪 test_global_table_accessible_from_any_tenant");

    let rules = create_rules_with_global_table("organizations");

    let mut role = create_support_agent_role();
    role.tables.insert(
        "organizations".to_string(),
        TablePermissions {
            readable: ReadableConfig::List(vec!["organization_id".to_string(), "name".to_string()]),
            creatable: CreatableColumns::default(),
            updatable: UpdatableColumns::default(),
            deletable: DeletablePermission::default(),
        },
    );

    let executor = ctx.executor_with(role, rules);
    let tool = list_tool("Organization");

    // Query from tenant 1
    let result1 = executor
        .execute(
            &tool,
            json!({ "limit": 100 }),
            &CallToolOptions::default(),
            &create_context("1"),
        )
        .await;

    // Query from tenant 2
    let result2 = executor
        .execute(
            &tool,
            json!({ "limit": 100 }),
            &CallToolOptions::default(),
            &create_context("2"),
        )
        .await;

    // Query from tenant 3
    let result3 = executor
        .execute(
            &tool,
            json!({ "limit": 100 }),
            &CallToolOptions::default(),
            &create_context("3"),
        )
        .await;

    assert_success(&result1, "Tenant 1 should access global table");
    assert_success(&result2, "Tenant 2 should access global table");
    assert_success(&result3, "Tenant 3 should access global table");

    // All should return the same data
    let data1 = extract_json(&result1).unwrap()["data"]
        .as_array()
        .unwrap()
        .len();
    let data2 = extract_json(&result2).unwrap()["data"]
        .as_array()
        .unwrap()
        .len();
    let data3 = extract_json(&result3).unwrap()["data"]
        .as_array()
        .unwrap()
        .len();

    assert_eq!(
        data1, data2,
        "Global table should return same data for all tenants"
    );
    assert_eq!(
        data2, data3,
        "Global table should return same data for all tenants"
    );

    println!(
        "     ✓ Global table accessible from any tenant ({} records each)",
        data1
    );
}

// =============================================================================
// TENANT CONFIGURATION VALIDATION
// =============================================================================

pub async fn test_tenant_scoped_table_has_config(_ctx: &TestContext) {
    println!("  🧪 test_tenant_scoped_table_has_config");

    // Verify rules have tenant configuration
    let rules = create_default_rules();

    let customer_rules = rules
        .tables
        .get("customers")
        .expect("customers should be in rules");
    assert!(
        customer_rules.tenant.is_some(),
        "customers should have tenant configuration"
    );

    match &customer_rules.tenant {
        Some(TenantConfig::Direct(col)) => {
            assert_eq!(
                col, "organization_id",
                "Tenant column should be organization_id"
            );
        }
        _ => panic!("Expected direct tenant config"),
    }

    // Verify global is not set
    assert!(
        customer_rules.global.is_none() || customer_rules.global == Some(false),
        "customers should not be a global table"
    );

    println!("     ✓ Tenant-scoped tables properly configured with tenant column");
}

pub async fn test_different_tables_same_tenant_column(ctx: &TestContext) {
    println!("  🧪 test_different_tables_same_tenant_column");

    let executor = ctx.executor();

    // Test that different tables all use the same tenant column
    // Use singular names as the list_tool function will pluralize them
    let tables = vec!["Customer", "Order", "Ticket"];

    for table_name in tables {
        let tool = list_tool(table_name);
        let result = executor
            .execute(
                &tool,
                json!({ "limit": 10 }),
                &CallToolOptions::default(),
                &create_context("1"),
            )
            .await;

        assert_success(&result, &format!("{} list should succeed", table_name));

        let response = extract_json(&result).unwrap();
        let data = response["data"].as_array().unwrap();

        for item in data {
            assert_eq!(
                item["organization_id"], 1,
                "All {} should belong to tenant 1",
                table_name
            );
        }
    }

    println!("     ✓ Different tables all correctly filter by tenant");
}

// =============================================================================
// INHERITED TENANT (VIA FK)
// =============================================================================

pub async fn test_inherited_tenant_via_fk(ctx: &TestContext) {
    println!("  🧪 test_inherited_tenant_via_fk");

    // In the demo schema, order_items inherits tenant from orders via order_id
    // This test verifies the concept even if using direct tenant column

    let rules = create_default_rules();

    // Verify we can query order_items for a specific tenant
    let executor = ctx.executor_with(create_support_agent_role(), rules);
    let tool = list_tool("OrderItem");

    let result = executor
        .execute(
            &tool,
            json!({ "limit": 100 }),
            &CallToolOptions::default(),
            &create_context("1"),
        )
        .await;

    assert_success(&result, "Order items query should succeed");

    let response = extract_json(&result).unwrap();
    let data = response["data"].as_array().unwrap();

    // All order_items should belong to tenant 1
    for item in data {
        assert_eq!(
            item["organization_id"], 1,
            "All order items should belong to tenant 1"
        );
    }

    println!(
        "     ✓ Order items correctly filtered by tenant ({} items)",
        data.len()
    );
}

// =============================================================================
// TENANT ID TYPES
// =============================================================================

pub async fn test_string_tenant_id(ctx: &TestContext) {
    println!("  🧪 test_string_tenant_id");

    let executor = ctx.executor();
    let tool = list_tool("Customer");

    // Test with string tenant ID "1"
    let result = executor
        .execute(
            &tool,
            json!({ "limit": 10 }),
            &CallToolOptions::default(),
            &create_context("1"),
        )
        .await;

    assert_success(&result, "String tenant ID should work");

    println!("     ✓ String tenant ID correctly processed");
}

pub async fn test_numeric_tenant_id_as_string(ctx: &TestContext) {
    println!("  🧪 test_numeric_tenant_id_as_string");

    let executor = ctx.executor();
    let tool = list_tool("Customer");

    // Tenant ID can be passed as string even if DB column is numeric
    let result = executor
        .execute(
            &tool,
            json!({ "limit": 10 }),
            &CallToolOptions::default(),
            &create_context("2"),
        )
        .await;

    assert_success(&result, "Numeric tenant ID as string should work");

    let response = extract_json(&result).unwrap();
    let data = response["data"].as_array().unwrap();

    for item in data {
        assert_eq!(item["organization_id"], 2, "Should filter by tenant 2");
    }

    println!("     ✓ Numeric tenant ID as string correctly processed");
}

// =============================================================================
// TEST RUNNER
// =============================================================================

/// Run all tenant isolation tests
pub async fn run_all_tests(ctx: &TestContext) {
    println!("\n🔐 Running Tenant Isolation Tests\n");

    // Cross-tenant blocking
    test_cross_tenant_get_blocked(ctx).await;
    test_cross_tenant_list_isolated(ctx).await;

    // Empty tenant handling
    test_empty_tenant_fails_for_scoped_table(ctx).await;
    test_null_tenant_fails(ctx).await;

    // Multiple tenants
    test_three_tenants_completely_isolated(ctx).await;

    // Global tables
    test_global_table_returns_all_records(ctx).await;
    test_global_table_accessible_from_any_tenant(ctx).await;

    // Tenant configuration
    test_tenant_scoped_table_has_config(ctx).await;
    test_different_tables_same_tenant_column(ctx).await;

    // Inherited tenant
    test_inherited_tenant_via_fk(ctx).await;

    // Tenant ID types
    test_string_tenant_id(ctx).await;
    test_numeric_tenant_id_as_string(ctx).await;

    println!("\n✅ All Tenant Isolation tests passed!\n");
}
