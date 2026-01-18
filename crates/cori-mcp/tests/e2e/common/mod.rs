//! Shared test infrastructure for Cori MCP end-to-end tests.
//!
//! This module provides:
//! - Docker container management for PostgreSQL
//! - Test fixtures (roles, rules, contexts)
//! - Helper functions for test assertions

use cori_core::config::role_definition::{
    ApprovalConfig, ApprovalRequirement, ColumnCondition, CreatableColumnConstraints,
    CreatableColumns, DeletablePermission, OnlyWhen, ReadableConfig, RoleDefinition,
    TablePermissions, UpdatableColumnConstraints, UpdatableColumns,
};
use cori_core::config::rules_definition::{RulesDefinition, TableRules, TenantConfig};
use cori_mcp::approval::ApprovalManager;
use cori_mcp::executor::{ExecutionContext, ToolExecutor};
use cori_mcp::protocol::{ToolContent, ToolDefinition};
use cori_mcp::schema::{ColumnSchema, DatabaseSchema, TableSchema};
use serde_json::{Value, json};
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
    pub fn executor_with(&self, role: RoleDefinition, rules: RulesDefinition) -> ToolExecutor {
        let approval_manager = Arc::new(ApprovalManager::default());
        ToolExecutor::new(role, approval_manager)
            .with_pool(self.pool.clone())
            .with_rules(rules)
            .with_schema(create_test_schema())
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

/// Get the primary key column name for a table (e.g., Customer -> customer_id)
fn pk_column_for(entity: &str) -> String {
    // Convert entity name to lowercase and singularize to get PK column name
    // e.g., "Customer" -> "customer" -> "customer_id"
    // e.g., "Customers" -> "customers" -> "customer" -> "customer_id"
    let lower = entity.to_lowercase();
    format!("{}_id", singularize(&lower))
}

/// Singularize a table name (e.g., customers -> customer)
fn singularize(s: &str) -> String {
    if s.ends_with("ies") {
        format!("{}y", &s[..s.len() - 3])
    } else if s.ends_with("es") {
        // Handle cases like "addresses" -> "address"
        let base = &s[..s.len() - 2];
        if base.ends_with("ss")
            || base.ends_with("ch")
            || base.ends_with("sh")
            || base.ends_with("x")
        {
            base.to_string()
        } else {
            s[..s.len() - 1].to_string()
        }
    } else if s.ends_with('s') && s.len() > 1 {
        s[..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

/// Create a GET tool definition for a table
pub fn get_tool(entity: &str) -> ToolDefinition {
    let name = format!("get{}", capitalize_first(entity));
    let pk_col = pk_column_for(entity);
    ToolDefinition {
        name,
        description: Some(format!("Get {} by primary key", entity)),
        input_schema: json!({
            "type": "object",
            "properties": { pk_col.clone(): { "type": "integer" } },
            "required": [pk_col]
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
pub fn update_tool(entity: &str, properties: Value) -> ToolDefinition {
    let name = format!("update{}", capitalize_first(entity));
    let pk_col = pk_column_for(entity);
    ToolDefinition {
        name,
        description: Some(format!("Update {}", entity)),
        input_schema: json!({
            "type": "object",
            "properties": properties,
            "required": [pk_col]
        }),
        annotations: None,
    }
}

/// Create a DELETE tool definition for a table
pub fn delete_tool(entity: &str) -> ToolDefinition {
    let name = format!("delete{}", capitalize_first(entity));
    let pk_col = pk_column_for(entity);
    ToolDefinition {
        name,
        description: Some(format!("Delete {}", entity)),
        input_schema: json!({
            "type": "object",
            "properties": { pk_col.clone(): { "type": "integer" } },
            "required": [pk_col]
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
            readable: ReadableConfig::List(vec![
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
            readable: ReadableConfig::List(vec![
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
            readable: ReadableConfig::List(vec![
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
            readable: ReadableConfig::List(vec![
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
            readable: ReadableConfig::List(vec![
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
                    only_when: Some(OnlyWhen::Single(HashMap::from([(
                        "new.status".to_string(),
                        ColumnCondition::In(vec![
                            json!("open"),
                            json!("in_progress"),
                            json!("pending_customer"),
                            json!("resolved"),
                            json!("closed"),
                        ]),
                    )]))),
                    ..Default::default()
                },
            )])),
            deletable: DeletablePermission::Allowed(true), // Allow deleting tickets
        },
    );

    // Notes - read and create
    tables.insert(
        "notes".to_string(),
        TablePermissions {
            readable: ReadableConfig::List(vec![
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
            readable: ReadableConfig::List(vec![
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
            readable: ReadableConfig::List(vec![
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
            readable: ReadableConfig::List(vec![
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
    }
}

/// Create a role with all columns readable ("*")
pub fn create_role_with_all_readable(table: &str) -> RoleDefinition {
    let mut tables = HashMap::new();
    tables.insert(
        table.to_string(),
        TablePermissions {
            readable: ReadableConfig::All(cori_core::config::role_definition::AllColumns),
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
            readable: ReadableConfig::List(readable),
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
            readable: ReadableConfig::List(readable),
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
    }
}

/// Create a role that has both tables with and without approval columns
pub fn create_role_with_approval_columns() -> RoleDefinition {
    let mut tables = HashMap::new();

    // tickets table - has a column that requires approval
    tables.insert(
        "tickets".to_string(),
        TablePermissions {
            readable: ReadableConfig::All(cori_core::config::role_definition::AllColumns),
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
            readable: ReadableConfig::All(cori_core::config::role_definition::AllColumns),
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
    }
}

// =============================================================================
// DATABASE SCHEMA FOR E2E TESTS
// =============================================================================

/// Create the database schema for E2E tests.
/// This maps to the demo schema.sql tables.
pub fn create_test_schema() -> DatabaseSchema {
    let mut schema = DatabaseSchema::new();

    // Organizations table
    let mut organizations = TableSchema::new("organizations");
    organizations.add_column(ColumnSchema::new("organization_id", "integer"));
    organizations.add_column(ColumnSchema::new("name", "varchar"));
    organizations.add_column(ColumnSchema::new("slug", "varchar"));
    organizations.add_column(ColumnSchema::new("plan", "varchar"));
    organizations
        .primary_key
        .push("organization_id".to_string());
    schema.add_table(organizations);

    // Customers table
    let mut customers = TableSchema::new("customers");
    customers.add_column(ColumnSchema::new("customer_id", "integer"));
    customers.add_column(ColumnSchema::new("organization_id", "integer"));
    customers.add_column(ColumnSchema::new("first_name", "varchar"));
    customers.add_column(ColumnSchema::new("last_name", "varchar"));
    customers.add_column(ColumnSchema::new("email", "varchar"));
    customers.add_column(ColumnSchema::new("phone", "varchar"));
    customers.add_column(ColumnSchema::new("company", "varchar"));
    customers.add_column(ColumnSchema::new("status", "varchar"));
    customers.add_column(ColumnSchema::new("notes", "text"));
    customers.add_column(ColumnSchema::new("lifetime_value", "numeric"));
    customers.add_column(ColumnSchema::new("created_at", "timestamp"));
    customers.add_column(ColumnSchema::new("updated_at", "timestamp"));
    customers.primary_key.push("customer_id".to_string());
    schema.add_table(customers);

    // Orders table
    let mut orders = TableSchema::new("orders");
    orders.add_column(ColumnSchema::new("order_id", "integer"));
    orders.add_column(ColumnSchema::new("organization_id", "integer"));
    orders.add_column(ColumnSchema::new("customer_id", "integer"));
    orders.add_column(ColumnSchema::new("order_number", "varchar"));
    orders.add_column(ColumnSchema::new("status", "varchar"));
    orders.add_column(ColumnSchema::new("subtotal", "numeric"));
    orders.add_column(ColumnSchema::new("total_amount", "numeric"));
    orders.add_column(ColumnSchema::new("shipping_cost", "numeric"));
    orders.add_column(ColumnSchema::new("tax_amount", "numeric"));
    orders.add_column(ColumnSchema::new("discount_amount", "numeric"));
    orders.add_column(ColumnSchema::new("order_date", "date"));
    orders.add_column(ColumnSchema::new("created_at", "timestamp"));
    orders.add_column(ColumnSchema::new("updated_at", "timestamp"));
    orders.primary_key.push("order_id".to_string());
    schema.add_table(orders);

    // Tickets table
    let mut tickets = TableSchema::new("tickets");
    tickets.add_column(ColumnSchema::new("ticket_id", "integer"));
    tickets.add_column(ColumnSchema::new("organization_id", "integer"));
    tickets.add_column(ColumnSchema::new("customer_id", "integer"));
    tickets.add_column(ColumnSchema::new("ticket_number", "varchar"));
    tickets.add_column(ColumnSchema::new("subject", "varchar"));
    tickets.add_column(ColumnSchema::new("description", "text"));
    tickets.add_column(ColumnSchema::new("status", "varchar"));
    tickets.add_column(ColumnSchema::new("priority", "varchar"));
    tickets.add_column(ColumnSchema::new("category", "varchar"));
    tickets.add_column(ColumnSchema::new("assigned_to", "integer"));
    tickets.add_column(ColumnSchema::new("created_at", "timestamp"));
    tickets.add_column(ColumnSchema::new("updated_at", "timestamp"));
    tickets.primary_key.push("ticket_id".to_string());
    schema.add_table(tickets);

    // Products table
    let mut products = TableSchema::new("products");
    products.add_column(ColumnSchema::new("product_id", "integer"));
    products.add_column(ColumnSchema::new("organization_id", "integer"));
    products.add_column(ColumnSchema::new("name", "varchar"));
    products.add_column(ColumnSchema::new("description", "text"));
    products.add_column(ColumnSchema::new("sku", "varchar"));
    products.add_column(ColumnSchema::new("price", "numeric"));
    products.add_column(ColumnSchema::new("cost", "numeric"));
    products.add_column(ColumnSchema::new("category", "varchar"));
    products.add_column(ColumnSchema::new("stock_quantity", "integer"));
    products.add_column(ColumnSchema::new("is_active", "boolean"));
    products.add_column(ColumnSchema::new("created_at", "timestamp"));
    products.add_column(ColumnSchema::new("updated_at", "timestamp"));
    products.primary_key.push("product_id".to_string());
    schema.add_table(products);

    // Users table
    let mut users = TableSchema::new("users");
    users.add_column(ColumnSchema::new("user_id", "integer"));
    users.add_column(ColumnSchema::new("organization_id", "integer"));
    users.add_column(ColumnSchema::new("username", "varchar"));
    users.add_column(ColumnSchema::new("email", "varchar"));
    users.add_column(ColumnSchema::new("password_hash", "varchar"));
    users.add_column(ColumnSchema::new("first_name", "varchar"));
    users.add_column(ColumnSchema::new("last_name", "varchar"));
    users.add_column(ColumnSchema::new("role", "varchar"));
    users.add_column(ColumnSchema::new("is_active", "boolean"));
    users.add_column(ColumnSchema::new("last_login_at", "timestamp"));
    users.add_column(ColumnSchema::new("created_at", "timestamp"));
    users.add_column(ColumnSchema::new("updated_at", "timestamp"));
    users.primary_key.push("user_id".to_string());
    schema.add_table(users);

    // Contacts table
    let mut contacts = TableSchema::new("contacts");
    contacts.add_column(ColumnSchema::new("contact_id", "integer"));
    contacts.add_column(ColumnSchema::new("organization_id", "integer"));
    contacts.add_column(ColumnSchema::new("customer_id", "integer"));
    contacts.add_column(ColumnSchema::new("first_name", "varchar"));
    contacts.add_column(ColumnSchema::new("last_name", "varchar"));
    contacts.add_column(ColumnSchema::new("position", "varchar"));
    contacts.add_column(ColumnSchema::new("email", "varchar"));
    contacts.add_column(ColumnSchema::new("phone", "varchar"));
    contacts.add_column(ColumnSchema::new("is_primary", "boolean"));
    contacts.add_column(ColumnSchema::new("created_at", "timestamp"));
    contacts.add_column(ColumnSchema::new("updated_at", "timestamp"));
    contacts.primary_key.push("contact_id".to_string());
    schema.add_table(contacts);

    // Addresses table
    let mut addresses = TableSchema::new("addresses");
    addresses.add_column(ColumnSchema::new("address_id", "integer"));
    addresses.add_column(ColumnSchema::new("organization_id", "integer"));
    addresses.add_column(ColumnSchema::new("customer_id", "integer"));
    addresses.add_column(ColumnSchema::new("street", "varchar"));
    addresses.add_column(ColumnSchema::new("city", "varchar"));
    addresses.add_column(ColumnSchema::new("state", "varchar"));
    addresses.add_column(ColumnSchema::new("zip", "varchar"));
    addresses.add_column(ColumnSchema::new("country", "varchar"));
    addresses.add_column(ColumnSchema::new("is_billing", "boolean"));
    addresses.add_column(ColumnSchema::new("is_shipping", "boolean"));
    addresses.add_column(ColumnSchema::new("created_at", "timestamp"));
    addresses.add_column(ColumnSchema::new("updated_at", "timestamp"));
    addresses.primary_key.push("address_id".to_string());
    schema.add_table(addresses);

    // Opportunities table
    let mut opportunities = TableSchema::new("opportunities");
    opportunities.add_column(ColumnSchema::new("opportunity_id", "integer"));
    opportunities.add_column(ColumnSchema::new("organization_id", "integer"));
    opportunities.add_column(ColumnSchema::new("customer_id", "integer"));
    opportunities.add_column(ColumnSchema::new("name", "varchar"));
    opportunities.add_column(ColumnSchema::new("value", "numeric"));
    opportunities.add_column(ColumnSchema::new("probability", "integer"));
    opportunities.add_column(ColumnSchema::new("stage", "varchar"));
    opportunities.add_column(ColumnSchema::new("expected_close_date", "date"));
    opportunities.add_column(ColumnSchema::new("assigned_to", "integer"));
    opportunities.add_column(ColumnSchema::new("source", "varchar"));
    opportunities.add_column(ColumnSchema::new("notes", "text"));
    opportunities.add_column(ColumnSchema::new("won_at", "timestamp"));
    opportunities.add_column(ColumnSchema::new("lost_at", "timestamp"));
    opportunities.add_column(ColumnSchema::new("lost_reason", "varchar"));
    opportunities.add_column(ColumnSchema::new("created_at", "timestamp"));
    opportunities.add_column(ColumnSchema::new("updated_at", "timestamp"));
    opportunities.primary_key.push("opportunity_id".to_string());
    schema.add_table(opportunities);

    // Invoices table
    let mut invoices = TableSchema::new("invoices");
    invoices.add_column(ColumnSchema::new("invoice_id", "integer"));
    invoices.add_column(ColumnSchema::new("organization_id", "integer"));
    invoices.add_column(ColumnSchema::new("order_id", "integer"));
    invoices.add_column(ColumnSchema::new("invoice_number", "varchar"));
    invoices.add_column(ColumnSchema::new("invoice_date", "date"));
    invoices.add_column(ColumnSchema::new("due_date", "date"));
    invoices.add_column(ColumnSchema::new("status", "varchar"));
    invoices.add_column(ColumnSchema::new("total_amount", "numeric"));
    invoices.add_column(ColumnSchema::new("paid_amount", "numeric"));
    invoices.add_column(ColumnSchema::new("created_at", "timestamp"));
    invoices.add_column(ColumnSchema::new("updated_at", "timestamp"));
    invoices.primary_key.push("invoice_id".to_string());
    schema.add_table(invoices);

    // Payments table
    let mut payments = TableSchema::new("payments");
    payments.add_column(ColumnSchema::new("payment_id", "integer"));
    payments.add_column(ColumnSchema::new("organization_id", "integer"));
    payments.add_column(ColumnSchema::new("invoice_id", "integer"));
    payments.add_column(ColumnSchema::new("payment_date", "date"));
    payments.add_column(ColumnSchema::new("amount", "numeric"));
    payments.add_column(ColumnSchema::new("payment_method", "varchar"));
    payments.add_column(ColumnSchema::new("reference_number", "varchar"));
    payments.add_column(ColumnSchema::new("notes", "text"));
    payments.add_column(ColumnSchema::new("created_at", "timestamp"));
    payments.add_column(ColumnSchema::new("updated_at", "timestamp"));
    payments.primary_key.push("payment_id".to_string());
    schema.add_table(payments);

    schema
}
