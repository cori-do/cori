//! API request and response types.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// =============================================================================
// Schema Browser Types
// =============================================================================

/// Response for schema listing.
#[derive(Debug, Serialize, Deserialize)]
pub struct SchemaResponse {
    pub tables: Vec<TableSchemaResponse>,
    pub refreshed_at: DateTime<Utc>,
}

/// Table schema information.
#[derive(Debug, Serialize, Deserialize)]
pub struct TableSchemaResponse {
    pub schema: String,
    pub name: String,
    pub columns: Vec<ColumnSchemaResponse>,
    pub primary_key: Vec<String>,
    pub foreign_keys: Vec<ForeignKeyResponse>,
    pub detected_tenant_column: Option<String>,
    pub configured_tenant_column: Option<String>,
    pub is_global: bool,
}

/// Column schema information.
#[derive(Debug, Serialize, Deserialize)]
pub struct ColumnSchemaResponse {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
    pub default: Option<String>,
    pub is_primary_key: bool,
    pub is_tenant_column: bool,
}

/// Foreign key information.
#[derive(Debug, Serialize, Deserialize)]
pub struct ForeignKeyResponse {
    pub name: String,
    pub columns: Vec<String>,
    pub references_table: String,
    pub references_columns: Vec<String>,
}

// =============================================================================
// Role Management Types
// =============================================================================

/// Request to create or update a role.
#[derive(Debug, Serialize, Deserialize)]
pub struct RoleRequest {
    pub name: String,
    pub description: Option<String>,
    pub tables: HashMap<String, TablePermissionRequest>,
    pub max_rows_per_query: Option<u64>,
    pub max_affected_rows: Option<u64>,
    pub blocked_operations: Vec<String>,
}

/// Permission configuration for a table.
#[derive(Debug, Serialize, Deserialize)]
pub struct TablePermissionRequest {
    pub operations: Vec<String>,
    pub readable: ReadableColumnsRequest,
    pub editable: HashMap<String, ColumnConstraintRequest>,
}

/// Readable columns specification.
#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ReadableColumnsRequest {
    All(String), // "*"
    List(Vec<String>),
}

/// Column constraint configuration.
#[derive(Debug, Serialize, Deserialize)]
pub struct ColumnConstraintRequest {
    pub allowed_values: Option<Vec<String>>,
    pub pattern: Option<String>,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub requires_approval: bool,
}

/// Response for role listing.
#[derive(Debug, Serialize, Deserialize)]
pub struct RoleListResponse {
    pub roles: Vec<RoleSummary>,
}

/// Role summary for listing.
#[derive(Debug, Serialize, Deserialize)]
pub struct RoleSummary {
    pub name: String,
    pub description: Option<String>,
    pub table_count: usize,
    pub has_custom_actions: bool,
}

/// Full role response.
#[derive(Debug, Serialize, Deserialize)]
pub struct RoleResponse {
    pub name: String,
    pub description: Option<String>,
    pub tables: HashMap<String, TablePermissionResponse>,
    pub max_rows_per_query: Option<u64>,
    pub max_affected_rows: Option<u64>,
    pub blocked_operations: Vec<String>,
    pub custom_actions: Vec<CustomActionResponse>,
}

/// Permission configuration response.
#[derive(Debug, Serialize, Deserialize)]
pub struct TablePermissionResponse {
    pub operations: Vec<String>,
    pub readable: Vec<String>,
    pub readable_all: bool,
    pub editable: HashMap<String, ColumnConstraintResponse>,
}

/// Column constraint response.
#[derive(Debug, Serialize, Deserialize)]
pub struct ColumnConstraintResponse {
    pub allowed_values: Option<Vec<String>>,
    pub pattern: Option<String>,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub requires_approval: bool,
}

/// Custom action response.
#[derive(Debug, Serialize, Deserialize)]
pub struct CustomActionResponse {
    pub name: String,
    pub description: Option<String>,
    pub requires_approval: bool,
}

// =============================================================================
// Token Minting Types
// =============================================================================

/// Request to mint a role token.
#[derive(Debug, Serialize, Deserialize)]
pub struct MintRoleTokenRequest {
    pub role: String,
}

/// Request to attenuate a token.
#[derive(Debug, Serialize, Deserialize)]
pub struct AttenuateTokenRequest {
    pub base_token: String,
    pub tenant: String,
    pub expires_in_hours: Option<u64>,
}

/// Request to mint an agent token directly.
#[derive(Debug, Serialize, Deserialize)]
pub struct MintAgentTokenRequest {
    pub role: String,
    pub tenant: String,
    pub expires_in_hours: Option<u64>,
}

/// Token response.
#[derive(Debug, Serialize, Deserialize)]
pub struct TokenResponse {
    pub token: String,
    #[serde(rename = "type")]
    pub token_type: String,
    pub role: Option<String>,
    pub tenant: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
}

/// Token inspection response.
#[derive(Debug, Serialize, Deserialize)]
pub struct TokenInspectResponse {
    #[serde(rename = "type")]
    pub token_type: String,
    pub role: Option<String>,
    pub tenant: Option<String>,
    pub tables: Option<Vec<String>>,
    pub block_count: usize,
    pub valid: bool,
    pub expires_at: Option<DateTime<Utc>>,
}

// =============================================================================
// Audit Log Types
// =============================================================================

/// Query parameters for audit logs.
#[derive(Debug, Deserialize)]
pub struct AuditQueryParams {
    pub tenant_id: Option<String>,
    pub role: Option<String>,
    pub event_type: Option<String>,
    pub start_time: Option<DateTime<Utc>>,
    pub end_time: Option<DateTime<Utc>>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

/// Audit event response.
#[derive(Debug, Serialize, Deserialize)]
pub struct AuditEventResponse {
    pub event_id: String,
    pub occurred_at: DateTime<Utc>,
    pub event_type: String,
    pub tenant_id: Option<String>,
    pub role: Option<String>,
    pub original_query: Option<String>,
    pub rewritten_query: Option<String>,
    pub tables: Option<Vec<String>>,
    pub row_count: Option<u64>,
    pub duration_ms: Option<u64>,
    pub error: Option<String>,
    pub tool_name: Option<String>,
    pub approval_id: Option<String>,
}

/// Audit log listing response.
#[derive(Debug, Serialize, Deserialize)]
pub struct AuditListResponse {
    pub events: Vec<AuditEventResponse>,
    pub total: usize,
    pub has_more: bool,
}

// =============================================================================
// Approval Types
// =============================================================================

/// Approval request response.
#[derive(Debug, Serialize, Deserialize)]
pub struct ApprovalResponse {
    pub id: String,
    pub tool_name: String,
    pub arguments: serde_json::Value,
    pub approval_fields: Vec<String>,
    pub status: String,
    pub tenant_id: String,
    pub role: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub decided_at: Option<DateTime<Utc>>,
    pub decided_by: Option<String>,
    pub reason: Option<String>,
}

/// List of approvals.
#[derive(Debug, Serialize, Deserialize)]
pub struct ApprovalListResponse {
    pub approvals: Vec<ApprovalResponse>,
    pub pending_count: usize,
}

/// Decision on an approval.
#[derive(Debug, Serialize, Deserialize)]
pub struct ApprovalDecisionRequest {
    pub action: String, // "approve" or "reject"
    pub reason: Option<String>,
    pub decided_by: Option<String>,
}

// =============================================================================
// Settings Types
// =============================================================================

/// Settings response.
#[derive(Debug, Serialize, Deserialize)]
pub struct SettingsResponse {
    pub upstream: UpstreamSettingsResponse,
    pub mcp: McpSettingsResponse,
    pub dashboard: DashboardSettingsResponse,
    pub audit: AuditSettingsResponse,
    pub guardrails: GuardrailsSettingsResponse,
    pub tenancy: TenancySettingsResponse,
}

/// Upstream database settings.
#[derive(Debug, Serialize, Deserialize)]
pub struct UpstreamSettingsResponse {
    pub host: String,
    pub port: u16,
    pub database: String,
    pub user: Option<String>,
    pub ssl_mode: Option<String>,
    pub connected: bool,
}

/// MCP settings.
#[derive(Debug, Serialize, Deserialize)]
pub struct McpSettingsResponse {
    pub enabled: bool,
    pub transport: String,
    pub http_port: Option<u16>,
}

/// Dashboard settings.
#[derive(Debug, Serialize, Deserialize)]
pub struct DashboardSettingsResponse {
    pub enabled: bool,
    pub listen_port: u16,
    pub auth_type: String,
}

/// Audit settings.
#[derive(Debug, Serialize, Deserialize)]
pub struct AuditSettingsResponse {
    pub enabled: bool,
    pub log_queries: bool,
    pub log_results: bool,
    pub retention_days: u32,
    pub storage_backend: String,
}

/// Guardrails settings.
#[derive(Debug, Serialize, Deserialize)]
pub struct GuardrailsSettingsResponse {
    pub max_rows_per_query: u64,
    pub max_affected_rows: u64,
    pub blocked_operations: Vec<String>,
}

/// Tenancy settings (from rules definition).
#[derive(Debug, Serialize, Deserialize)]
pub struct TenancySettingsResponse {
    pub table_count: usize,
    pub global_table_count: usize,
}

/// Settings update request.
#[derive(Debug, Serialize, Deserialize)]
pub struct SettingsUpdateRequest {
    pub guardrails: Option<GuardrailsSettingsUpdate>,
    pub audit: Option<AuditSettingsUpdate>,
}

/// Guardrails settings update.
#[derive(Debug, Serialize, Deserialize)]
pub struct GuardrailsSettingsUpdate {
    pub max_rows_per_query: Option<u64>,
    pub max_affected_rows: Option<u64>,
    pub blocked_operations: Option<Vec<String>>,
}

/// Audit settings update.
#[derive(Debug, Serialize, Deserialize)]
pub struct AuditSettingsUpdate {
    pub enabled: Option<bool>,
    pub log_queries: Option<bool>,
    pub log_results: Option<bool>,
    pub retention_days: Option<u32>,
}

// =============================================================================
// MCP Tool Preview Types
// =============================================================================

/// MCP tool preview for a role.
#[derive(Debug, Serialize, Deserialize)]
pub struct McpToolPreviewResponse {
    pub role: String,
    pub tools: Vec<McpToolResponse>,
}

/// MCP tool definition.
#[derive(Debug, Serialize, Deserialize)]
pub struct McpToolResponse {
    pub name: String,
    pub description: Option<String>,
    pub input_schema: serde_json::Value,
    pub annotations: McpToolAnnotationsResponse,
}

/// MCP tool annotations.
#[derive(Debug, Serialize, Deserialize)]
pub struct McpToolAnnotationsResponse {
    pub requires_approval: bool,
    pub dry_run_supported: bool,
    pub read_only: bool,
    pub approval_fields: Vec<String>,
}

// =============================================================================
// Generic Types
// =============================================================================

/// Generic success response.
#[derive(Debug, Serialize, Deserialize)]
pub struct SuccessResponse {
    pub success: bool,
    pub message: Option<String>,
}

/// Generic error response.
#[derive(Debug, Serialize, Deserialize)]
pub struct ErrorResponse {
    pub error: String,
    pub details: Option<String>,
}
