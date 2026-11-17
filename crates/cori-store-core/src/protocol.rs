// SPDX-License-Identifier: Apache-2.0
//! MCP protocol types.
//!
//! The tool definitions a store role is given, the options of a call, and what a call answers.

use serde::{Deserialize, Serialize};
use serde_json::Value;

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

/// Options for tool calls.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CallToolOptions {
    #[serde(rename = "dryRun", default)]
    pub dry_run: bool,
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
