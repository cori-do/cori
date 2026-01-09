//! Shared test infrastructure for Cori MCP end-to-end tests.
//!
//! This module provides:
//! - Docker container management for PostgreSQL
//! - Test fixtures (roles, rules, contexts)
//! - Helper functions for test assertions

use cori_core::config::role_definition::{
    ApprovalConfig, ApprovalRequirement, ColumnList, CreatableColumnConstraints, CreatableColumns,
    DeletablePermission, RoleDefinition, TablePermissions,
    UpdatableColumnConstraints, UpdatableColumns,
};
use cori_core::config::rules_definition::{
    RulesDefinition, TableRules,
    TenantConfig,
};
use cori_mcp::approval::ApprovalManager;
use cori_mcp::executor::{ExecutionContext, ToolExecutor};
use cori_mcp::protocol::{ToolContent, ToolDefinition};
use serde_json::{json, Value};
use sqlx::PgPool;
use std::collections::HashMap;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

// =============================================================================
// DOCKER CONTAINER CONFIGURATION
// =============================================================================

pub const CONTAINER_NAME: &str = "cori_test_postgres";
pub const POSTGRES_PORT: u16 = 5433;
pub const POSTGRES_PASSWORD: &str = "cori_test_password";
pub const DATABASE_NAME: &str = "cori_test";

pub fn database_url() -> String {
    format!(
        "postgres://postgres:{}@localhost:{}/{}",
        POSTGRES_PASSWORD, POSTGRES_PORT, DATABASE_NAME
    )
}

// =============================================================================
// DOCKER CONTAINER MANAGEMENT
// =============================================================================

/// Start a PostgreSQL container for testing
pub fn start_postgres_container() -> Result<(), String> {
    let output = Command::new("docker")
        .args(["ps", "-a", "-q", "-f", &format!("name={}", CONTAINER_NAME)])
        .output()
        .map_err(|e| format!("Failed to check existing container: {}", e))?;

    let container_exists = !String::from_utf8_lossy(&output.stdout).trim().is_empty();

    if container_exists {
        let _ = Command::new("docker")
            .args(["rm", "-f", CONTAINER_NAME])
            .output();
    }

    let status = Command::new("docker")
        .args([
            "run",
            "-d",
            "--name",
            CONTAINER_NAME,
            "-e",
            &format!("POSTGRES_PASSWORD={}", POSTGRES_PASSWORD),
            "-e",
            &format!("POSTGRES_DB={}", DATABASE_NAME),
            "-p",
            &format!("{}:5432", POSTGRES_PORT),
            "postgres:16-alpine",
        ])
        .status()
        .map_err(|e| format!("Failed to start container: {}", e))?;

    if !status.success() {
        return Err("Failed to start PostgreSQL container".to_string());
    }

    Ok(())
}

/// Stop and remove the PostgreSQL container
pub fn stop_postgres_container() {
    let _ = Command::new("docker")
        .args(["rm", "-f", CONTAINER_NAME])
        .output();
}

/// Wait for PostgreSQL to be ready
pub async fn wait_for_postgres() -> Result<PgPool, String> {
    for attempt in 1..=30 {
        match PgPool::connect(&database_url()).await {
            Ok(pool) => {
                if sqlx::query("SELECT 1").fetch_one(&pool).await.is_ok() {
                    println!("✅ PostgreSQL ready after {} attempts", attempt);
                    return Ok(pool);
                }
            }
            Err(_) => {
                if attempt % 5 == 0 {
                    println!("⏳ Waiting for PostgreSQL... (attempt {})", attempt);
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    Err("PostgreSQL did not become ready in time".to_string())
}

// =============================================================================
// DATABASE INITIALIZATION
// =============================================================================

const SCHEMA_SQL: &str = include_str!("../../../../../examples/demo/database/schema.sql");
const SEED_SQL: &str = include_str!("../../../../../examples/demo/database/seed.sql");

pub async fn initialize_database(pool: &PgPool) -> Result<(), sqlx::Error> {
    sqlx::raw_sql(SCHEMA_SQL).execute(pool).await?;
    sqlx::raw_sql(SEED_SQL).execute(pool).await?;
    println!("✅ Database initialized with schema and seed data");
    Ok(())
}

// =============================================================================
// TEST CONTEXT
// =============================================================================

pub struct TestContext {
    pub pool: PgPool,
}

impl TestContext {
    pub async fn setup() -> Result<Self, String> {
        start_postgres_container()?;
        let pool = wait_for_postgres().await?;
        initialize_database(&pool)
            .await
            .map_err(|e| format!("Failed to initialize database: {}", e))?;
        Ok(Self { pool })
    }

    /// Create an executor with a custom role and rules
    pub fn executor_with(
        &self,
        role: RoleDefinition,
        rules: RulesDefinition,
    ) -> ToolExecutor {
        let approval_manager = Arc::new(ApprovalManager::default());
        ToolExecutor::new(role, approval_manager)
            .with_pool(self.pool.clone())
            .with_rules(rules)
    }

    /// Create an executor with the default support agent role
    pub fn executor(&self) -> ToolExecutor {
        self.executor_with(create_support_agent_role(), create_default_rules())
    }
}

impl Drop for TestContext {
    fn drop(&mut self) {
        stop_postgres_container();
        println!("🧹 Cleaned up PostgreSQL container");
    }
}

// =============================================================================
// EXECUTION CONTEXT HELPERS
// =============================================================================

/// Create an execution context for a specific tenant
pub fn create_context(tenant_id: &str) -> ExecutionContext {
    create_context_with_role(tenant_id, "support_agent")
}

/// Create an execution context with a specific role
pub fn create_context_with_role(tenant_id: &str, role: &str) -> ExecutionContext {
    ExecutionContext {
        tenant_id: tenant_id.to_string(),
        role: role.to_string(),
        connection_id: Some("e2e-test".to_string()),
    }
}

// =============================================================================
// RESULT HELPERS
// =============================================================================

/// Extract JSON from execution result
pub fn extract_json(result: &cori_mcp::executor::ExecutionResult) -> Option<Value> {
    result.content.first().and_then(|c| match c {
        ToolContent::Json { json } => Some(json.clone()),
        ToolContent::Text { text } => serde_json::from_str(text).ok(),
    })
}

/// Assert that a result is successful
pub fn assert_success(result: &cori_mcp::executor::ExecutionResult, msg: &str) {
    assert!(result.success, "{}: {:?}", msg, result);
}

/// Assert that a result is a failure
pub fn assert_failure(result: &cori_mcp::executor::ExecutionResult, msg: &str) {
    assert!(!result.success, "{}: {:?}", msg, result);
}

// =============================================================================
// TOOL DEFINITION BUILDERS
// =============================================================================

/// Create a GET tool definition for a table
pub fn get_tool(table: &str) -> ToolDefinition {
    let name = format!("get{}", capitalize_first(table));
    ToolDefinition {
        name,
        description: Some(format!("Get {} by ID", table)),
        input_schema: json!({
            "type": "object",
            "properties": { "id": { "type": "integer" } },
            "required": ["id"]
        }),
        annotations: None,
    }
}

/// Create a LIST tool definition for a table
pub fn list_tool(table: &str) -> ToolDefinition {
    let name = format!("list{}", capitalize_first(&pluralize(table)));
    ToolDefinition {
        name,
        description: Some(format!("List {}", table)),
        input_schema: json!({
            "type": "object",
            "properties": {
                "limit": { "type": "integer" },
                "offset": { "type": "integer" }
            }
        }),
        annotations: None,
    }
}

/// Create a CREATE tool definition for a table
pub fn create_tool(table: &str, properties: Value) -> ToolDefinition {
    let name = format!("create{}", capitalize_first(table));
    ToolDefinition {
        name,
        description: Some(format!("Create {}", table)),
        input_schema: json!({
            "type": "object",
            "properties": properties
        }),
        annotations: None,
    }
}

/// Create an UPDATE tool definition for a table
pub fn update_tool(table: &str, properties: Value) -> ToolDefinition {
    let name = format!("update{}", capitalize_first(table));
    ToolDefinition {
        name,
        description: Some(format!("Update {}", table)),
        input_schema: json!({
            "type": "object",
            "properties": properties,
            "required": ["id"]
        }),
        annotations: None,
    }
}

/// Create a DELETE tool definition for a table
pub fn delete_tool(table: &str) -> ToolDefinition {
    let name = format!("delete{}", capitalize_first(table));
    ToolDefinition {
        name,
        description: Some(format!("Delete {}", table)),
        input_schema: json!({
            "type": "object",
            "properties": { "id": { "type": "integer" } },
            "required": ["id"]
        }),
        annotations: None,
    }
}

fn capitalize_first(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        None => String::new(),
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
    }
}

fn pluralize(s: &str) -> String {
    // Simple pluralization - add 's' or 'es'
    if s.ends_with('s') || s.ends_with('x') || s.ends_with("ch") || s.ends_with("sh") {
        format!("{}es", s)
    } else if s.ends_with('y') {
        format!("{}ies", &s[..s.len() - 1])
    } else {
        format!("{}s", s)
    }
}

// =============================================================================
// DEFAULT RULES DEFINITION
// =============================================================================

pub fn create_default_rules() -> RulesDefinition {
    let mut tables = HashMap::new();

    // Standard tenant-scoped tables with direct organization_id
    for table in [
        "customers",
        "orders",
        "order_items",
        "tickets",
        "notes",
        "contacts",
        "customer_tags",
        "products",
        "invoices",
    ] {
        tables.insert(
            table.to_string(),
            TableRules {
                description: None,
                tenant: Some(TenantConfig::Direct("organization_id".to_string())),
                global: None,
                soft_delete: None,
                columns: HashMap::new(),
            },
        );
    }

    // Global tables (not tenant-scoped)
    tables.insert(
        "currencies".to_string(),
        TableRules {
            description: Some("Currency reference table".to_string()),
            tenant: None,
            global: Some(true),
            soft_delete: None,
            columns: HashMap::new(),
        },
    );

    RulesDefinition {
        version: "1.0.0".to_string(),
        tables,
    }
}

/// Create rules with a global table
pub fn create_rules_with_global_table(global_table: &str) -> RulesDefinition {
    let mut rules = create_default_rules();
    rules.tables.insert(
        global_table.to_string(),
        TableRules {
            description: Some(format!("Global {} table", global_table)),
            tenant: None,
            global: Some(true),
            soft_delete: None,
            columns: HashMap::new(),
        },
    );
    rules
}
// =============================================================================
// ROLE DEFINITION BUILDERS
// =============================================================================

/// Create the default support agent role
pub fn create_support_agent_role() -> RoleDefinition {
    let mut tables = HashMap::new();

    // Customers - read only
    tables.insert(
        "customers".to_string(),
        TablePermissions {
            readable: ColumnList::List(vec![
                "customer_id".to_string(),
                "organization_id".to_string(),
                "first_name".to_string(),
                "last_name".to_string(),
                "email".to_string(),
                "company".to_string(),
                "status".to_string(),
                "lifetime_value".to_string(),
                "created_at".to_string(),
                "updated_at".to_string(),
            ]),
            creatable: CreatableColumns::default(),
            updatable: UpdatableColumns::default(),
            deletable: DeletablePermission::default(),
        },
    );

    // Orders - read only with DECIMAL columns
    tables.insert(
        "orders".to_string(),
        TablePermissions {
            readable: ColumnList::List(vec![
                "order_id".to_string(),
                "organization_id".to_string(),
                "customer_id".to_string(),
                "order_number".to_string(),
                "status".to_string(),
                "subtotal".to_string(),
                "total_amount".to_string(),
                "shipping_cost".to_string(),
                "tax_amount".to_string(),
                "discount_amount".to_string(),
                "order_date".to_string(),
                "created_at".to_string(),
                "updated_at".to_string(),
            ]),
            creatable: CreatableColumns::default(),
            updatable: UpdatableColumns::default(),
            deletable: DeletablePermission::default(),
        },
    );

    // Order items - read only
    tables.insert(
        "order_items".to_string(),
        TablePermissions {
            readable: ColumnList::List(vec![
                "order_item_id".to_string(),
                "organization_id".to_string(),
                "order_id".to_string(),
                "product_id".to_string(),
                "quantity".to_string(),
                "unit_price".to_string(),
                "discount_percentage".to_string(),
                "line_total".to_string(),
                "created_at".to_string(),
            ]),
            creatable: CreatableColumns::default(),
            updatable: UpdatableColumns::default(),
            deletable: DeletablePermission::default(),
        },
    );

    // Products - read only
    tables.insert(
        "products".to_string(),
        TablePermissions {
            readable: ColumnList::List(vec![
                "product_id".to_string(),
                "organization_id".to_string(),
                "name".to_string(),
                "description".to_string(),
                "sku".to_string(),
                "price".to_string(),
                "category".to_string(),
                "stock_quantity".to_string(),
                "is_active".to_string(),
                "created_at".to_string(),
            ]),
            creatable: CreatableColumns::default(),
            updatable: UpdatableColumns::default(),
            deletable: DeletablePermission::default(),
        },
    );

    // Tickets - read and update status
    tables.insert(
        "tickets".to_string(),
        TablePermissions {
            readable: ColumnList::List(vec![
                "ticket_id".to_string(),
                "organization_id".to_string(),
                "customer_id".to_string(),
                "ticket_number".to_string(),
                "subject".to_string(),
                "description".to_string(),
                "status".to_string(),
                "priority".to_string(),
                "category".to_string(),
                "created_at".to_string(),
            ]),
            creatable: CreatableColumns::default(),
            updatable: UpdatableColumns::Map(HashMap::from([(
                "status".to_string(),
                UpdatableColumnConstraints {
                    restrict_to: Some(vec![
                        json!("open"),
                        json!("in_progress"),
                        json!("pending_customer"),
                        json!("resolved"),
                        json!("closed"),
                    ]),
                    ..Default::default()
                },
            )])),
            deletable: DeletablePermission::Allowed(true),  // Allow deleting tickets
        },
    );

    // Notes - read and create
    tables.insert(
        "notes".to_string(),
        TablePermissions {
            readable: ColumnList::List(vec![
                "note_id".to_string(),
                "organization_id".to_string(),
                "customer_id".to_string(),
                "content".to_string(),
                "is_internal".to_string(),
                "created_at".to_string(),
            ]),
            creatable: CreatableColumns::Map(HashMap::from([
                (
                    "customer_id".to_string(),
                    CreatableColumnConstraints {
                        required: true,
                        ..Default::default()
                    },
                ),
                (
                    "content".to_string(),
                    CreatableColumnConstraints {
                        required: true,
                        ..Default::default()
                    },
                ),
                (
                    "is_internal".to_string(),
                    CreatableColumnConstraints {
                        default: Some(json!(false)),
                        ..Default::default()
                    },
                ),
                (
                    "created_by".to_string(),
                    CreatableColumnConstraints {
                        // Default to user_id 2 (Alice Johnson from org 1)
                        default: Some(json!(2)),
                        ..Default::default()
                    },
                ),
            ])),
            updatable: UpdatableColumns::default(),
            deletable: DeletablePermission::default(),
        },
    );

    // Contacts - read only
    tables.insert(
        "contacts".to_string(),
        TablePermissions {
            readable: ColumnList::List(vec![
                "contact_id".to_string(),
                "organization_id".to_string(),
                "customer_id".to_string(),
                "first_name".to_string(),
                "last_name".to_string(),
                "position".to_string(),
                "email".to_string(),
                "phone".to_string(),
                "is_primary".to_string(),
                "created_at".to_string(),
            ]),
            creatable: CreatableColumns::default(),
            updatable: UpdatableColumns::default(),
            deletable: DeletablePermission::default(),
        },
    );

    // Customer tags - read only
    tables.insert(
        "customer_tags".to_string(),
        TablePermissions {
            readable: ColumnList::List(vec![
                "customer_id".to_string(),
                "tag_id".to_string(),
                "organization_id".to_string(),
                "created_at".to_string(),
            ]),
            creatable: CreatableColumns::default(),
            updatable: UpdatableColumns::default(),
            deletable: DeletablePermission::default(),
        },
    );

    // Invoices - read only with DECIMAL columns
    tables.insert(
        "invoices".to_string(),
        TablePermissions {
            readable: ColumnList::List(vec![
                "invoice_id".to_string(),
                "organization_id".to_string(),
                "order_id".to_string(),
                "invoice_number".to_string(),
                "invoice_date".to_string(),
                "due_date".to_string(),
                "status".to_string(),
                "total_amount".to_string(),
                "paid_amount".to_string(),
                "created_at".to_string(),
            ]),
            creatable: CreatableColumns::default(),
            updatable: UpdatableColumns::default(),
            deletable: DeletablePermission::default(),
        },
    );

    RoleDefinition {
        name: "support_agent".to_string(),
        description: Some("Support agent with read access and ticket updates".to_string()),
        approvals: Some(ApprovalConfig {
            group: "support_managers".to_string(),
            notify_on_pending: true,
            message: Some("Support action requires manager approval".to_string()),
        }),
        tables,
        blocked_tables: vec![
            "users".to_string(),
            "api_keys".to_string(),
            "billing".to_string(),
            "audit_logs".to_string(),
        ],
        max_rows_per_query: Some(100),
        max_affected_rows: Some(10),
    }
}

/// Create a role with all columns readable ("*")
pub fn create_role_with_all_readable(table: &str) -> RoleDefinition {
    let mut tables = HashMap::new();
    tables.insert(
        table.to_string(),
        TablePermissions {
            readable: ColumnList::All(cori_core::config::role_definition::AllColumns),
            creatable: CreatableColumns::default(),
            updatable: UpdatableColumns::default(),
            deletable: DeletablePermission::default(),
        },
    );

    RoleDefinition {
        name: "all_readable_role".to_string(),
        description: Some("Role with all columns readable".to_string()),
        approvals: None,
        tables,
        blocked_tables: Vec::new(),
        max_rows_per_query: Some(100),
        max_affected_rows: Some(10),
    }
}

/// Create a role with creatable columns and constraints
pub fn create_role_with_creatable(
    table: &str,
    creatable: HashMap<String, CreatableColumnConstraints>,
    readable: Vec<String>,
) -> RoleDefinition {
    let mut tables = HashMap::new();
    tables.insert(
        table.to_string(),
        TablePermissions {
            readable: ColumnList::List(readable),
            creatable: CreatableColumns::Map(creatable),
            updatable: UpdatableColumns::default(),
            deletable: DeletablePermission::default(),
        },
    );

    RoleDefinition {
        name: "creatable_role".to_string(),
        description: Some("Role with creatable columns".to_string()),
        approvals: None,
        tables,
        blocked_tables: Vec::new(),
        max_rows_per_query: Some(100),
        max_affected_rows: Some(10),
    }
}

/// Create a role with updatable columns and constraints
pub fn create_role_with_updatable(
    table: &str,
    updatable: HashMap<String, UpdatableColumnConstraints>,
    readable: Vec<String>,
) -> RoleDefinition {
    let mut tables = HashMap::new();
    tables.insert(
        table.to_string(),
        TablePermissions {
            readable: ColumnList::List(readable),
            creatable: CreatableColumns::default(),
            updatable: UpdatableColumns::Map(updatable),
            deletable: DeletablePermission::default(),
        },
    );

    RoleDefinition {
        name: "updatable_role".to_string(),
        description: Some("Role with updatable columns".to_string()),
        approvals: None,
        tables,
        blocked_tables: Vec::new(),
        max_rows_per_query: Some(100),
        max_affected_rows: Some(10),
    }
}

/// Create a role that has both tables with and without approval columns
pub fn create_role_with_approval_columns() -> RoleDefinition {
    let mut tables = HashMap::new();

    // tickets table - has a column that requires approval
    tables.insert(
        "tickets".to_string(),
        TablePermissions {
            readable: ColumnList::All(cori_core::config::role_definition::AllColumns),
            creatable: CreatableColumns::default(),
            updatable: UpdatableColumns::Map(HashMap::from([(
                "priority".to_string(),
                UpdatableColumnConstraints {
                    requires_approval: Some(ApprovalRequirement::Simple(true)),
                    ..Default::default()
                },
            )])),
            deletable: DeletablePermission::default(),
        },
    );

    // customers table - no approval columns
    tables.insert(
        "customers".to_string(),
        TablePermissions {
            readable: ColumnList::All(cori_core::config::role_definition::AllColumns),
            creatable: CreatableColumns::default(),
            updatable: UpdatableColumns::Map(HashMap::new()),
            deletable: DeletablePermission::default(),
        },
    );

    RoleDefinition {
        name: "mixed_approval_role".to_string(),
        description: Some("Role with some approval columns".to_string()),
        approvals: None,
        tables,
        blocked_tables: Vec::new(),
        max_rows_per_query: Some(100),
        max_affected_rows: Some(10),
    }
}

/// Create a role with max_affected_rows limit
pub fn create_role_with_max_affected(max_affected: u64) -> RoleDefinition {
    let mut role = create_support_agent_role();
    role.max_affected_rows = Some(max_affected);
    role
}
