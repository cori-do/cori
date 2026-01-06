//! Cori Policy Enforcement
//!
//! Biscuit-native policy model: policy decisions are made based on Biscuit token claims
//! and role YAML configuration. No external policy decision point (PDP) required.
//!
//! See AGENTS.md Section 8: "Policy Model: Biscuit-Native (No External Engine)"

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Result of a policy check.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyDecision {
    pub allow: bool,
    pub obligations: serde_json::Value,
    pub rule_id: Option<String>,
    pub reason: Option<String>,
}

/// Input for a policy check.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyCheckInput {
    pub principal: serde_json::Value,
    pub resource: serde_json::Value,
    pub action: String,
    pub context: serde_json::Value,
}

/// Policy client trait for checking authorization.
///
/// In the Biscuit-native model, policy decisions are made by:
/// 1. Verifying the Biscuit token (handled by cori-biscuit)
/// 2. Checking role permissions from the token claims
/// 3. Enforcing constraints from role configuration
///
/// This trait abstracts the policy decision interface for backwards compatibility.
#[async_trait]
pub trait PolicyClient: Send + Sync {
    async fn check(&self, input: PolicyCheckInput) -> anyhow::Result<PolicyDecision>;
}

/// Biscuit-native policy client.
///
/// This client implements the Biscuit-native policy model where:
/// - Token validity and expiration are checked by the Biscuit verifier
/// - Role permissions (tables, columns, operations) come from token claims
/// - Runtime guards (RLS injection, row limits) are enforced at query time
///
/// For now, this acts as an allow-all stub since actual enforcement happens
/// at the MCP layer based on token claims.
pub struct BiscuitPolicyClient;

impl BiscuitPolicyClient {
    pub fn new() -> Self {
        Self
    }
}

impl Default for BiscuitPolicyClient {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl PolicyClient for BiscuitPolicyClient {
    async fn check(&self, input: PolicyCheckInput) -> anyhow::Result<PolicyDecision> {
        // In the Biscuit-native model, authorization is enforced at multiple layers:
        // 1. Biscuit token verification (signature, expiration, tenant claim)
        // 2. Role configuration (table/column access, allowed_values)
        // 3. Runtime guards (tenant filtering, column filtering, row limits)
        //
        // This policy client exists for audit logging and compatibility.
        // Actual enforcement happens in cori-mcp tool execution.
        
        tracing::debug!(
            action = %input.action,
            "Biscuit-native policy check (enforcement at MCP layer)"
        );

        Ok(PolicyDecision {
            allow: true,
            obligations: serde_json::json!({}),
            rule_id: None,
            reason: Some("biscuit_native".to_string()),
        })
    }
}

/// Legacy allow-all policy client for backwards compatibility.
/// 
/// Use `BiscuitPolicyClient` for production deployments.
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
