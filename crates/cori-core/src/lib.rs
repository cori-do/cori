use serde::{Deserialize, Serialize};
use uuid::Uuid;

// Configuration types shared across all Cori crates
pub mod config;

// Re-export commonly used config types for convenience
pub use config::{
    ApprovalConfig,
    ApprovalRequirement,
    // Main config
    ApprovalsConfig,
    AuditConfig,
    BiscuitConfig,
    // Role definition types needed by cori-mcp
    ColumnList,
    CoriConfig,
    CreatableColumnConstraints,
    CreatableColumns,
    DashboardConfig,
    DeletablePermission,
    GroupDefinition,
    GuardrailsConfig,
    McpConfig,
    ReadableConfig,
    ReadableConfigFull,
    RoleDefinition,
    RulesDefinition,
    // New schema-based config types
    SchemaDefinition,
    SoftDeleteConfig,
    TablePermissions,
    TableRules,
    TenantConfig,
    TypeDef,
    TypesDefinition,
    UpdatableColumnConstraints,
    UpdatableColumns,
    UpstreamConfig,
    VirtualSchemaConfig,
};

/// Top-level intent (idempotent) that can be executed or previewed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MutationIntent {
    /// Protocol/schema version for forwards/backwards compatibility.
    /// Matches `schemas/MutationIntent.schema.json` (default: "0.1.0").
    #[serde(default = "default_schema_version")]
    pub schema_version: String,
    pub intent_id: String,
    pub tenant_id: String,
    pub environment: String, // "prod" | "staging" | "dev"
    pub preview: bool,
    pub principal: Principal,
    pub plan: Plan,
    /// Optional user request metadata (e.g., original NL request, ticket id).
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub request: serde_json::Value,
    /// Free-form extension metadata (non-authoritative).
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub meta: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Principal {
    pub id: String,
    pub roles: Vec<String>,
    /// Arbitrary attributes passed to policy evaluation.
    /// Schema expects an object; runtime should treat this as such.
    pub attrs: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plan {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    pub steps: Vec<Step>,
    /// Free-form extension metadata (non-authoritative).
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub meta: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Step {
    pub id: String,
    pub kind: StepKind,
    pub action: String, // must match an ActionDefinition.name
    pub inputs: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub depends_on: Option<Vec<String>>,
    /// Free-form step metadata (non-authoritative).
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub meta: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum StepKind {
    Query,
    Mutation,
    Control,
}

/// Catalog entry for an executable action.
/// For MVP, this is enough to constrain planning and validate inputs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionDefinition {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub kind: StepKind,
    pub resource_kind: String,
    /// The policy action type (e.g., "get", "list", "update_fields", "soft_delete").
    /// Used for authorization checks and audit logging.
    #[serde(alias = "cerbos_action")] // backwards compatibility
    pub policy_action: String,
    pub input_schema: serde_json::Value, // JSON Schema (later enforced)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effects: Option<Vec<String>>,
    #[serde(default)]
    pub meta: serde_json::Value,
}

/// Audit event type (matches `schemas/AuditEvent.schema.json`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AuditEventType {
    IntentReceived,
    PlanValidated,
    PolicyChecked,
    ApprovalRequired,
    Approved,
    ActionPreviewed,
    ActionExecuted,
    VerificationFailed,
    Committed,
    Compensated,
    Failed,
}

/// Portable audit evidence event (matches `schemas/AuditEvent.schema.json`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEvent {
    pub event_id: Uuid,
    /// UTC timestamp (RFC3339).
    pub occurred_at: chrono::DateTime<chrono::Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sequence: Option<u64>,

    pub tenant_id: String,
    pub intent_id: String,
    pub principal_id: String,
    pub step_id: String,
    pub event_type: AuditEventType,
    pub action: String,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_id: Option<String>,

    pub allowed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<bool>,

    /// Policy decision payload (obligations + explanation metadata).
    #[serde(default)]
    pub decision: serde_json::Map<String, serde_json::Value>,
    /// Execution/preview outcome (affected_count, diff summary, errors).
    #[serde(default)]
    pub outcome: serde_json::Map<String, serde_json::Value>,

    /// Optional tamper-evident fields for enterprise/WORM storage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub integrity: Option<AuditIntegrity>,

    /// Free-form extension metadata (non-authoritative).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditIntegrity {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prev_hash: Option<String>,
}

fn default_schema_version() -> String {
    "0.1.0".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audit_event_serialization_validates_against_schema() {
        let e = AuditEvent {
            event_id: Uuid::new_v4(),
            occurred_at: chrono::Utc::now(),
            sequence: Some(0),
            tenant_id: "acme".to_string(),
            intent_id: "intent-123".to_string(),
            principal_id: "user:alice".to_string(),
            step_id: "__intent__".to_string(),
            event_type: AuditEventType::IntentReceived,
            action: "__intent__".to_string(),
            resource_kind: None,
            resource_id: None,
            allowed: true,
            preview: Some(true),
            decision: serde_json::Map::new(),
            outcome: serde_json::Map::new(),
            integrity: None,
            meta: None,
        };

        let instance = serde_json::to_value(&e).expect("audit event must serialize");
        let schema: serde_json::Value =
            serde_json::from_str(include_str!("../../../schemas/AuditEvent.schema.json"))
                .expect("schema must parse");

        let validator = jsonschema::draft202012::options()
            .build(&schema)
            .expect("schema must compile");

        if !validator.is_valid(&instance) {
            let mut msgs = Vec::new();
            for (idx, err) in validator.iter_errors(&instance).take(20).enumerate() {
                msgs.push(format!("{}: {}", idx + 1, err));
            }
            panic!("audit event did not validate: {}", msgs.join("; "));
        }
    }
}
