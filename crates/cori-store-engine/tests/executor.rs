// SPDX-License-Identifier: Apache-2.0
//! The executor end to end on Postgres: every generated tool kind under the role and rules checks, against a demo
//! schema. Needs CORI_STORE_DATABASE_URL (an admin URL: the `cori_executor_test` database is dropped and created again
//! on its server); skipped without it.
//!
//! Test modules are organized by feature area:
//! - `crud_operations` - GET/LIST operations, pagination, filtering, dry-run
//! - `tenant_isolation` - Multi-tenant scenarios, cross-tenant blocking, global tables
//! - `readable_columns` - ColumnList tests (All vs specific columns), tables not in role
//! - `creatable_columns` - CreatableColumnConstraints: required, default, restrict_to, etc.
//! - `updatable_columns` - UpdatableColumnConstraints: restrict_to, transitions, etc.
//! - `deletable` - DeletablePermission: boolean, requires_approval, soft_delete
//! - `rules` - RulesDefinition: tenant config, global tables, soft_delete
//! - `approvals` - Human-in-the-loop approval workflow
//! - `validation` - ToolValidator: role/rules validation before execution

// Test modules (located in executor/)
#[path = "executor/common/mod.rs"]
mod common;

#[path = "executor/approvals.rs"]
mod approvals;

#[path = "executor/creatable_columns.rs"]
mod creatable_columns;

#[path = "executor/crud_operations.rs"]
mod crud_operations;

#[path = "executor/deletable.rs"]
mod deletable;

#[path = "executor/readable_columns.rs"]
mod readable_columns;

#[path = "executor/rules.rs"]
mod rules;

#[path = "executor/tenant_isolation.rs"]
mod tenant_isolation;

#[path = "executor/updatable_columns.rs"]
mod updatable_columns;

#[path = "executor/validation.rs"]
mod validation;

use common::TestContext;

// =============================================================================
// MAIN TEST RUNNER
// =============================================================================

/// Runs every test module in sequence on one test database.
#[tokio::test]
async fn executor_end_to_end() {
    let Ok(admin_url) = std::env::var("CORI_STORE_DATABASE_URL") else { eprintln!("CORI_STORE_DATABASE_URL not set: skipped"); return };
    // with a database configured, a setup failure fails the suite: it never goes green without testing anything
    let ctx = TestContext::setup(&admin_url).await.unwrap_or_else(|e| panic!("executor test database: {e}"));

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
    validation::run_all_tests(&ctx).await;
}
