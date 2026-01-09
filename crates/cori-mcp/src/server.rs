//! MCP server implementation.
//!
//! This module provides the main MCP server that handles tool discovery
//! and execution with role-driven dynamic tool generation.

use crate::approval::ApprovalManager;
use crate::error::McpError;
use crate::executor::{ExecutionContext, ExecutionResult, ToolExecutor};
use crate::http_transport::HttpServer;
use crate::protocol::*;
use crate::schema::DatabaseSchema;
use crate::tool_generator::ToolGenerator;
use crate::tools::ToolRegistry;
use cori_biscuit::PublicKey;
use cori_core::config::mcp::{McpConfig, Transport};
use cori_core::config::rules_definition::RulesDefinition;
use cori_core::RoleDefinition;
use serde_json::{json, Value};
use sqlx::PgPool;
use std::io::{BufRead, Write};
use std::sync::Arc;
use tokio::sync::mpsc;

/// The MCP server.
pub struct McpServer {
    config: McpConfig,
    tools: ToolRegistry,
    role: Option<RoleDefinition>,
    schema: Option<DatabaseSchema>,
    approval_manager: Arc<ApprovalManager>,
    tenant_id: Option<String>,
    executor: Option<ToolExecutor>,
    pool: Option<PgPool>,
    rules: Option<RulesDefinition>,
    /// Public key for token verification (required for HTTP transport).
    public_key: Option<PublicKey>,
}

impl McpServer {
    /// Create a new MCP server with the given configuration.
    pub fn new(config: McpConfig) -> Self {
        Self {
            config,
            tools: ToolRegistry::new(),
            role: None,
            schema: None,
            approval_manager: Arc::new(ApprovalManager::default()),
            tenant_id: None,
            executor: None,
            pool: None,
            rules: None,
            public_key: None,
        }
    }

    /// Set the role definition for dynamic tool generation.
    pub fn with_role(mut self, role: RoleDefinition) -> Self {
        self.executor = Some(self.build_executor(&role));
        self.role = Some(role);
        self
    }

    /// Set the database connection pool.
    pub fn with_pool(mut self, pool: PgPool) -> Self {
        self.pool = Some(pool.clone());
        // Update executor if it exists
        if let Some(role) = &self.role {
            self.executor = Some(self.build_executor(role).with_pool(pool));
        }
        self
    }

    /// Set rules definition for tenant configuration.
    pub fn with_rules(mut self, rules: RulesDefinition) -> Self {
        self.rules = Some(rules);
        if let Some(role) = &self.role {
            self.executor = Some(self.build_executor(role));
        }
        self
    }

    /// Set the database schema for tool generation.
    pub fn with_schema(mut self, schema: DatabaseSchema) -> Self {
        self.schema = Some(schema.clone());
        // Update executor if it exists
        if let Some(role) = &self.role {
            self.executor = Some(self.build_executor(role).with_schema(schema));
        }
        self
    }

    /// Set the tenant ID from the authenticated token.
    pub fn with_tenant_id(mut self, tenant_id: impl Into<String>) -> Self {
        self.tenant_id = Some(tenant_id.into());
        self
    }

    /// Set the public key for token verification.
    /// 
    /// This is required for HTTP transport. Without a public key,
    /// the HTTP server will reject all requests (unless auth is disabled).
    pub fn with_public_key(mut self, public_key: PublicKey) -> Self {
        self.public_key = Some(public_key);
        self
    }

    /// Set the approval manager.
    pub fn with_approval_manager(mut self, manager: Arc<ApprovalManager>) -> Self {
        self.approval_manager = manager;
        // Recreate executor with new approval manager
        if let Some(role) = &self.role {
            self.executor = Some(self.build_executor(role));
        }
        self
    }

    fn build_executor(&self, role: &RoleDefinition) -> ToolExecutor {
        let mut executor = ToolExecutor::new(role.clone(), self.approval_manager.clone());
        if let Some(rules) = &self.rules {
            executor = executor.with_rules(rules.clone());
        }
        if let Some(pool) = &self.pool {
            executor = executor.with_pool(pool.clone());
        }
        if let Some(schema) = &self.schema {
            executor = executor.with_schema(schema.clone());
        }
        executor
    }

    /// Get a mutable reference to the tool registry.
    pub fn tools_mut(&mut self) -> &mut ToolRegistry {
        &mut self.tools
    }

    /// Get a reference to the approval manager.
    pub fn approval_manager(&self) -> &Arc<ApprovalManager> {
        &self.approval_manager
    }

    /// Generate tools from role definition and schema.
    pub fn generate_tools(&mut self) {
        if let Some(role) = &self.role {
            let schema = self.schema.clone().unwrap_or_default();
            let generator = ToolGenerator::new(role.clone(), schema);
            let tools = generator.generate_all();

            for tool in tools {
                self.tools.register(tool);
            }

            tracing::info!(
                role = %role.name,
                tool_count = self.tools.list().len(),
                "Generated tools from role definition"
            );
        }
    }

    /// Start the MCP server.
    pub async fn run(&self) -> Result<(), McpError> {
        match self.config.transport {
            Transport::Stdio => self.run_stdio().await,
            Transport::Http => self.run_http().await,
        }
    }

    /// Run the server with stdio transport.
    async fn run_stdio(&self) -> Result<(), McpError> {
        tracing::info!("Starting MCP server with stdio transport");

        let stdin = std::io::stdin();
        let stdout = std::io::stdout();
        let mut stdout_lock = stdout.lock();

        for line in stdin.lock().lines() {
            let line = line?;
            if line.is_empty() {
                continue;
            }

            let request: JsonRpcRequest = serde_json::from_str(&line)?;
            if let Some(response) = self.handle_request(request).await {
                let response_json = serde_json::to_string(&response)?;
                writeln!(stdout_lock, "{}", response_json)?;
                stdout_lock.flush()?;
            }
        }

        Ok(())
    }

    /// Run the server with HTTP transport.
    pub async fn run_http(&self) -> Result<(), McpError> {
        tracing::info!(
            port = self.config.port,
            auth_enabled = self.public_key.is_some(),
            "Starting MCP server with HTTP transport"
        );


        // Create channel for request handling
        let (request_tx, mut request_rx) =
            mpsc::channel::<(JsonRpcRequest, mpsc::Sender<JsonRpcResponse>)>(100);

        // Clone self for the request handler task
        let tools = self.tools.clone();
        let role = self.role.clone();
        let tenant_id = self.tenant_id.clone();
        let approval_manager = self.approval_manager.clone();
        let config = self.config.clone();
        let pool = self.pool.clone();
        let schema = self.schema.clone();
        let rules = self.rules.clone();

        // Spawn request handler task
        tokio::spawn(async move {
            while let Some((request, response_tx)) = request_rx.recv().await {
                // Create a temporary server instance to handle the request
                let mut temp_server = McpServer::new(config.clone());
                temp_server.tools = tools.clone();
                temp_server.role = role.clone();
                temp_server.tenant_id = tenant_id.clone();
                temp_server.approval_manager = approval_manager.clone();
                temp_server.pool = pool.clone();
                temp_server.rules = rules.clone();
                temp_server.schema = schema.clone();

                if let Some(r) = &temp_server.role {
                    temp_server.executor = Some(temp_server.build_executor(r));
                }

                if let Some(response) = temp_server.handle_request(request).await {
                    let _ = response_tx.send(response).await;
                }
            }
        });

        // Start HTTP server with or without authentication
        let http_server = match &self.public_key {
            Some(pk) => HttpServer::with_auth(self.config.get_port(), request_tx, pk.clone()),
            None => {
                tracing::warn!(
                    "No public key configured - MCP HTTP server running WITHOUT authentication!"
                );
                HttpServer::without_auth(self.config.get_port(), request_tx)
            }
        };
        http_server.run().await
    }

    /// Handle a JSON-RPC request.
    pub async fn handle_request(&self, request: JsonRpcRequest) -> Option<JsonRpcResponse> {
        // MCP clients (including Claude Desktop) send JSON-RPC notifications with no `id`,
        // e.g. `notifications/initialized`. Notifications MUST NOT receive responses.
        // If we respond with an error, it will contain `"id": null`, which some clients reject.
        let id = request.id.clone();
        if id.is_none() {
            // Best-effort: handle/ignore known notifications, ignore unknown ones.
            match request.method.as_str() {
                "initialized" | "notifications/initialized" => {
                    tracing::debug!(method = %request.method, "Received initialized notification");
                }
                other if other.starts_with("notifications/") => {
                    tracing::debug!(method = %request.method, "Ignoring notification");
                }
                _ => {
                    tracing::debug!(
                        method = %request.method,
                        "Received request without id; treating as notification and not responding"
                    );
                }
            }
            return None;
        }

        match request.method.as_str() {
            "initialize" => Some(self.handle_initialize(id)),
            "initialized" => {
                // This is a notification, not a request - do not respond
                tracing::debug!("Received initialized notification");
                None
            },
            "notifications/initialized" => {
                // Notification variant used by Claude Desktop.
                tracing::debug!("Received notifications/initialized notification");
                None
            }
            "tools/list" => Some(self.handle_list_tools(id)),
            "tools/call" => Some(self.handle_call_tool(id, request.params).await),
            "shutdown" => Some(self.handle_shutdown(id)),
            // Approval-related methods
            "approvals/list" => Some(self.handle_list_approvals(id, request.params)),
            "approvals/get" => Some(self.handle_get_approval(id, request.params)),
            "approvals/approve" => Some(self.handle_approve(id, request.params)),
            "approvals/reject" => Some(self.handle_reject(id, request.params)),
            _ => Some(JsonRpcResponse::error(
                id,
                -32601,
                format!("Method not found: {}", request.method),
            )),
        }
    }

    fn handle_initialize(&self, id: Option<Value>) -> JsonRpcResponse {
        let result = json!({
            "protocolVersion": "2024-11-05",
            "serverInfo": {
                "name": "cori-mcp",
                "version": env!("CARGO_PKG_VERSION")
            },
            "capabilities": {
                "tools": {
                    "listChanged": true
                }
            }
        });
        JsonRpcResponse::success(id, result)
    }

    fn handle_list_tools(&self, id: Option<Value>) -> JsonRpcResponse {
        let tools: Vec<_> = self
            .tools
            .list()
            .iter()
            .map(|t| {
                json!({
                    "name": t.name,
                    "description": t.description,
                    "inputSchema": t.input_schema,
                    "annotations": t.annotations
                })
            })
            .collect();

        let result = json!({ "tools": tools });
        JsonRpcResponse::success(id, result)
    }

    async fn handle_call_tool(&self, id: Option<Value>, params: Option<Value>) -> JsonRpcResponse {
        let params: CallToolParams = match params {
            Some(p) => match serde_json::from_value(p) {
                Ok(params) => params,
                Err(e) => {
                    return JsonRpcResponse::error(id, -32602, format!("Invalid params: {}", e))
                }
            },
            None => return JsonRpcResponse::error(id, -32602, "Missing params"),
        };

        // Check if tool exists
        let tool = match self.tools.get(&params.name) {
            Some(t) => t.clone(),
            None => {
                return JsonRpcResponse::error(id, -32602, format!("Tool not found: {}", params.name))
            }
        };

        // Execute the tool
        if let Some(executor) = &self.executor {
            let context = ExecutionContext {
                tenant_id: self.tenant_id.clone().unwrap_or_else(|| "unknown".to_string()),
                role: self
                    .role
                    .as_ref()
                    .map(|r| r.name.clone())
                    .unwrap_or_else(|| "unknown".to_string()),
                connection_id: None,
            };

            let result = executor
                .execute(&tool, params.arguments, &params.options, &context)
                .await;

            return self.execution_result_to_response(id, result);
        }

        // Fallback: stub implementation
        if params.options.dry_run {
            let result = json!({
                "content": [{
                    "type": "json",
                    "json": {
                        "dryRun": true,
                        "wouldAffect": {},
                        "preview": null
                    }
                }]
            });
            return JsonRpcResponse::success(id, result);
        }

        let result = json!({
            "content": [{
                "type": "text",
                "text": format!("Tool {} executed (stub)", params.name)
            }]
        });
        JsonRpcResponse::success(id, result)
    }

    fn execution_result_to_response(
        &self,
        id: Option<Value>,
        result: ExecutionResult,
    ) -> JsonRpcResponse {
        // Claude Desktop expects MCP tool results as text/image content.
        // Our internal executor can produce structured JSON; serialize it to text here.
        fn content_to_json(c: &ToolContent) -> Value {
            match c {
                ToolContent::Text { text } => json!({"type": "text", "text": text}),
                ToolContent::Json { json } => {
                    let rendered = serde_json::to_string_pretty(json)
                        .unwrap_or_else(|_| json.to_string());
                    json!({"type": "text", "text": rendered})
                }
            }
        }

        if result.success {
            let response = json!({
                "content": result.content.iter().map(content_to_json).collect::<Vec<_>>(),
                "isError": false
            });
            JsonRpcResponse::success(id, response)
        } else {
            let response = json!({
                "content": result.content.iter().map(content_to_json).collect::<Vec<_>>(),
                "isError": true
            });
            JsonRpcResponse::success(id, response)
        }
    }

    fn handle_list_approvals(&self, id: Option<Value>, _params: Option<Value>) -> JsonRpcResponse {
        let tenant_id = self.tenant_id.as_deref();
        let approvals = self.approval_manager.list_pending(tenant_id);

        let result = json!({
            "approvals": approvals
        });
        JsonRpcResponse::success(id, result)
    }

    fn handle_get_approval(&self, id: Option<Value>, params: Option<Value>) -> JsonRpcResponse {
        let approval_id = params
            .as_ref()
            .and_then(|p| p.get("approvalId"))
            .and_then(|v| v.as_str());

        match approval_id {
            Some(aid) => match self.approval_manager.get(aid) {
                Some(approval) => JsonRpcResponse::success(id, json!(approval)),
                None => JsonRpcResponse::error(id, -32602, "Approval not found"),
            },
            None => JsonRpcResponse::error(id, -32602, "Missing approvalId"),
        }
    }

    fn handle_approve(&self, id: Option<Value>, params: Option<Value>) -> JsonRpcResponse {
        let approval_id = params
            .as_ref()
            .and_then(|p| p.get("approvalId"))
            .and_then(|v| v.as_str());

        let reason = params
            .as_ref()
            .and_then(|p| p.get("reason"))
            .and_then(|v| v.as_str())
            .map(String::from);

        let by = params
            .as_ref()
            .and_then(|p| p.get("by"))
            .and_then(|v| v.as_str())
            .unwrap_or("system");

        match approval_id {
            Some(aid) => match self.approval_manager.approve(aid, by, reason) {
                Ok(approval) => JsonRpcResponse::success(id, json!(approval)),
                Err(e) => JsonRpcResponse::error(id, -32602, e.to_string()),
            },
            None => JsonRpcResponse::error(id, -32602, "Missing approvalId"),
        }
    }

    fn handle_reject(&self, id: Option<Value>, params: Option<Value>) -> JsonRpcResponse {
        let approval_id = params
            .as_ref()
            .and_then(|p| p.get("approvalId"))
            .and_then(|v| v.as_str());

        let reason = params
            .as_ref()
            .and_then(|p| p.get("reason"))
            .and_then(|v| v.as_str())
            .map(String::from);

        let by = params
            .as_ref()
            .and_then(|p| p.get("by"))
            .and_then(|v| v.as_str())
            .unwrap_or("system");

        match approval_id {
            Some(aid) => match self.approval_manager.reject(aid, by, reason) {
                Ok(approval) => JsonRpcResponse::success(id, json!(approval)),
                Err(e) => JsonRpcResponse::error(id, -32602, e.to_string()),
            },
            None => JsonRpcResponse::error(id, -32602, "Missing approvalId"),
        }
    }

    fn handle_shutdown(&self, id: Option<Value>) -> JsonRpcResponse {
        tracing::info!("MCP server shutdown requested");
        JsonRpcResponse::success(id, json!(null))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_initialize() {
        let server = McpServer::new(McpConfig::default());
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(1)),
            method: "initialize".to_string(),
            params: None,
        };

        let response = server.handle_request(request).await.expect("Should return response for initialize");
        assert!(response.result.is_some());
        assert!(response.error.is_none());
    }

    #[tokio::test]
    async fn test_list_tools() {
        let server = McpServer::new(McpConfig::default());
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(1)),
            method: "tools/list".to_string(),
            params: None,
        };

        let response = server.handle_request(request).await.expect("Should return response for tools/list");
        assert!(response.result.is_some());
    }

    #[tokio::test]
    async fn test_call_nonexistent_tool() {
        let server = McpServer::new(McpConfig::default());
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(1)),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "nonexistent",
                "arguments": {}
            })),
        };

        let response = server.handle_request(request).await.expect("Should return response for tools/call");
        assert!(response.error.is_some());
    }

    #[tokio::test]
    async fn test_initialized_notification() {
        let server = McpServer::new(McpConfig::default());
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: None, // Notifications have no id
            method: "initialized".to_string(),
            params: None,
        };

        let response = server.handle_request(request).await;
        assert!(response.is_none(), "Notifications should not return a response");
    }
}
