//! Tool execution engine.
//!
//! This module handles the actual execution of MCP tools, including:
//! - Mapping tool calls to SQL queries
//! - Applying RLS predicates
//! - Dry-run execution
//! - Constraint validation
//! - Result formatting

use crate::approval::{ApprovalManager, ApprovalPendingResponse};
use crate::protocol::{CallToolOptions, DryRunResult, ToolContent, ToolDefinition};
use crate::schema::DatabaseSchema;
use cori_core::config::rules_definition::{RulesDefinition, TenantConfig};
use cori_core::RoleDefinition;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Row};
use std::sync::Arc;

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
}

impl ExecutionResult {
    /// Create a successful result with JSON content.
    pub fn success_json(value: Value) -> Self {
        Self {
            success: true,
            content: vec![ToolContent::Json { json: value }],
            is_dry_run: false,
            error: None,
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
        }
    }

    /// Create a pending approval result.
    pub fn pending_approval(response: ApprovalPendingResponse) -> Self {
        Self {
            success: true,
            content: vec![ToolContent::Json {
                json: serde_json::to_value(response).unwrap_or_default(),
            }],
            is_dry_run: false,
            error: None,
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

/// The tool executor handles running tools against the database.
pub struct ToolExecutor {
    /// Role definition for permission checks.
    role: RoleDefinition,
    /// Approval manager for human-in-the-loop.
    approval_manager: Arc<ApprovalManager>,
    /// Database connection pool.
    pool: Option<PgPool>,
    /// Rules definition for tenant configuration.
    rules: Option<RulesDefinition>,
    /// Database schema for primary key lookup.
    schema: Option<DatabaseSchema>,
}

impl ToolExecutor {
    /// Create a new tool executor.
    pub fn new(role: RoleDefinition, approval_manager: Arc<ApprovalManager>) -> Self {
        Self {
            role,
            approval_manager,
            pool: None,
            rules: None,
            schema: None,
        }
    }

    /// Set the database connection pool.
    pub fn with_pool(mut self, pool: PgPool) -> Self {
        self.pool = Some(pool);
        self
    }

    /// Set the database URL and create a pool.
    pub async fn with_database_url(mut self, url: impl Into<String>) -> Result<Self, sqlx::Error> {
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(&url.into())
            .await?;
        self.pool = Some(pool);
        Ok(self)
    }

    /// Set the rules definition for tenant configuration.
    pub fn with_rules(mut self, rules: RulesDefinition) -> Self {
        self.rules = Some(rules);
        self
    }

    /// Set the database schema for primary key lookup.
    pub fn with_schema(mut self, schema: DatabaseSchema) -> Self {
        self.schema = Some(schema);
        self
    }

    fn tenant_column_for_table(&self, table: &str) -> Option<String> {
        let rules = self.rules.as_ref()?;
        let table_rules = rules.get_table_rules(table)?;

        // Check if global table
        if table_rules.global.unwrap_or(false) {
            return None;
        }

        // Get tenant config
        match &table_rules.tenant {
            Some(TenantConfig::Direct(column)) => Some(column.clone()),
            Some(TenantConfig::Inherited(_)) => {
                // For inherited tenancy, we need to JOIN through FK
                // For now, return None (handled separately in query building)
                None
            }
            None => None,
        }
    }

    fn validate_tenant_for_table(&self, table: &str, tenant_id: &str) -> Result<(), String> {
        let tenant_column = self.tenant_column_for_table(table);
        if tenant_column.is_none() {
            // Global / unscoped table or inherited tenancy: tenant not required at this level.
            return Ok(());
        }

        if tenant_id == "unknown" || tenant_id.trim().is_empty() {
            return Err(format!(
                "Missing tenant id for tenant-scoped table '{}'. Provide a tenant-scoped Biscuit token (with a tenant(...) attenuation block) via --token or CORI_TOKEN.",
                table
            ));
        }

        // No type validation here - database will validate
        Ok(())
    }

    /// Get the primary key column for a table from schema.
    /// Falls back to deriving from table name if schema not available.
    fn get_primary_key_column(&self, table: &str) -> String {
        if let Some(schema) = &self.schema {
            if let Some(table_schema) = schema.get_table(table) {
                if let Some(pk) = table_schema.primary_key.first() {
                    return pk.clone();
                }
            }
        }
        // Fallback: derive from table name (e.g., customers -> customer_id)
        format!("{}_id", singularize(table))
    }

    /// Execute a tool call.
    pub async fn execute(
        &self,
        tool: &ToolDefinition,
        arguments: Value,
        options: &CallToolOptions,
        context: &ExecutionContext,
    ) -> ExecutionResult {
        // 1. Validate arguments against constraints
        if let Err(e) = self.validate_arguments(tool, &arguments) {
            return ExecutionResult::error(e);
        }

        // 2. Check if approval is required
        if self.requires_approval(tool, &arguments) && !options.dry_run {
            // Create approval request
            let approval_fields = tool
                .annotations
                .as_ref()
                .and_then(|a| a.approval_fields.clone())
                .unwrap_or_default();

            let request = self.approval_manager.create_request(
                &tool.name,
                arguments.clone(),
                approval_fields,
                &context.tenant_id,
                &context.role,
            );

            return ExecutionResult::pending_approval(ApprovalPendingResponse::from(&request));
        }

        // 3. Parse tool name to determine operation
        let operation = self.parse_tool_operation(&tool.name);

        // 4. Execute based on operation type
        match operation {
            ToolOperation::Get { table } => {
                self.execute_get(&table, &arguments, options, context).await
            }
            ToolOperation::List { table } => {
                self.execute_list(&table, &arguments, options, context).await
            }
            ToolOperation::Create { table } => {
                self.execute_create(&table, &arguments, options, context)
                    .await
            }
            ToolOperation::Update { table } => {
                self.execute_update(&table, &arguments, options, context)
                    .await
            }
            ToolOperation::Delete { table } => {
                self.execute_delete(&table, &arguments, options, context)
                    .await
            }
            ToolOperation::Custom { name } => {
                self.execute_custom(&name, &arguments, options, context)
                    .await
            }
        }
    }

    /// Validate arguments against tool constraints.
    fn validate_arguments(&self, tool: &ToolDefinition, arguments: &Value) -> Result<(), String> {
        let schema = &tool.input_schema;

        // Check required fields
        if let Some(required) = schema["required"].as_array() {
            for req in required {
                if let Some(field) = req.as_str() {
                    if arguments.get(field).is_none() {
                        return Err(format!("Missing required field: {}", field));
                    }
                }
            }
        }

        // Check enum constraints
        if let Some(props) = schema["properties"].as_object() {
            for (field, prop_schema) in props {
                if let Some(value) = arguments.get(field) {
                    // Check enum
                    if let Some(allowed) = prop_schema["enum"].as_array() {
                        let allowed_values: Vec<&Value> = allowed.iter().collect();
                        if !allowed_values.contains(&value) {
                            return Err(format!(
                                "Invalid value for '{}': {:?}. Allowed: {:?}",
                                field, value, allowed
                            ));
                        }
                    }

                    // Check type
                    if let Some(expected_type) = prop_schema["type"].as_str() {
                        if !self.check_type(value, expected_type) {
                            return Err(format!(
                                "Invalid type for '{}': expected {}, got {:?}",
                                field, expected_type, value
                            ));
                        }
                    }

                    // Check min/max
                    if let Some(min) = prop_schema["minimum"].as_f64() {
                        if let Some(v) = value.as_f64() {
                            if v < min {
                                return Err(format!(
                                    "Value for '{}' must be at least {}",
                                    field, min
                                ));
                            }
                        }
                    }
                    if let Some(max) = prop_schema["maximum"].as_f64() {
                        if let Some(v) = value.as_f64() {
                            if v > max {
                                return Err(format!(
                                    "Value for '{}' must be at most {}",
                                    field, max
                                ));
                            }
                        }
                    }

                    // Check pattern
                    if let Some(pattern) = prop_schema["pattern"].as_str() {
                        if let Some(s) = value.as_str() {
                            if let Ok(re) = regex::Regex::new(pattern) {
                                if !re.is_match(s) {
                                    return Err(format!(
                                        "Value for '{}' does not match pattern: {}",
                                        field, pattern
                                    ));
                                }
                            }
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Check if a value matches an expected type.
    fn check_type(&self, value: &Value, expected: &str) -> bool {
        match expected {
            "string" => value.is_string(),
            "integer" => value.is_i64() || value.is_u64(),
            "number" => value.is_number(),
            "boolean" => value.is_boolean(),
            "object" => value.is_object(),
            "array" => value.is_array(),
            "null" => value.is_null(),
            _ => true,
        }
    }

    /// Check if approval is required for this tool call.
    fn requires_approval(&self, tool: &ToolDefinition, arguments: &Value) -> bool {
        if let Some(annotations) = &tool.annotations {
            if annotations.requires_approval == Some(true) {
                // Check if any approval fields are being modified
                if let Some(fields) = &annotations.approval_fields {
                    return fields
                        .iter()
                        .any(|f| arguments.get(f).is_some());
                }
                return true;
            }
        }
        false
    }

    /// Parse tool name to determine the operation.
    fn parse_tool_operation(&self, name: &str) -> ToolOperation {
        // Standard patterns: get{Entity}, list{Entities}, create{Entity}, update{Entity}, delete{Entity}
        // Database tables are typically plural (e.g., customers, tickets, orders)
        if let Some(entity) = name.strip_prefix("get") {
            // getCustomer -> customers table
            ToolOperation::Get {
                table: pluralize(&snake_case(entity)),
            }
        } else if let Some(entity) = name.strip_prefix("list") {
            // listCustomers -> customers table (already plural)
            ToolOperation::List {
                table: snake_case(entity),
            }
        } else if let Some(entity) = name.strip_prefix("create") {
            // createCustomer -> customers table
            ToolOperation::Create {
                table: pluralize(&snake_case(entity)),
            }
        } else if let Some(entity) = name.strip_prefix("update") {
            // updateCustomer -> customers table
            ToolOperation::Update {
                table: pluralize(&snake_case(entity)),
            }
        } else if let Some(entity) = name.strip_prefix("delete") {
            // deleteCustomer -> customers table
            ToolOperation::Delete {
                table: pluralize(&snake_case(entity)),
            }
        } else {
            ToolOperation::Custom {
                name: name.to_string(),
            }
        }
    }

    /// Execute a GET operation.
    async fn execute_get(
        &self,
        table: &str,
        arguments: &Value,
        options: &CallToolOptions,
        context: &ExecutionContext,
    ) -> ExecutionResult {
        if let Err(e) = self.validate_tenant_for_table(table, &context.tenant_id) {
            return ExecutionResult::error(e);
        }
        let id = arguments.get("id");
        let tenant_column = self.tenant_column_for_table(table);

        if options.dry_run {
            let where_clause = if let Some(tc) = &tenant_column {
                format!(" WHERE id = $1 AND {} = $2", tc)
            } else {
                " WHERE id = $1".to_string()
            };
            return ExecutionResult::dry_run(DryRunResult {
                dry_run: true,
                would_affect: json!({
                    table: { "select": 1 }
                }),
                preview: Some(json!({
                    "query": format!("SELECT * FROM {}{}", table, where_clause),
                    "params": if tenant_column.is_some() { json!([id, context.tenant_id]) } else { json!([id]) }
                })),
            });
        }

        // Execute actual query
        let pool = match &self.pool {
            Some(p) => p,
            None => {
                return ExecutionResult::error("Database connection not configured");
            }
        };

        // Get the readable columns for this table (or use * if not specified)
        let columns = self.get_readable_columns(table);
        let column_list = if columns.is_empty() {
            "*".to_string()
        } else {
            columns.join(", ")
        };

        // Build optional tenant condition - embed directly since it comes from trusted token
        let tenant_condition = tenant_column.as_ref().map(|tc| {
            if context.tenant_id.parse::<i64>().is_ok() {
                format!("{} = {}", tc, context.tenant_id)
            } else {
                format!("{} = '{}'", tc, context.tenant_id.replace("'", "''"))
            }
        });

        // Parse id as i64 for query (common case)
        let id_value: i64 = match id {
            Some(v) => v.as_i64().unwrap_or(0),
            None => return ExecutionResult::error("Missing required field: id"),
        };

        // Get primary key column from schema (or fallback to convention)
        let pk_column = self.get_primary_key_column(table);

        let query = if let Some(tc) = tenant_condition {
            format!(
                "SELECT {} FROM {} WHERE {} = {} AND {}",
                column_list, table, pk_column, id_value, tc
            )
        } else {
            format!(
                "SELECT {} FROM {} WHERE {} = {}",
                column_list, table, pk_column, id_value
            )
        };

        tracing::debug!("Executing GET query: {}", query);

        let result = sqlx::query(&query).fetch_optional(pool).await;

        match result {
            Ok(Some(row)) => {
                let data = row_to_json(&row, &columns);
                ExecutionResult::success_json(data)
            }
            Ok(None) => ExecutionResult::success_json(json!({
                "data": null,
                "message": "Record not found"
            })),
            Err(e) => ExecutionResult::error(format!("Database error: {}", e)),
        }
    }

    /// Execute a LIST operation.
    async fn execute_list(
        &self,
        table: &str,
        arguments: &Value,
        options: &CallToolOptions,
        context: &ExecutionContext,
    ) -> ExecutionResult {
        if let Err(e) = self.validate_tenant_for_table(table, &context.tenant_id) {
            return ExecutionResult::error(e);
        }
        let limit = arguments
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(50);
        let offset = arguments
            .get("offset")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        // Apply max rows limit from role
        let max_rows = self.role.max_rows_per_query.unwrap_or(1000);
        let effective_limit = limit.min(max_rows);
        let tenant_column = self.tenant_column_for_table(table);

        if options.dry_run {
            let where_clause = if let Some(tc) = &tenant_column {
                format!(" WHERE {} = $1", tc)
            } else {
                "".to_string()
            };
            return ExecutionResult::dry_run(DryRunResult {
                dry_run: true,
                would_affect: json!({
                    table: { "select": "unknown" }
                }),
                preview: Some(json!({
                    "query": format!("SELECT * FROM {}{} LIMIT {} OFFSET {}", table, where_clause, effective_limit, offset),
                    "params": if tenant_column.is_some() { json!([context.tenant_id]) } else { json!([]) }
                })),
            });
        }

        // Execute actual query
        let pool = match &self.pool {
            Some(p) => p,
            None => {
                return ExecutionResult::error("Database connection not configured");
            }
        };

        // Get the readable columns for this table (or use * if not specified)
        let columns = self.get_readable_columns(table);
        let column_list = if columns.is_empty() {
            "*".to_string()
        } else {
            columns.join(", ")
        };

        // Build filter conditions from arguments (excluding limit/offset)
        // Tenant ID comes from the trusted token, so we can safely embed it.
        let mut conditions: Vec<String> = Vec::new();
        if let Some(tc) = &tenant_column {
            let tenant_condition = if context.tenant_id.parse::<i64>().is_ok() {
                format!("{} = {}", tc, context.tenant_id)
            } else {
                format!("{} = '{}'", tc, context.tenant_id.replace("'", "''"))
            };
            conditions.push(tenant_condition);
        }

        // Add user-provided filter conditions
        let empty_map = serde_json::Map::new();
        let args_map = arguments.as_object().unwrap_or(&empty_map);
        for (key, value) in args_map {
            if key == "limit" || key == "offset" {
                continue;
            }
            // Validate column name (alphanumeric and underscore only)
            if !key.chars().all(|c| c.is_alphanumeric() || c == '_') {
                continue;
            }
            if let Some(s) = value.as_str() {
                conditions.push(format!("{} = '{}'", key, s.replace("'", "''")));
            } else if let Some(n) = value.as_i64() {
                conditions.push(format!("{} = {}", key, n));
            } else if let Some(b) = value.as_bool() {
                conditions.push(format!("{} = {}", key, b));
            }
        }

        let query = if conditions.is_empty() {
            format!(
                "SELECT {} FROM {} LIMIT {} OFFSET {}",
                column_list, table, effective_limit, offset
            )
        } else {
            format!(
                "SELECT {} FROM {} WHERE {} LIMIT {} OFFSET {}",
                column_list,
                table,
                conditions.join(" AND "),
                effective_limit,
                offset
            )
        };

        tracing::info!("Executing LIST query: {}", query);

        let result = sqlx::query(&query).fetch_all(pool).await;

        match result {
            Ok(rows) => {
                let data: Vec<Value> = rows.iter().map(|r| row_to_json(r, &columns)).collect();
                ExecutionResult::success_json(json!({
                    "data": data,
                    "count": data.len(),
                    "limit": effective_limit,
                    "offset": offset
                }))
            }
            Err(e) => ExecutionResult::error(format!("Database error: {}", e)),
        }
    }

    /// Get readable columns for a table from role.
    fn get_readable_columns(&self, table: &str) -> Vec<String> {
        self.role
            .get_readable_columns(table)
            .and_then(|cols| cols.as_list())
            .map(|cols| cols.to_vec())
            .unwrap_or_default()
    }

    /// Execute a CREATE operation.
    async fn execute_create(
        &self,
        table: &str,
        arguments: &Value,
        options: &CallToolOptions,
        context: &ExecutionContext,
    ) -> ExecutionResult {
        if let Err(e) = self.validate_tenant_for_table(table, &context.tenant_id) {
            return ExecutionResult::error(e);
        }
        let tenant_column = self.tenant_column_for_table(table);
        if options.dry_run {
            return ExecutionResult::dry_run(DryRunResult {
                dry_run: true,
                would_affect: json!({
                    table: { "insert": 1 }
                }),
                preview: Some(json!({
                    "operation": "INSERT",
                    "table": table,
                    "data": arguments,
                    "tenant_id": context.tenant_id
                })),
            });
        }

        // Execute actual insert
        let pool = match &self.pool {
            Some(p) => p,
            None => {
                return ExecutionResult::error("Database connection not configured");
            }
        };

        // Build INSERT statement from arguments
        let empty_map = serde_json::Map::new();
        let obj = arguments.as_object().unwrap_or(&empty_map);

        // SECURITY: Remove tenant column from user input - tenant is ALWAYS set from Biscuit token
        // This prevents any attempt by users to override tenant isolation
        let mut final_args = obj.clone();
        if let Some(tc) = &tenant_column {
            final_args.remove(tc);
        }

        // Validate required fields and apply default values from role's creatable column constraints
        if let Some(creatable) = self.role.get_creatable_columns(table) {
            if let Some(constraints_map) = creatable.as_map() {
                for (col_name, constraints) in constraints_map {
                    // Skip tenant column - it's handled separately from the token
                    if let Some(tc) = &tenant_column {
                        if col_name == tc {
                            continue;
                        }
                    }

                    let has_value = final_args.contains_key(col_name);

                    // Check required constraint
                    if constraints.required && !has_value {
                        return ExecutionResult::error(format!(
                            "Missing required field: '{}'",
                            col_name
                        ));
                    }

                    // If column not provided and has a default, apply it
                    if !has_value {
                        if let Some(default_val) = &constraints.default {
                            final_args.insert(col_name.clone(), default_val.clone());
                        }
                    }
                }
            }
        }

        // Start with tenant column (if table is tenant-scoped)
        // SECURITY: Tenant value ALWAYS comes from the Biscuit token, never from user input
        let mut columns: Vec<String> = Vec::new();
        let mut value_strs: Vec<String> = Vec::new();
        if let Some(tc) = &tenant_column {
            columns.push(tc.clone());
            let tenant_value = if context.tenant_id.parse::<i64>().is_ok() {
                context.tenant_id.clone()
            } else {
                format!("'{}'", context.tenant_id.replace("'", "''"))
            };
            value_strs.push(tenant_value);
        }

        for (key, value) in &final_args {
            // Validate column name (alphanumeric and underscore only)
            if !key.chars().all(|c| c.is_alphanumeric() || c == '_') {
                continue;
            }
            columns.push(key.clone());
            // Convert value to SQL literal
            if let Some(s) = value.as_str() {
                value_strs.push(format!("'{}'", s.replace("'", "''")));
            } else if let Some(n) = value.as_i64() {
                value_strs.push(n.to_string());
            } else if let Some(f) = value.as_f64() {
                value_strs.push(f.to_string());
            } else if let Some(b) = value.as_bool() {
                value_strs.push(b.to_string());
            } else if value.is_null() {
                value_strs.push("NULL".to_string());
            } else {
                // For complex types, convert to JSON string
                value_strs.push(format!("'{}'", value.to_string().replace("'", "''")));
            }
        }

        let query = format!(
            "INSERT INTO {} ({}) VALUES ({}) RETURNING *",
            table,
            columns.join(", "),
            value_strs.join(", ")
        );

        tracing::debug!("Executing CREATE query: {}", query);

        match sqlx::query(&query).fetch_one(pool).await {
            Ok(row) => {
                let data = row_to_json(&row, &[]);
                ExecutionResult::success_json(json!({
                    "data": data,
                    "message": "Record created successfully"
                }))
            }
            Err(e) => ExecutionResult::error(format!("Database error: {}", e)),
        }
    }

    /// Execute an UPDATE operation.
    async fn execute_update(
        &self,
        table: &str,
        arguments: &Value,
        options: &CallToolOptions,
        context: &ExecutionContext,
    ) -> ExecutionResult {
        if let Err(e) = self.validate_tenant_for_table(table, &context.tenant_id) {
            return ExecutionResult::error(e);
        }
        let id = arguments.get("id");
        let tenant_column = self.tenant_column_for_table(table);

        if options.dry_run {
            return ExecutionResult::dry_run(DryRunResult {
                dry_run: true,
                would_affect: json!({
                    table: { "update": 1 }
                }),
                preview: Some(json!({
                    "operation": "UPDATE",
                    "table": table,
                    "id": id,
                    "changes": arguments,
                    "tenant_id": context.tenant_id
                })),
            });
        }

        // Execute actual update
        let pool = match &self.pool {
            Some(p) => p,
            None => {
                return ExecutionResult::error("Database connection not configured");
            }
        };

        let id_value: i64 = match id {
            Some(v) => v.as_i64().unwrap_or(0),
            None => return ExecutionResult::error("Missing required field: id"),
        };

        // Get updatable columns for this table from role definition
        let updatable_columns = self.role.tables.get(table).map(|p| &p.updatable);

        // Build UPDATE statement from arguments (excluding id)
        let empty_map = serde_json::Map::new();
        let obj = arguments.as_object().unwrap_or(&empty_map);
        let mut set_clauses = Vec::new();
        let mut rejected_columns = Vec::new();

        for (key, value) in obj {
            if key == "id" {
                continue;
            }
            // Validate column name (alphanumeric and underscore only)
            if !key.chars().all(|c| c.is_alphanumeric() || c == '_') {
                continue;
            }

            // Check if column is updatable according to role permissions
            let is_updatable = match updatable_columns {
                Some(cols) => cols.contains(key),
                None => false, // If no table permissions, default to not updatable
            };

            if !is_updatable {
                rejected_columns.push(key.clone());
                continue; // Skip non-updatable columns
            }

            // Check restrict_to constraint from role definition
            if let Some(cols) = updatable_columns {
                if let Some(constraints) = cols.get_constraints(key) {
                    if let Some(restrict_to) = &constraints.restrict_to {
                        if !restrict_to.contains(value) {
                            return ExecutionResult::error(format!(
                                "Value '{}' for column '{}' not in allowed values: {:?}",
                                value, key, restrict_to
                            ));
                        }
                    }
                }
            }

            // Build SET clause with inline values
            if let Some(s) = value.as_str() {
                set_clauses.push(format!("{} = '{}'", key, s.replace("'", "''")));
            } else if let Some(n) = value.as_i64() {
                set_clauses.push(format!("{} = {}", key, n));
            } else if let Some(b) = value.as_bool() {
                set_clauses.push(format!("{} = {}", key, b));
            } else if value.is_null() {
                set_clauses.push(format!("{} = NULL", key));
            }
        }

        // Log rejected columns for audit/debugging
        if !rejected_columns.is_empty() {
            tracing::warn!(
                "UPDATE on {} rejected non-updatable columns: {:?}",
                table,
                rejected_columns
            );
        }

        if set_clauses.is_empty() {
            return ExecutionResult::error(format!(
                "No updatable fields provided. Rejected columns: {:?}",
                rejected_columns
            ));
        }

        // Build optional tenant condition - embed directly since it comes from trusted token
        let tenant_condition = tenant_column.as_ref().map(|tc| {
            if context.tenant_id.parse::<i64>().is_ok() {
                format!("{} = {}", tc, context.tenant_id)
            } else {
                format!("{} = '{}'", tc, context.tenant_id.replace("'", "''"))
            }
        });

        // Get primary key column from schema (or fallback to convention)
        let pk_column = self.get_primary_key_column(table);

        let query = if let Some(tc) = tenant_condition {
            format!(
                "UPDATE {} SET {} WHERE {} = {} AND {} RETURNING *",
                table,
                set_clauses.join(", "),
                pk_column,
                id_value,
                tc
            )
        } else {
            format!(
                "UPDATE {} SET {} WHERE {} = {} RETURNING *",
                table,
                set_clauses.join(", "),
                pk_column,
                id_value
            )
        };

        tracing::debug!("Executing UPDATE query: {}", query);

        match sqlx::query(&query).fetch_optional(pool).await {
            Ok(Some(row)) => {
                let data = row_to_json(&row, &[]);
                ExecutionResult::success_json(json!({
                    "data": data,
                    "message": "Record updated successfully"
                }))
            }
            Ok(None) => ExecutionResult::error("Record not found or access denied"),
            Err(e) => ExecutionResult::error(format!("Database error: {}", e)),
        }
    }

    /// Execute a DELETE operation.
    async fn execute_delete(
        &self,
        table: &str,
        arguments: &Value,
        options: &CallToolOptions,
        context: &ExecutionContext,
    ) -> ExecutionResult {
        if let Err(e) = self.validate_tenant_for_table(table, &context.tenant_id) {
            return ExecutionResult::error(e);
        }
        let id = arguments.get("id");
        let tenant_column = self.tenant_column_for_table(table);

        if options.dry_run {
            return ExecutionResult::dry_run(DryRunResult {
                dry_run: true,
                would_affect: json!({
                    table: { "delete": 1 }
                }),
                preview: Some(json!({
                    "operation": "DELETE",
                    "table": table,
                    "id": id,
                    "tenant_id": context.tenant_id
                })),
            });
        }

        // Execute actual delete
        let pool = match &self.pool {
            Some(p) => p,
            None => {
                return ExecutionResult::error("Database connection not configured");
            }
        };

        let id_value: i64 = match id {
            Some(v) => v.as_i64().unwrap_or(0),
            None => return ExecutionResult::error("Missing required field: id"),
        };

        // Build optional tenant condition - embed directly since it comes from trusted token
        let tenant_condition = tenant_column.as_ref().map(|tc| {
            if context.tenant_id.parse::<i64>().is_ok() {
                format!("{} = {}", tc, context.tenant_id)
            } else {
                format!("{} = '{}'", tc, context.tenant_id.replace("'", "''"))
            }
        });

        // Get primary key column from schema (or fallback to convention)
        let pk_column = self.get_primary_key_column(table);

        let query = if let Some(tc) = tenant_condition {
            format!(
                "DELETE FROM {} WHERE {} = {} AND {} RETURNING {}",
                table, pk_column, id_value, tc, pk_column
            )
        } else {
            format!(
                "DELETE FROM {} WHERE {} = {} RETURNING {}",
                table, pk_column, id_value, pk_column
            )
        };

        tracing::debug!("Executing DELETE query: {}", query);

        match sqlx::query(&query).fetch_optional(pool).await
        {
            Ok(Some(_)) => ExecutionResult::success_json(json!({
                "message": "Record deleted successfully",
                "id": id_value
            })),
            Ok(None) => ExecutionResult::error("Record not found or access denied"),
            Err(e) => ExecutionResult::error(format!("Database error: {}", e)),
        }
    }

    /// Execute a custom action (not currently supported).
    async fn execute_custom(
        &self,
        name: &str,
        _arguments: &Value,
        _options: &CallToolOptions,
        _context: &ExecutionContext,
    ) -> ExecutionResult {
        // Custom actions are not supported in the current version
        ExecutionResult::error(format!(
            "Custom action '{}' is not supported. Only standard CRUD operations (get, list, create, update, delete) are available.",
            name
        ))
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
    Custom { name: String },
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

/// Simple singularization (removes trailing 's' or 'es').
fn singularize(s: &str) -> String {
    // Common irregular plurals
    let irregulars = [
        ("people", "person"),
        ("children", "child"),
        ("men", "man"),
        ("women", "woman"),
        ("mice", "mouse"),
        ("geese", "goose"),
        ("teeth", "tooth"),
        ("feet", "foot"),
    ];
    
    for (plural, singular) in irregulars {
        if s == plural {
            return singular.to_string();
        }
    }
    
    // Words ending in 'ies' -> 'y' (e.g., categories -> category)
    if s.ends_with("ies") && s.len() > 3 {
        return format!("{}y", &s[..s.len() - 3]);
    }
    
    // Words ending in 'es' that should keep the 'e' (e.g., boxes -> box)
    // Be careful about words like "status" that end in "us" not "es"
    if s.ends_with("xes") || s.ends_with("ches") || s.ends_with("shes") || s.ends_with("sses") {
        return s[..s.len() - 2].to_string();
    }
    
    // Words ending in 'ves' -> 'f' or 'fe' (e.g., leaves -> leaf)
    if s.ends_with("ves") {
        return format!("{}f", &s[..s.len() - 3]);
    }
    
    // Words ending in 's' but not 'ss' or 'us' or 'is' (e.g., users -> user)
    if s.ends_with('s') && !s.ends_with("ss") && !s.ends_with("us") && !s.ends_with("is") {
        return s[..s.len() - 1].to_string();
    }
    
    // Return as-is if no rule matched
    s.to_string()
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

/// Convert a sqlx row to JSON, using provided columns or all columns if empty.
fn row_to_json(row: &sqlx::postgres::PgRow, columns: &[String]) -> Value {
    use bigdecimal::BigDecimal;
    use chrono::{DateTime, NaiveDate, NaiveDateTime, NaiveTime, Utc};
    use sqlx::Column;

    let mut obj = serde_json::Map::new();

    for col in row.columns() {
        let name = col.name();

        // If columns list is provided and non-empty, filter
        if !columns.is_empty() && !columns.iter().any(|c| c == name) {
            continue;
        }

        // Try to extract the value as different types
        // Order matters: try more specific types first
        let value: Value = 
            // Integer types
            if let Ok(v) = row.try_get::<i64, _>(name) {
                json!(v)
            } else if let Ok(v) = row.try_get::<i32, _>(name) {
                json!(v)
            } else if let Ok(v) = row.try_get::<i16, _>(name) {
                json!(v)
            } 
            // Floating point
            else if let Ok(v) = row.try_get::<f64, _>(name) {
                json!(v)
            } else if let Ok(v) = row.try_get::<f32, _>(name) {
                json!(v)
            }
            // BigDecimal for DECIMAL/NUMERIC columns
            else if let Ok(v) = row.try_get::<BigDecimal, _>(name) {
                // Convert to f64 for JSON, preserving precision
                use bigdecimal::ToPrimitive;
                json!(v.to_f64().unwrap_or(0.0))
            }
            // Optional BigDecimal
            else if let Ok(v) = row.try_get::<Option<BigDecimal>, _>(name) {
                match v {
                    Some(d) => {
                        use bigdecimal::ToPrimitive;
                        json!(d.to_f64().unwrap_or(0.0))
                    }
                    None => Value::Null,
                }
            }
            // Boolean
            else if let Ok(v) = row.try_get::<bool, _>(name) {
                json!(v)
            }
            // Timestamp with timezone
            else if let Ok(v) = row.try_get::<DateTime<Utc>, _>(name) {
                json!(v.to_rfc3339())
            }
            // Optional timestamp with timezone
            else if let Ok(v) = row.try_get::<Option<DateTime<Utc>>, _>(name) {
                match v {
                    Some(dt) => json!(dt.to_rfc3339()),
                    None => Value::Null,
                }
            }
            // Timestamp without timezone
            else if let Ok(v) = row.try_get::<NaiveDateTime, _>(name) {
                json!(v.format("%Y-%m-%dT%H:%M:%S").to_string())
            }
            // Optional timestamp without timezone
            else if let Ok(v) = row.try_get::<Option<NaiveDateTime>, _>(name) {
                match v {
                    Some(dt) => json!(dt.format("%Y-%m-%dT%H:%M:%S").to_string()),
                    None => Value::Null,
                }
            }
            // Date
            else if let Ok(v) = row.try_get::<NaiveDate, _>(name) {
                json!(v.format("%Y-%m-%d").to_string())
            }
            // Optional date
            else if let Ok(v) = row.try_get::<Option<NaiveDate>, _>(name) {
                match v {
                    Some(d) => json!(d.format("%Y-%m-%d").to_string()),
                    None => Value::Null,
                }
            }
            // Time
            else if let Ok(v) = row.try_get::<NaiveTime, _>(name) {
                json!(v.format("%H:%M:%S").to_string())
            }
            // Optional time
            else if let Ok(v) = row.try_get::<Option<NaiveTime>, _>(name) {
                match v {
                    Some(t) => json!(t.format("%H:%M:%S").to_string()),
                    None => Value::Null,
                }
            }
            // UUID
            else if let Ok(v) = row.try_get::<uuid::Uuid, _>(name) {
                json!(v.to_string())
            }
            // Optional UUID
            else if let Ok(v) = row.try_get::<Option<uuid::Uuid>, _>(name) {
                match v {
                    Some(u) => json!(u.to_string()),
                    None => Value::Null,
                }
            }
            // String
            else if let Ok(v) = row.try_get::<String, _>(name) {
                json!(v)
            }
            // JSON/JSONB
            else if let Ok(v) = row.try_get::<serde_json::Value, _>(name) {
                v
            }
            // Optional string (fallback)
            else if let Ok(v) = row.try_get::<Option<String>, _>(name) {
                match v {
                    Some(s) => json!(s),
                    None => Value::Null,
                }
            }
            // String array
            else if let Ok(v) = row.try_get::<Vec<String>, _>(name) {
                json!(v)
            }
            // Optional string array
            else if let Ok(v) = row.try_get::<Option<Vec<String>>, _>(name) {
                match v {
                    Some(arr) => json!(arr),
                    None => Value::Null,
                }
            }
            // Final fallback
            else {
                Value::Null
            };

        obj.insert(name.to_string(), value);
    }

    Value::Object(obj)
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
    fn test_singularize() {
        assert_eq!(singularize("users"), "user");
        assert_eq!(singularize("categories"), "category");
        assert_eq!(singularize("boxes"), "box");
        assert_eq!(singularize("status"), "status");
    }

    #[test]
    fn test_parse_tool_operation() {
        let role = RoleDefinition {
            name: "test".to_string(),
            description: None,
            approvals: None,
            tables: std::collections::HashMap::new(),
            blocked_tables: Vec::new(),
            max_rows_per_query: None,
            max_affected_rows: None,
        };
        let approval_manager = Arc::new(ApprovalManager::default());
        let executor = ToolExecutor::new(role, approval_manager);

        matches!(
            executor.parse_tool_operation("getUser"),
            ToolOperation::Get { table } if table == "user"
        );
        matches!(
            executor.parse_tool_operation("listUsers"),
            ToolOperation::List { table } if table == "user"
        );
        matches!(
            executor.parse_tool_operation("createTicket"),
            ToolOperation::Create { table } if table == "ticket"
        );
    }
}
