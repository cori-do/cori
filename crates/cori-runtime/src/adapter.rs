use async_trait::async_trait;
use cori_core::ActionDefinition;

#[derive(Debug, Clone)]
pub struct ActionOutcome {
    pub affected_count: u64,
    pub preview_diff: Option<serde_json::Value>,
    pub output: serde_json::Value,
}

#[async_trait]
pub trait DataAdapter: Send + Sync {
    /// Load minimal resource attributes needed for policy checks.
    async fn load_resource_attrs(
        &self,
        tenant_id: &str,
        resource_kind: &str,
        resource_id: &str,
    ) -> anyhow::Result<serde_json::Value>;

    /// Execute an action (or preview it). Must be deterministic and bounded.
    async fn execute_action(
        &self,
        tenant_id: &str,
        action: &ActionDefinition,
        inputs: &serde_json::Value,
        preview: bool,
    ) -> anyhow::Result<ActionOutcome>;
}
