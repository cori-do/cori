use serde::{Deserialize, Serialize};

/// Top-level intent (idempotent) that can be executed or previewed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MutationIntent {
    pub intent_id: String,
    pub tenant_id: String,
    pub environment: String, // "prod" | "staging" | "dev"
    pub preview: bool,
    pub principal: Principal,
    pub plan: Plan,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Principal {
    pub id: String,
    pub roles: Vec<String>,
    pub attrs: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plan {
    pub steps: Vec<Step>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Step {
    pub id: String,
    pub kind: StepKind,
    pub action: String, // must match an ActionDefinition.name
    pub inputs: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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
    pub kind: StepKind,
    pub resource_kind: String,
    pub cerbos_action: String,
    pub input_schema: serde_json::Value, // JSON Schema (later enforced)
}
