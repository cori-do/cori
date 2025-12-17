use crate::adapter::DataAdapter;
use crate::audit::{AuditEvent, AuditSink};
use cori_core::{MutationIntent, StepKind};
use cori_policy::{PolicyCheckInput, PolicyClient};

pub struct Orchestrator<P: PolicyClient, A: DataAdapter, S: AuditSink> {
    policy: P,
    adapter: A,
    audit: S,
}

impl<P: PolicyClient, A: DataAdapter, S: AuditSink> Orchestrator<P, A, S> {
    pub fn new(policy: P, adapter: A, audit: S) -> Self {
        Self { policy, adapter, audit }
    }

    /// Execute (or preview) a MutationIntent end-to-end.
    pub async fn run(&self, intent: &MutationIntent) -> anyhow::Result<serde_json::Value> {
        let mut results = Vec::new();

        for step in &intent.plan.steps {
            // For MVP: policy-check only mutation steps.
            if matches!(step.kind, StepKind::Mutation) {
                // In a real system, resolve resource_id + attrs deterministically.
                // For now, we allow policy checks with minimal info.
                let resource = serde_json::json!({
                    "kind": "unknown",
                    "id": "unknown",
                    "attr": {}
                });

                let principal = serde_json::to_value(&intent.principal)?;
                let context = serde_json::json!({
                    "environment": intent.environment,
                    "tenant_id": intent.tenant_id,
                    "intent_id": intent.intent_id,
                    "preview": intent.preview
                });

                let decision = self.policy.check(PolicyCheckInput {
                    principal,
                    resource,
                    action: step.action.clone(),
                    context,
                }).await?;

                self.audit.record(AuditEvent {
                    intent_id: intent.intent_id.clone(),
                    step_id: step.id.clone(),
                    action: step.action.clone(),
                    allowed: decision.allow,
                    decision: serde_json::to_value(&decision)?,
                    outcome: serde_json::json!({ "stage": "policy_checked" }),
                });

                if !decision.allow {
                    return Err(anyhow::anyhow!("Denied by policy at step {}", step.id));
                }
            }

            let outcome = self
                .adapter
                .execute_action(&intent.tenant_id, &step.action, &step.inputs, intent.preview)
                .await?;

            self.audit.record(AuditEvent {
                intent_id: intent.intent_id.clone(),
                step_id: step.id.clone(),
                action: step.action.clone(),
                allowed: true,
                decision: serde_json::json!({ "note": "policy_checked_or_not_required" }),
                outcome: serde_json::json!({
                    "affected_count": outcome.affected_count,
                    "preview_diff": outcome.preview_diff,
                    "output": outcome.output
                }),
            });

            results.push(serde_json::json!({
                "step_id": step.id,
                "action": step.action,
                "affected_count": outcome.affected_count,
                "preview_diff": outcome.preview_diff,
                "output": outcome.output
            }));
        }

        Ok(serde_json::json!({ "intent_id": intent.intent_id, "results": results }))
    }
}
