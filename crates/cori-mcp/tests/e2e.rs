//! End-to-end tests using a Docker PostgreSQL container.
//!
//! This is the main test orchestrator that runs all test modules.
//!
//! Test modules are organized by feature area:
//! - `crud_operations` - GET/LIST operations, pagination, filtering, dry-run
//! - `tenant_isolation` - Multi-tenant scenarios, cross-tenant blocking, global tables
//! - `readable_columns` - ColumnList tests (All vs specific columns), blocked_tables
//! - `creatable_columns` - CreatableColumnConstraints: required, default, restrict_to, etc.
//! - `updatable_columns` - UpdatableColumnConstraints: restrict_to, transitions, etc.
//! - `deletable` - DeletablePermission: boolean, requires_approval, soft_delete
//! - `rules` - RulesDefinition: tenant config, global tables, soft_delete
//! - `approvals` - Human-in-the-loop approval workflow
//!
//! Run with:
//!   cargo test -p cori-mcp --test e2e -- --nocapture --test-threads=1
//!
//! Requirements:
//!   - Docker must be running
//!   - Port 5433 must be available (uses non-standard port to avoid conflicts)

// Test modules (located in e2e/ subdirectory)
#[path = "e2e/common/mod.rs"]
mod common;

#[path = "e2e/approvals.rs"]
mod approvals;

#[path = "e2e/creatable_columns.rs"]
mod creatable_columns;

#[path = "e2e/crud_operations.rs"]
mod crud_operations;

#[path = "e2e/deletable.rs"]
mod deletable;

#[path = "e2e/readable_columns.rs"]
mod readable_columns;

#[path = "e2e/rules.rs"]
mod rules;

#[path = "e2e/tenant_isolation.rs"]
mod tenant_isolation;

#[path = "e2e/updatable_columns.rs"]
mod updatable_columns;

use common::TestContext;

// =============================================================================
// MAIN TEST RUNNER
// =============================================================================

/// Run all E2E tests sequentially to share the Docker container.
///
/// This orchestrates all test modules in a single test run to avoid
/// starting/stopping Docker containers for each module.
#[tokio::test]
async fn e2e_all_tests() {
    println!("\n🚀 Starting Cori MCP End-to-End Tests\n");

    let ctx = match TestContext::setup().await {
        Ok(ctx) => ctx,
        Err(e) => {
            eprintln!("❌ Failed to setup test context: {}", e);
            eprintln!("   Make sure Docker is running and port 5433 is available");
            return;
        }
    };

    println!("\n📋 Running test modules...\n");

    // Run all test modules
    crud_operations::run_all_tests(&ctx).await;
    tenant_isolation::run_all_tests(&ctx).await;
    readable_columns::run_all_tests(&ctx).await;
    creatable_columns::run_all_tests(&ctx).await;
    updatable_columns::run_all_tests(&ctx).await;
    deletable::run_all_tests(&ctx).await;
    rules::run_all_tests(&ctx).await;
    approvals::run_all_tests(&ctx).await;

    println!("\n🎉 All E2E test modules passed!\n");
}
