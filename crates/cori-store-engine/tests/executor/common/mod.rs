// SPDX-License-Identifier: Apache-2.0
//! Shared test infrastructure for the executor's end-to-end tests.
//!
//! This module provides:
//! - a test database on the Postgres server of `CORI_STORE_DATABASE_URL` (an admin URL)
//! - executors as a store runs them: the role's grants and row level security applied to that database, every call in
//!   a transaction under the role's data role with the caller's tenant set
//! - Test fixtures (roles, rules, contexts)
//! - Helper functions for test assertions

use cori_store_core::config::role_definition::{
    ApprovalConfig, ApprovalRequirement, ColumnCondition, CreatableColumnConstraints,
    CreatableColumns, DeletablePermission, OnlyWhen, ReadableConfig, RoleDefinition,
    TablePermissions, UpdatableColumnConstraints, UpdatableColumns,
};
use cori_store_core::config::rules_definition::{RulesDefinition, TableRules, TenantConfig};
use cori_store_engine::ParkedChange;
use cori_store_engine::executor::{ExecutionContext, ExecutionResult, ToolExecutor};
use cori_store_engine::grants::{self, Policy};
use cori_store_core::protocol::{CallToolOptions, ToolContent, ToolDefinition};
use cori_store_core::config::SchemaDefinition;
use serde_json::{Value, json};
use sqlx::{AssertSqlSafe, PgPool, Postgres, Transaction};
use std::collections::HashMap;
use std::sync::Arc;

/// The database the tests recreate on the server of `CORI_STORE_DATABASE_URL`.
pub const DATABASE_NAME: &str = "cori_executor_test";

// =============================================================================
// DATABASE INITIALIZATION
// =============================================================================

const SCHEMA_SQL: &str = include_str!("../fixtures/schema.sql");
const SEED_SQL: &str = include_str!("../fixtures/seed.sql");

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
    /// The schema introspected from the test database, as a store is opened with it.
    pub schema: SchemaDefinition,
    /// The login of `pool`, which the policies let switch to their roles (a store's runtime login).
    login: String,
    /// The hash of the policy the database enforces now: an executor applies its own before its first call.
    enforced: Arc<tokio::sync::Mutex<Option<String>>>,
}

/// The prefix of the Postgres roles the suite's policies create (a store's is per app and store).
const ROLE_PREFIX: &str = "cori_extest";

/// The test database, dropped and created again on the server of `admin_url`: its pool and URL.
async fn test_database(admin_url: &str) -> Result<(PgPool, String), String> {
    let admin = PgPool::connect(admin_url).await.map_err(|e| format!("CORI_STORE_DATABASE_URL: {}", e))?;
    sqlx::query(AssertSqlSafe(format!("DROP DATABASE IF EXISTS {} WITH (FORCE)", DATABASE_NAME))).execute(&admin).await.map_err(|e| e.to_string())?;
    sqlx::query(AssertSqlSafe(format!("CREATE DATABASE {}", DATABASE_NAME))).execute(&admin).await.map_err(|e| e.to_string())?;
    admin.close().await;
    let base = admin_url.split('?').next().unwrap_or(admin_url);
    let url = format!("{}/{}", base.rsplit_once('/').map(|(b, _)| b).unwrap_or(base), DATABASE_NAME);
    Ok((PgPool::connect(&url).await.map_err(|e| e.to_string())?, url))
}

impl TestContext {
    pub async fn setup(admin_url: &str) -> Result<Self, String> {
        let (pool, url) = test_database(admin_url).await?;
        initialize_database(&pool)
            .await
            .map_err(|e| format!("Failed to initialize database: {}", e))?;
        let json = cori_store_engine::introspect::introspect_schema_json(&url).await.map_err(|e| format!("introspection: {e}"))?;
        let definition: SchemaDefinition = serde_json::from_value(json).map_err(|e| format!("introspected schema: {e}"))?;
        let login: String = sqlx::query_scalar("SELECT current_user::text").fetch_one(&pool).await.map_err(|e| e.to_string())?;
        Ok(Self { pool, schema: definition, login, enforced: Arc::default() })
    }

    /// An executor for a custom role and rules, with the policy a store compiles for them
    pub fn executor_with(&self, role: RoleDefinition, rules: RulesDefinition) -> Enforced {
        let policy = grants::compile(&self.schema, &rules, std::slice::from_ref(&role), ROLE_PREFIX).unwrap_or_else(|e| panic!("the policy of role `{}`: {e}", role.name));
        let pg = policy.roles.iter().find(|p| p.role == role.name).cloned().expect("the role's Postgres roles");
        let executor = ToolExecutor::new(role, pg.applier.clone()).with_rules(rules).with_schema(self.schema.clone());
        Enforced { executor, policy, data: pg.data, applier: pg.applier, pool: self.pool.clone(), login: self.login.clone(), enforced: self.enforced.clone() }
    }

    /// Create an executor with the default support agent role
    pub fn executor(&self) -> Enforced {
        self.executor_with(create_support_agent_role(), create_default_rules())
    }
}

/// An executor as a store runs it: its policy (grants, row level security, only_when triggers) enforced by the
/// database, and every call in a session under its role's data role with the caller's tenant set (`Store::call`).
pub struct Enforced {
    executor: ToolExecutor,
    policy: Policy,
    data: String,
    applier: String,
    pool: PgPool,
    login: String,
    enforced: Arc<tokio::sync::Mutex<Option<String>>>,
}

impl Enforced {
    /// Applies this executor's policy unless the database enforces it already: the roles (server wide), then the
    /// statements in one transaction. A policy replaces the one before it (it drops what an earlier one created).
    async fn enforce(&self) {
        let mut enforced = self.enforced.lock().await;
        if enforced.as_deref() == Some(self.policy.hash.as_str()) { return; }
        for s in grants::role_statements(&self.policy, &self.login) {
            sqlx::raw_sql(AssertSqlSafe(s.as_str())).execute(&self.pool).await.unwrap_or_else(|e| panic!("policy roles: {e}\n  in: {s}"));
        }
        let mut tx = self.pool.begin().await.unwrap();
        for s in &self.policy.statements {
            sqlx::raw_sql(AssertSqlSafe(s.as_str())).execute(&mut *tx).await.unwrap_or_else(|e| panic!("policy: {e}\n  in: {}", s.lines().next().unwrap_or("")));
        }
        tx.commit().await.unwrap();
        *enforced = Some(self.policy.hash.clone());
    }

    /// A session as a store opens one: a transaction under `pg_role`, the caller's tenant set.
    async fn session(&self, pg_role: &str, tenant: &str) -> Transaction<'static, Postgres> {
        self.enforce().await;
        let mut tx = self.pool.begin().await.unwrap();
        sqlx::query("SELECT set_config('cori.tenant', $1, true), set_config('statement_timeout', '10s', true), set_config('standard_conforming_strings', 'on', true), set_config('role', $2, true)")
            .bind(tenant).bind(pg_role).execute(&mut *tx).await.unwrap();
        tx
    }

    /// A session under the role's data role for `tenant`, to run a statement straight at the database.
    pub async fn data_session(&self, tenant: &str) -> Transaction<'static, Postgres> {
        self.session(&self.data, tenant).await
    }

    /// A call as `Store::call` makes it: under the role's data role, committed when it succeeds (a dry run never is).
    pub async fn execute(&self, tool: &ToolDefinition, arguments: Value, options: &CallToolOptions, context: &ExecutionContext) -> ExecutionResult {
        let mut tx = self.session(&self.data, &context.tenant_id).await;
        let result = self.executor.execute_on(&mut tx, tool, arguments, options, context).await;
        if result.success && !options.dry_run { tx.commit().await.unwrap(); } else { tx.rollback().await.ok(); }
        result
    }

    /// An approved change applied as `Store::apply_parked` applies it: under the role's applier role.
    pub async fn execute_approved(&self, tool: &ToolDefinition, change: &ParkedChange, context: &ExecutionContext) -> ExecutionResult {
        let mut tx = self.session(&self.applier, &context.tenant_id).await;
        let result = self.executor.execute_approved_on(&mut tx, tool, change, context).await;
        if result.success { tx.commit().await.unwrap(); } else { tx.rollback().await.ok(); }
        result
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
pub fn extract_json(result: &cori_store_engine::executor::ExecutionResult) -> Option<Value> {
    result.content.first().and_then(|c| match c {
        ToolContent::Json { json } => Some(json.clone()),
        ToolContent::Text { text } => serde_json::from_str(text).ok(),
    })
}

/// Assert that a result is successful
pub fn assert_success(result: &cori_store_engine::executor::ExecutionResult, msg: &str) {
    assert!(result.success, "{}: {:?}", msg, result);
}

/// Assert that a result is a failure
pub fn assert_failure(result: &cori_store_engine::executor::ExecutionResult, msg: &str) {
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
    } else if let Some(base) = s.strip_suffix("es") {
        // Handle cases like "addresses" -> "address"
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
            scope: Default::default(),
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
            scope: Default::default(),
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
            scope: Default::default(),
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
            scope: Default::default(),
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
            scope: Default::default(),
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
            scope: Default::default(),
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
            scope: Default::default(),
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
            scope: Default::default(),
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
            scope: Default::default(),
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
            scope: Default::default(),
            readable: ReadableConfig::All(cori_store_core::config::role_definition::AllColumns),
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
            scope: Default::default(),
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
            scope: Default::default(),
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
            scope: Default::default(),
            readable: ReadableConfig::All(cori_store_core::config::role_definition::AllColumns),
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
            scope: Default::default(),
            readable: ReadableConfig::All(cori_store_core::config::role_definition::AllColumns),
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
