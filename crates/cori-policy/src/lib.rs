use async_trait::async_trait;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyDecision {
    pub allow: bool,
    pub obligations: serde_json::Value,
    pub rule_id: Option<String>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyCheckInput {
    pub principal: serde_json::Value,
    pub resource: serde_json::Value,
    pub action: String,
    pub context: serde_json::Value,
}

#[async_trait]
pub trait PolicyClient: Send + Sync {
    async fn check(&self, input: PolicyCheckInput) -> anyhow::Result<PolicyDecision>;
}

/// Stub client for now; replace with real Cerbos gRPC client.
pub struct AllowAllPolicyClient;

#[async_trait]
impl PolicyClient for AllowAllPolicyClient {
    async fn check(&self, _input: PolicyCheckInput) -> anyhow::Result<PolicyDecision> {
        Ok(PolicyDecision {
            allow: true,
            obligations: serde_json::json!({}),
            rule_id: None,
            reason: Some("allow_all_stub".to_string()),
        })
    }
}
