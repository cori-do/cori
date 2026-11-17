// SPDX-License-Identifier: Apache-2.0
//! Tool execution engine.
//!
//! This module handles the actual execution of MCP tools, including:
//! - Mapping tool calls to SQL queries
//! - Applying RLS predicates
//! - Dry-run execution
//! - Constraint validation
//! - Result formatting
//! - Audit logging
//!
//! `access`: what the role and the rules allow on a table; `checks`: the checks around a call (the argument schema,
//! foreign keys, the current row); `read` and `write`: the statement of each tool kind; `sql`: values and rows.

mod access;
mod checks;
mod read;
mod sql;
mod write;

use crate::approval::ParkedChange;
use sql::is_key;
use cori_store_core::protocol::{CallToolOptions, DryRunResult, ToolContent, ToolDefinition};
use cori_store_core::config::SchemaDefinition;
use cori_store_core::RoleDefinition;
use cori_store_core::config::rules_definition::RulesDefinition;
use crate::validation::{OperationType, ToolValidator, ValidationRequest};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::PgConnection;

/// Result of a tool execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionResult {
    /// Whether the execution was successful.
    pub success: bool,
    /// The result content.
    pub content: Vec<ToolContent>,
    /// Whether this was a dry-run.
    #[serde(rename = "isDryRun")]
    pub is_dry_run: bool,
    /// Error message if failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// SQL query that was executed (for audit logging).
    #[serde(skip)]
    pub executed_sql: Option<String>,
    /// State before mutation (for audit logging).
    #[serde(skip)]
    pub before_state: Option<Value>,
    /// State after mutation (for audit logging).
    #[serde(skip)]
    pub after_state: Option<Value>,
    /// A change that needs an approval, not executed: the caller keeps it until it is decided.
    #[serde(skip)]
    pub parked: Option<ParkedChange>,
}

impl ExecutionResult {

    /// Create a successful result with JSON content and SQL.
    pub fn success_with_sql(value: Value, sql: impl Into<String>) -> Self {
        Self {
            success: true,
            content: vec![ToolContent::Json { json: value }],
            is_dry_run: false,
            error: None,
            executed_sql: Some(sql.into()),
            before_state: None,
            after_state: None,
            parked: None,
        }
    }

    /// Create a successful mutation result with before/after state for audit.
    pub fn mutation_success(
        value: Value,
        sql: impl Into<String>,
        before_state: Option<Value>,
        after_state: Option<Value>,
    ) -> Self {
        Self {
            success: true,
            content: vec![ToolContent::Json { json: value }],
            is_dry_run: false,
            error: None,
            executed_sql: Some(sql.into()),
            before_state,
            after_state,
            parked: None,
        }
    }

    /// Create a successful dry-run result.
    pub fn dry_run(result: DryRunResult) -> Self {
        Self {
            success: true,
            content: vec![ToolContent::Json {
                json: serde_json::to_value(result).unwrap_or_default(),
            }],
            is_dry_run: true,
            error: None,
            executed_sql: None,
            before_state: None,
            after_state: None,
            parked: None,
        }
    }

    /// Create an error result.
    pub fn error(message: impl Into<String>) -> Self {
        let msg = message.into();
        Self {
            success: false,
            content: vec![ToolContent::Text { text: msg.clone() }],
            is_dry_run: false,
            error: Some(msg),
            executed_sql: None,
            before_state: None,
            after_state: None,
            parked: None,
        }
    }

    /// A change parked for approval: the caller is told it is pending.
    pub fn parked(change: ParkedChange) -> Self {
        Self {
            success: true,
            content: vec![ToolContent::Json { json: change.pending_response() }],
            is_dry_run: false,
            error: None,
            executed_sql: None,
            before_state: None,
            after_state: None,
            parked: Some(change),
        }
    }
}

/// Context for tool execution.
#[derive(Debug, Clone)]
pub struct ExecutionContext {
    /// Tenant ID from token.
    pub tenant_id: String,
    /// Role name from token.
    pub role: String,
    /// Connection ID (for auditing).
    pub connection_id: Option<String>,
}

/// The tool executor handles running tools against the database. It runs on the connections a store gives it: a
/// transaction under the role's data role with the caller's tenant set, where the database enforces the role's grants
/// and row level security compiled from the same role and rules (`grants`). Cori's own reads and writes on the role's
/// behalf (the current row for `only_when`, approval snapshots, foreign key checks, soft deletes) switch to its applier
/// role.
pub struct ToolExecutor {
    /// Role definition for permission checks.
    role: RoleDefinition,
    /// Rules definition for tenant configuration.
    rules: Option<RulesDefinition>,
    /// Database schema for primary key lookup.
    schema: Option<SchemaDefinition>,
    /// The Postgres role Cori's own statements for this role run under.
    applier: String,
}

impl ToolExecutor {
    /// An executor for `role`, whose Postgres applier role is `applier_role`.
    pub fn new(role: RoleDefinition, applier_role: impl Into<String>) -> Self {
        Self { role, rules: None, schema: None, applier: applier_role.into() }
    }

    /// Set the rules definition for tenant configuration.
    pub fn with_rules(mut self, rules: RulesDefinition) -> Self {
        self.rules = Some(rules);
        self
    }

    /// Set the database schema for primary key lookup.
    pub fn with_schema(mut self, schema: SchemaDefinition) -> Self {
        self.schema = Some(schema);
        self
    }

    /// Execute a tool call on a connection in the role's session (see the type). A change that needs an approval is not
    /// executed: the result carries it (`parked`).
    pub async fn execute_on(
        &self,
        db: &mut PgConnection,
        tool: &ToolDefinition,
        arguments: Value,
        options: &CallToolOptions,
        context: &ExecutionContext,
    ) -> ExecutionResult {
        self.execute_checked(db, tool, arguments, options, context, true).await
    }

    /// A call under every check (role, rules, constraints, the tool's argument schema); `park`: a change that needs an
    /// approval is returned instead of executed.
    async fn execute_checked(
        &self,
        db: &mut PgConnection,
        tool: &ToolDefinition,
        arguments: Value,
        options: &CallToolOptions,
        context: &ExecutionContext,
        park: bool,
    ) -> ExecutionResult {
        // 0. Parse tool name to determine operation and table
        let Some(operation) = self.parse_tool_operation(&tool.name) else {
            return ExecutionResult::error(format!("Tool '{}' is not supported: a role has get, list, create, update and delete tools only", tool.name));
        };

        // 1. Create validator and validate the request against role and rules
        let (op_type, table) = match &operation {
            ToolOperation::Get { table } => (OperationType::Get, table.as_str()),
            ToolOperation::List { table } => (OperationType::List, table.as_str()),
            ToolOperation::Create { table } => (OperationType::Create, table.as_str()),
            ToolOperation::Update { table } => (OperationType::Update, table.as_str()),
            ToolOperation::Delete { table } => (OperationType::Delete, table.as_str()),
        };

        // Get primary key columns from schema (supports composite keys)
        let pk_columns = self.get_primary_key_columns(table);
        let pk_column_refs: Vec<&str> = pk_columns.iter().map(|s| s.as_str()).collect();

        // For UPDATE operations with old.* constraints, we need to fetch the current row first
        let current_row: Option<Value> = if op_type == OperationType::Update {
            // Check if role has old.* constraints that need current row
            if self.role_has_old_constraints(table) {
                // Fetch current row from database
                let fetched = self.fetch_current_row(db, table, &arguments, context).await;
                match fetched {
                    Ok(Some(row)) => Some(row),
                    Ok(None) => {
                        return ExecutionResult::error("Record not found or access denied");
                    }
                    Err(e) => {
                        return ExecutionResult::error(format!(
                            "Failed to fetch current row for validation: {}",
                            e
                        ));
                    }
                }
            } else {
                None
            }
        } else {
            None
        };

        // Build the validation request
        let validation_request = ValidationRequest {
            operation: op_type,
            table,
            arguments: &arguments,
            tenant_id: &context.tenant_id,
            role_name: &context.role,
            current_row: current_row.as_ref(),
            primary_key_columns: Some(pk_column_refs.clone()),
        };

        // Create validator with role and optionally rules
        let mut validator = ToolValidator::new(&self.role);
        if let Some(rules) = &self.rules {
            validator = validator.with_rules(rules);
        }

        // Validate the request (checks permissions and constraints)
        if let Err(e) = validator.validate(&validation_request) {
            return ExecutionResult::error(e.to_string());
        }

        // 2. The tool's argument schema, before a change is parked: a parked change is a complete call
        if let Err(e) = self.validate_arguments(tool, &arguments) {
            return ExecutionResult::error(e);
        }

        // 3. A change to a `requires_approval` column is parked (a dry run shows what it would do instead). An update or
        // delete of an existing row keeps the row as the role reads it: the change applies only while it reads the same.
        if park
            && !options.dry_run
            && let Some(approval_fields) = validator.requires_approval(&validation_request)
        {
            let mut change = ParkedChange::new(&tool.name, arguments.clone(), approval_fields, &context.tenant_id, &context.role);
            if let ToolOperation::Update { table } | ToolOperation::Delete { table } = &operation {
                let pk_json = self.get_primary_key_columns(table).first().and_then(|pk| arguments.get(pk)).cloned().unwrap_or(Value::Null);
                if is_key(&pk_json) {
                    let snapshot = self.fetch_row_snapshot(db, table, &pk_json, &context.tenant_id).await;
                    if let Some(original_values) = snapshot {
                        change.target_table = Some(table.clone());
                        change.target_pk = Some(pk_json);
                        change.original_values = Some(original_values);
                    }
                }
            }
            return ExecutionResult::parked(change);
        }

        // 4. Execute based on operation type
        let result = match operation {
            ToolOperation::Get { table } => {
                self.execute_get(db, &table, &arguments, options, context).await
            }
            ToolOperation::List { table } => {
                self.execute_list(db, &table, &arguments, options, context)
                    .await
            }
            ToolOperation::Create { table } => {
                self.execute_create(db, &table, &arguments, options, context)
                    .await
            }
            ToolOperation::Update { table } => {
                self.execute_update(db, &table, &arguments, options, context)
                    .await
            }
            ToolOperation::Delete { table } => {
                self.execute_delete(db, &table, &arguments, options, context)
                    .await
            }
        };

        result
    }

    /// Applies an approved change: first checks that the row it was parked on (update, delete) still reads the same,
    /// then runs `tool` (the change's tool as the role has it now) under the same checks as a call, without asking for
    /// the approval again. Run it in the session of the role's applier role, which holds the approval-gated grants.
    pub async fn execute_approved_on(
        &self,
        db: &mut PgConnection,
        tool: &ToolDefinition,
        change: &ParkedChange,
        context: &ExecutionContext,
    ) -> ExecutionResult {
        if let (Some(table), Some(pk), Some(original_values)) = (&change.target_table, &change.target_pk, &change.original_values)
            && is_key(pk)
        {
            let current = self.fetch_row_snapshot(db, table, pk, &context.tenant_id).await;
            match current {
                Some(current) if &current != original_values => {
                    return ExecutionResult::error("The row changed since the change was requested: ask for it again on the current data.");
                }
                Some(_) => {}
                None => return ExecutionResult::error("The row no longer exists (or the role no longer reaches it)."),
            }
        }
        self.execute_checked(db, tool, change.arguments.clone(), &CallToolOptions { dry_run: false }, context, false).await
    }

    /// Parse tool name to determine the operation.
    fn parse_tool_operation(&self, name: &str) -> Option<ToolOperation> {
        // A generated tool maps back to the role's table whose generated name it is (`getPerson` → `people`,
        // `getStaff` → `staff`): re-pluralizing the name would guess another table for irregular or singular names.
        if let Some((op, table)) = cori_store_core::tool_generator::tool_table(&self.role, name) {
            let table = table.to_string();
            return Some(match op {
                "get" => ToolOperation::Get { table },
                "list" => ToolOperation::List { table },
                "create" => ToolOperation::Create { table },
                "update" => ToolOperation::Update { table },
                _ => ToolOperation::Delete { table },
            });
        }
        // Not a tool of the role's tables: the naming heuristic (the validator then refuses a table outside the role).
        // `get{Entity}`, `create{Entity}`, `update{Entity}` and `delete{Entity}` name a plural table (`getCustomer` →
        // `customers`), `list{Entities}` names it as is.
        let (op, entity) = cori_store_core::tool_generator::OPERATIONS.into_iter().find_map(|op| name.strip_prefix(op).map(|e| (op, e)))?;
        let table = if op == "list" { snake_case(entity) } else { pluralize(&snake_case(entity)) };
        Some(match op {
            "get" => ToolOperation::Get { table },
            "list" => ToolOperation::List { table },
            "create" => ToolOperation::Create { table },
            "update" => ToolOperation::Update { table },
            _ => ToolOperation::Delete { table },
        })
    }
}

/// Parsed tool operation.
#[derive(Debug)]
enum ToolOperation {
    Get { table: String },
    List { table: String },
    Create { table: String },
    Update { table: String },
    Delete { table: String },
}

/// Convert PascalCase to snake_case.
fn snake_case(s: &str) -> String {
    let mut result = String::new();
    for (i, c) in s.chars().enumerate() {
        if c.is_uppercase() {
            if i > 0 {
                result.push('_');
            }
            result.push(c.to_lowercase().next().unwrap());
        } else {
            result.push(c);
        }
    }
    result
}

/// Simple pluralization.
fn pluralize(s: &str) -> String {
    if s.ends_with('s') || s.ends_with('x') || s.ends_with("ch") || s.ends_with("sh") {
        format!("{}es", s)
    } else if s.ends_with('y') && !s.ends_with("ey") && !s.ends_with("ay") && !s.ends_with("oy") {
        format!("{}ies", &s[..s.len() - 1])
    } else {
        format!("{}s", s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_snake_case() {
        assert_eq!(snake_case("User"), "user");
        assert_eq!(snake_case("OrderItem"), "order_item");
        assert_eq!(snake_case("APIKey"), "a_p_i_key");
    }

    #[test]
    fn test_parse_tool_operation() {
        let tables: std::collections::HashMap<String, cori_store_core::TablePermissions> = ["users", "people", "staff", "inventory", "categories", "boxes", "status"]
            .iter().map(|t| (t.to_string(), cori_store_core::TablePermissions::default())).collect();
        let role = RoleDefinition {
            name: "test".to_string(),
            description: None,
            approvals: None,
            tables,
        };
        let executor = ToolExecutor::new(role, "cori_test__applier");

        // every generated name maps back to its own table, whatever the plural
        for table in ["users", "people", "staff", "inventory", "categories", "boxes", "status"] {
            let name = |op: &str| cori_store_core::tool_generator::tool_name(op, table);
            assert!(matches!(executor.parse_tool_operation(&name("get")), Some(ToolOperation::Get { table: t }) if t == table), "get {table}");
            assert!(matches!(executor.parse_tool_operation(&name("list")), Some(ToolOperation::List { table: t }) if t == table), "list {table}");
            assert!(matches!(executor.parse_tool_operation(&name("create")), Some(ToolOperation::Create { table: t }) if t == table), "create {table}");
            assert!(matches!(executor.parse_tool_operation(&name("update")), Some(ToolOperation::Update { table: t }) if t == table), "update {table}");
            assert!(matches!(executor.parse_tool_operation(&name("delete")), Some(ToolOperation::Delete { table: t }) if t == table), "delete {table}");
        }
        assert_eq!(cori_store_core::tool_generator::tool_name("get", "people"), "getPerson");
        assert_eq!(cori_store_core::tool_generator::tool_name("list", "people"), "listPersons");
        // a name outside the role's tables keeps the naming heuristic
        assert!(matches!(executor.parse_tool_operation("getTicket"), Some(ToolOperation::Get { table }) if table == "tickets"));
        assert!(executor.parse_tool_operation("refundOrder").is_none());
    }
}
