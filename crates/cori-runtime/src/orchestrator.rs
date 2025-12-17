use crate::adapter::DataAdapter;
use crate::audit::{AuditEvent, AuditSink};
use cori_core::{ActionDefinition, MutationIntent, StepKind};
use cori_policy::{PolicyCheckInput, PolicyClient};
use std::collections::BTreeMap;
use std::sync::Arc;

pub struct Orchestrator<A: DataAdapter, S: AuditSink> {
    policy: Arc<dyn PolicyClient>,
    adapter: A,
    audit: S,
    actions: BTreeMap<String, ActionDefinition>,
}

impl<A: DataAdapter, S: AuditSink> Orchestrator<A, S> {
    pub fn new(
        policy: Arc<dyn PolicyClient>,
        adapter: A,
        audit: S,
        actions: BTreeMap<String, ActionDefinition>,
    ) -> Self {
        Self {
            policy,
            adapter,
            audit,
            actions,
        }
    }

    /// Execute (or preview) a MutationIntent end-to-end.
    pub async fn run(&self, intent: &MutationIntent) -> anyhow::Result<serde_json::Value> {
        let mut results = Vec::new();

        for step in &intent.plan.steps {
            let action_def = self.actions.get(&step.action).ok_or_else(|| {
                anyhow::anyhow!(
                    "Action '{}' not found in action catalog (required for policy checks).",
                    step.action
                )
            })?;

            // For MVP: policy-check only mutation steps.
            if matches!(step.kind, StepKind::Mutation) {
                let resource_id = infer_resource_id(action_def, &step.inputs);
                let resource_attr = if resource_id != "unknown" && resource_id != "*" {
                    // Best-effort ABAC attrs. Adapter currently stubs this out.
                    self.adapter
                        .load_resource_attrs(&intent.tenant_id, &action_def.resource_kind, &resource_id)
                        .await
                        .unwrap_or_else(|_| serde_json::json!({}))
                } else {
                    serde_json::json!({})
                };
                let resource = serde_json::json!({
                    "kind": action_def.resource_kind,
                    "id": resource_id,
                    "attr": resource_attr
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
                    action: action_def.cerbos_action.clone(),
                    context: context.clone(),
                }).await?;

                self.audit.record(AuditEvent {
                    intent_id: intent.intent_id.clone(),
                    step_id: step.id.clone(),
                    action: step.action.clone(),
                    allowed: decision.allow,
                    decision: serde_json::json!({
                        "policy_action": action_def.cerbos_action,
                        "input": {
                            "principal": serde_json::to_value(&intent.principal).unwrap_or_else(|_| serde_json::json!({})),
                            "resource_kind": action_def.resource_kind,
                            "resource_id": infer_resource_id(action_def, &step.inputs),
                            "context": context
                        },
                        "output": serde_json::to_value(&decision).unwrap_or_else(|_| serde_json::json!({}))
                    }),
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

fn infer_resource_id(def: &ActionDefinition, inputs: &serde_json::Value) -> String {
    // Use input_schema.required minus a small ignore-list to form a deterministic resource id.
    // This works well for generated PK-based actions.
    let ignore: std::collections::BTreeSet<&'static str> = [
        "tenant_id",
        "limit",
        "cursor",
        "patch",
        "expected_version",
        "reason",
        "deleted_by",
    ]
    .into_iter()
    .collect();

    let required = def
        .input_schema
        .get("required")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let mut keys: Vec<String> = required
        .into_iter()
        .filter_map(|v| v.as_str().map(|s| s.to_string()))
        .filter(|k| !ignore.contains(k.as_str()))
        .collect();

    if keys.is_empty() {
        // For list-style actions, treat the collection as the resource.
        if def.cerbos_action == "list" {
            return "*".to_string();
        }
        return "unknown".to_string();
    }

    keys.sort();
    if keys.len() == 1 {
        let k = &keys[0];
        return scalarish_to_string(inputs.get(k)).unwrap_or_else(|| "unknown".to_string());
    }

    // Composite key: stable, human-readable string.
    let mut parts = Vec::new();
    for k in keys {
        let v = scalarish_to_string(inputs.get(&k)).unwrap_or_else(|| "unknown".to_string());
        parts.push(format!("{}={}", k, v));
    }
    parts.join(";")
}

fn scalarish_to_string(v: Option<&serde_json::Value>) -> Option<String> {
    let v = v?;
    match v {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        serde_json::Value::Bool(b) => Some(b.to_string()),
        serde_json::Value::Null => None,
        other => Some(other.to_string()),
    }
}
