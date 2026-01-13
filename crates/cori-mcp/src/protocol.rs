//! MCP protocol types.
//!
//! This module defines the JSON-RPC message types used by MCP.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// JSON-RPC request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: Option<Value>,
    pub method: String,
    #[serde(default)]
    pub params: Option<Value>,
}

/// JSON-RPC response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

/// JSON-RPC error.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl JsonRpcResponse {
    /// Create a success response.
    pub fn success(id: Option<Value>, result: Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: Some(result),
            error: None,
        }
    }

    /// Create an error response.
    pub fn error(id: Option<Value>, code: i32, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: None,
            error: Some(JsonRpcError {
                code,
                message: message.into(),
                data: None,
            }),
        }
    }
}

/// MCP server info response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerInfo {
    pub name: String,
    pub version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protocol_version: Option<String>,
}

/// MCP tool definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub annotations: Option<ToolAnnotations>,
}

/// Tool annotations (MCP extensions).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ToolAnnotations {
    #[serde(rename = "requiresApproval", skip_serializing_if = "Option::is_none")]
    pub requires_approval: Option<bool>,
    #[serde(rename = "dryRunSupported", skip_serializing_if = "Option::is_none")]
    pub dry_run_supported: Option<bool>,
    #[serde(rename = "readOnly", skip_serializing_if = "Option::is_none")]
    pub read_only: Option<bool>,
    #[serde(rename = "approvalFields", skip_serializing_if = "Option::is_none")]
    pub approval_fields: Option<Vec<String>>,
}

/// List tools response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListToolsResponse {
    pub tools: Vec<ToolDefinition>,
}

/// Call tool request parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallToolParams {
    pub name: String,
    #[serde(default)]
    pub arguments: Value,
    #[serde(default)]
    pub options: CallToolOptions,
}

/// Options for tool calls.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CallToolOptions {
    #[serde(rename = "dryRun", default)]
    pub dry_run: bool,
}

/// Call tool response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallToolResponse {
    pub content: Vec<ToolContent>,
    #[serde(rename = "isError", skip_serializing_if = "Option::is_none")]
    pub is_error: Option<bool>,
}

/// Tool response content.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ToolContent {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "json")]
    Json { json: Value },
}

/// Dry-run result for mutations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DryRunResult {
    #[serde(rename = "dryRun")]
    pub dry_run: bool,
    #[serde(rename = "wouldAffect")]
    pub would_affect: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview: Option<Value>,
}

/// Approval pending result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalPendingResult {
    pub status: String,
    #[serde(rename = "approvalId")]
    pub approval_id: String,
    pub message: String,
}

/// Request context passed from HTTP transport to MCP server.
///
/// Contains authentication information extracted from the verified Biscuit token.
/// In HTTP mode, this is populated per-request from the Authorization header.
/// In stdio mode, this is set once at startup from the environment token.
#[derive(Debug, Clone, Default)]
pub struct RequestContext {
    /// Tenant ID from the attenuated token (required for tenant-scoped operations).
    pub tenant_id: Option<String>,
    /// Role name from the token's authority block.
    pub role: Option<String>,
}

