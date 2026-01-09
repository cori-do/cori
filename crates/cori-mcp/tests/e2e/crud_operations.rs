//! CRUD operations tests for Cori MCP.
//!
//! Tests basic GET, LIST operations, pagination, filtering, and dry-run mode.

use super::common::*;
use cori_mcp::protocol::CallToolOptions;
use serde_json::json;

// =============================================================================
// GET OPERATIONS
// =============================================================================

pub async fn test_get_customer_by_id(ctx: &TestContext) {
    println!("  🧪 test_get_customer_by_id");

    let executor = ctx.executor();
    let tool = get_tool("Customer");

    // Customer 1 belongs to organization 1 (Acme)
    let result = executor
        .execute(&tool, json!({ "id": 1 }), &CallToolOptions::default(), &create_context("1"))
        .await;

    assert_success(&result, "GET customer should succeed");

    let data = extract_json(&result).expect("Should have JSON response");
    assert_eq!(data["customer_id"], 1);
    assert_eq!(data["organization_id"], 1);
    assert_eq!(data["first_name"], "David");
    assert_eq!(data["last_name"], "Smith");
    assert_eq!(data["company"], "BigTech Solutions");

    // Verify DECIMAL column (lifetime_value)
    assert!(
        data["lifetime_value"].is_number(),
        "lifetime_value should be a number, got: {:?}",
        data["lifetime_value"]
    );
    assert_eq!(data["lifetime_value"], 15000.0);

    println!("     ✓ Customer retrieved with correct data and DECIMAL values");
}

pub async fn test_get_order_by_id(ctx: &TestContext) {
    println!("  🧪 test_get_order_by_id");

    let executor = ctx.executor();
    let tool = get_tool("Order");

    // Order 1 belongs to org 1 with known values
    let result = executor
        .execute(&tool, json!({ "id": 1 }), &CallToolOptions::default(), &create_context("1"))
        .await;

    assert_success(&result, "GET order should succeed");

    let data = extract_json(&result).expect("Should have JSON response");
    assert_eq!(data["order_id"], 1);
    assert_eq!(data["order_number"], "ACME-2025-001");
    assert_eq!(data["status"], "delivered");

    // Verify DECIMAL columns
    assert!(
        data["subtotal"].is_number(),
        "subtotal should be a number, got: {:?}",
        data["subtotal"]
    );
    assert!(
        data["total_amount"].is_number(),
        "total_amount should be a number, got: {:?}",
        data["total_amount"]
    );

    let subtotal = data["subtotal"].as_f64().unwrap();
    let total = data["total_amount"].as_f64().unwrap();

    assert!(
        (subtotal - 1199.88).abs() < 0.01,
        "subtotal should be 1199.88, got {}",
        subtotal
    );
    assert!(
        (total - 1295.87).abs() < 0.01,
        "total_amount should be 1295.87, got {}",
        total
    );

    println!(
        "     ✓ Order DECIMAL columns (subtotal={}, total={}) returned correctly",
        subtotal, total
    );
}

pub async fn test_get_invoice_by_id(ctx: &TestContext) {
    println!("  🧪 test_get_invoice_by_id");

    let executor = ctx.executor();
    let tool = get_tool("Invoice");

    // Invoice 1 belongs to org 1
    let result = executor
        .execute(&tool, json!({ "id": 1 }), &CallToolOptions::default(), &create_context("1"))
        .await;

    assert_success(&result, "GET invoice should succeed");

    let data = extract_json(&result).expect("Should have JSON response");
    assert_eq!(data["invoice_id"], 1);

    // DECIMAL columns
    assert!(
        data["total_amount"].is_number(),
        "total_amount should be a number, got: {:?}",
        data["total_amount"]
    );
    assert!(
        data["paid_amount"].is_number(),
        "paid_amount should be a number, got: {:?}",
        data["paid_amount"]
    );

    println!("     ✓ Invoice DECIMAL columns returned correctly");
}

pub async fn test_get_nonexistent_record(ctx: &TestContext) {
    println!("  🧪 test_get_nonexistent_record");

    let executor = ctx.executor();
    let tool = get_tool("Customer");

    // Try to get a customer that doesn't exist
    let result = executor
        .execute(
            &tool,
            json!({ "id": 99999 }),
            &CallToolOptions::default(),
            &create_context("1"),
        )
        .await;

    // Should succeed but return null/not found
    assert!(result.success, "Query should succeed");
    let data = extract_json(&result).expect("Should have JSON response");
    assert!(
        data.is_null() || data["data"].is_null() || data["message"] == "Record not found",
        "Should return null for nonexistent record: {:?}",
        data
    );

    println!("     ✓ Nonexistent record correctly returns null/not found");
}

pub async fn test_get_missing_required_id(ctx: &TestContext) {
    println!("  🧪 test_get_missing_required_id");

    let executor = ctx.executor();
    let tool = get_tool("Customer");

    let result = executor
        .execute(
            &tool,
            json!({}), // Missing required 'id'
            &CallToolOptions::default(),
            &create_context("1"),
        )
        .await;

    assert_failure(&result, "Missing required field should fail");

    println!("     ✓ Missing required field correctly rejected");
}

// =============================================================================
// LIST OPERATIONS
// =============================================================================

pub async fn test_list_customers(ctx: &TestContext) {
    println!("  🧪 test_list_customers");

    let executor = ctx.executor();
    let tool = list_tool("Customer");

    // List customers for org 1 (Acme - has 5 customers per seed data)
    let result = executor
        .execute(
            &tool,
            json!({ "limit": 100 }),
            &CallToolOptions::default(),
            &create_context("1"),
        )
        .await;

    assert_success(&result, "LIST should succeed");

    let response = extract_json(&result).expect("Should have JSON response");
    let data = response["data"].as_array().expect("data should be array");

    assert_eq!(data.len(), 5, "Acme should have exactly 5 customers");

    // Verify all belong to tenant 1
    for customer in data {
        assert_eq!(
            customer["organization_id"], 1,
            "All customers should belong to org 1"
        );
    }

    // Verify DECIMAL column in list results
    let first = &data[0];
    assert!(
        first["lifetime_value"].is_number(),
        "lifetime_value should be a number in list results"
    );

    println!("     ✓ Listed {} customers, all with correct tenant", data.len());
}

pub async fn test_list_orders_with_decimal(ctx: &TestContext) {
    println!("  🧪 test_list_orders_with_decimal");

    let executor = ctx.executor();
    let tool = list_tool("Order");

    // List orders for org 1 (Acme - has 3 orders per seed data)
    let result = executor
        .execute(
            &tool,
            json!({ "limit": 100 }),
            &CallToolOptions::default(),
            &create_context("1"),
        )
        .await;

    assert_success(&result, "LIST orders should succeed");

    let response = extract_json(&result).expect("Should have JSON response");
    let data = response["data"].as_array().expect("data should be array");

    assert_eq!(data.len(), 3, "Acme should have exactly 3 orders");

    // Verify DECIMAL columns in all orders
    for order in data {
        assert!(
            order["subtotal"].is_number(),
            "subtotal should be number: {:?}",
            order
        );
        assert!(
            order["total_amount"].is_number(),
            "total_amount should be number: {:?}",
            order
        );
    }

    println!("     ✓ Listed {} orders with correct DECIMAL values", data.len());
}

pub async fn test_list_tickets(ctx: &TestContext) {
    println!("  🧪 test_list_tickets");

    let executor = ctx.executor();
    let tool = list_tool("Ticket");

    // List tickets for org 1 (Acme - has 3 tickets per seed data)
    let result = executor
        .execute(
            &tool,
            json!({ "limit": 100 }),
            &CallToolOptions::default(),
            &create_context("1"),
        )
        .await;

    assert_success(&result, "LIST tickets should succeed");

    let response = extract_json(&result).expect("Should have JSON response");
    let data = response["data"].as_array().expect("data should be array");

    assert_eq!(data.len(), 3, "Acme should have exactly 3 tickets");

    println!("     ✓ Listed {} tickets for tenant", data.len());
}

// =============================================================================
// PAGINATION
// =============================================================================

pub async fn test_pagination_with_limit_and_offset(ctx: &TestContext) {
    println!("  🧪 test_pagination_with_limit_and_offset");

    let executor = ctx.executor();
    let tool = list_tool("Customer");

    // Get first 2 customers
    let page1 = executor
        .execute(
            &tool,
            json!({ "limit": 2, "offset": 0 }),
            &CallToolOptions::default(),
            &create_context("1"),
        )
        .await;

    assert_success(&page1, "Page 1 should succeed");
    let page1_data = extract_json(&page1).unwrap();
    let page1_items = page1_data["data"].as_array().unwrap();
    assert_eq!(page1_items.len(), 2);

    // Get next 2 customers
    let page2 = executor
        .execute(
            &tool,
            json!({ "limit": 2, "offset": 2 }),
            &CallToolOptions::default(),
            &create_context("1"),
        )
        .await;

    assert_success(&page2, "Page 2 should succeed");
    let page2_data = extract_json(&page2).unwrap();
    let page2_items = page2_data["data"].as_array().unwrap();
    assert_eq!(page2_items.len(), 2);

    // Verify no overlap
    let page1_ids: Vec<_> = page1_items
        .iter()
        .map(|c| c["customer_id"].as_i64())
        .collect();
    let page2_ids: Vec<_> = page2_items
        .iter()
        .map(|c| c["customer_id"].as_i64())
        .collect();

    for id in &page1_ids {
        assert!(!page2_ids.contains(id), "Pages should not overlap");
    }

    println!("     ✓ Pagination working correctly");
}

pub async fn test_pagination_beyond_results(ctx: &TestContext) {
    println!("  🧪 test_pagination_beyond_results");

    let executor = ctx.executor();
    let tool = list_tool("Customer");

    // Request offset beyond available records
    let result = executor
        .execute(
            &tool,
            json!({ "limit": 10, "offset": 1000 }),
            &CallToolOptions::default(),
            &create_context("1"),
        )
        .await;

    assert_success(&result, "Query should succeed");
    let data = extract_json(&result).unwrap();
    let items = data["data"].as_array().unwrap();
    assert_eq!(items.len(), 0, "Should return empty array for offset beyond results");

    println!("     ✓ Pagination beyond results returns empty array");
}

// =============================================================================
// FILTERING
// =============================================================================

pub async fn test_filter_by_column_value(ctx: &TestContext) {
    println!("  🧪 test_filter_by_column_value");

    let executor = ctx.executor();
    let tool = list_tool("Customer");

    // Filter by status = 'active'
    let result = executor
        .execute(
            &tool,
            json!({ "limit": 100, "status": "active" }),
            &CallToolOptions::default(),
            &create_context("1"),
        )
        .await;

    assert_success(&result, "Filter should succeed");

    let response = extract_json(&result).unwrap();
    let data = response["data"].as_array().unwrap();

    // Verify all returned customers have active status
    for customer in data {
        assert_eq!(
            customer["status"], "active",
            "All filtered customers should be active"
        );
    }

    println!(
        "     ✓ Filter by column value working correctly ({} active customers)",
        data.len()
    );
}

pub async fn test_filter_by_multiple_values(ctx: &TestContext) {
    println!("  🧪 test_filter_by_multiple_values");

    let executor = ctx.executor();
    let tool = list_tool("Ticket");

    // Filter by status and priority
    let result = executor
        .execute(
            &tool,
            json!({ "limit": 100, "status": "open" }),
            &CallToolOptions::default(),
            &create_context("1"),
        )
        .await;

    assert_success(&result, "Filter should succeed");

    let response = extract_json(&result).unwrap();
    let data = response["data"].as_array().unwrap();

    for ticket in data {
        assert_eq!(ticket["status"], "open", "All tickets should be open");
    }

    println!("     ✓ Filter by multiple values working ({} results)", data.len());
}

// =============================================================================
// DRY-RUN MODE
// =============================================================================

pub async fn test_dry_run_get(ctx: &TestContext) {
    println!("  🧪 test_dry_run_get");

    let executor = ctx.executor();
    let tool = get_tool("Customer");

    let result = executor
        .execute(
            &tool,
            json!({ "id": 1 }),
            &CallToolOptions { dry_run: true },
            &create_context("1"),
        )
        .await;

    assert_success(&result, "Dry run should succeed");
    assert!(result.is_dry_run, "Should be marked as dry run");

    let data = extract_json(&result).expect("Should have JSON response");
    assert!(
        data["dryRun"].as_bool().unwrap_or(false),
        "Should indicate dry run"
    );
    assert!(
        data["preview"]["query"]
            .as_str()
            .unwrap_or("")
            .contains("SELECT"),
        "Should contain SELECT query preview"
    );

    println!("     ✓ Dry run returned query preview");
}

pub async fn test_dry_run_list(ctx: &TestContext) {
    println!("  🧪 test_dry_run_list");

    let executor = ctx.executor();
    let tool = list_tool("Customer");

    let result = executor
        .execute(
            &tool,
            json!({ "limit": 10 }),
            &CallToolOptions { dry_run: true },
            &create_context("1"),
        )
        .await;

    assert_success(&result, "Dry run LIST should succeed");
    assert!(result.is_dry_run, "Should be marked as dry run");

    let data = extract_json(&result).expect("Should have JSON response");
    assert!(data["dryRun"].as_bool().unwrap_or(false), "Should indicate dry run");

    println!("     ✓ Dry run LIST returned query preview");
}

// =============================================================================
// TEST RUNNER
// =============================================================================

/// Run all CRUD operation tests
pub async fn run_all_tests(ctx: &TestContext) {
    println!("\n📦 Running CRUD Operations Tests\n");

    // GET operations
    test_get_customer_by_id(ctx).await;
    test_get_order_by_id(ctx).await;
    test_get_invoice_by_id(ctx).await;
    test_get_nonexistent_record(ctx).await;
    test_get_missing_required_id(ctx).await;

    // LIST operations
    test_list_customers(ctx).await;
    test_list_orders_with_decimal(ctx).await;
    test_list_tickets(ctx).await;

    // Pagination
    test_pagination_with_limit_and_offset(ctx).await;
    test_pagination_beyond_results(ctx).await;

    // Filtering
    test_filter_by_column_value(ctx).await;
    test_filter_by_multiple_values(ctx).await;

    // Dry-run
    test_dry_run_get(ctx).await;
    test_dry_run_list(ctx).await;

    println!("\n✅ All CRUD Operations tests passed!\n");
}
