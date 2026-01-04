use crate::adapter::DataAdapter;
use crate::audit::{AuditEvent, AuditEventType, AuditSink};
use cori_core::{ActionDefinition, MutationIntent, StepKind};
use cori_policy::{PolicyCheckInput, PolicyClient};
use std::collections::BTreeMap;
use std::sync::Arc;
use uuid::Uuid;

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
        let mut seq: u64 = 0;

        // Intent-level envelope event.
        self.audit.record(new_event(
            intent,
            &mut seq,
            AuditEventType::IntentReceived,
            "__intent__",
            "__intent__",
            None,
            None,
            true,
            serde_json::Map::new(),
            serde_json::Map::new(),
        ));

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
                let resource_kind = action_def.resource_kind.clone();
                let resource_attr = if resource_id != "unknown" && resource_id != "*" {
                    // Best-effort ABAC attrs. Adapter currently stubs this out.
                    self.adapter
                        .load_resource_attrs(
                            &intent.tenant_id,
                            &action_def.resource_kind,
                            &resource_id,
                        )
                        .await
                        .unwrap_or_else(|_| serde_json::json!({}))
                } else {
                    serde_json::json!({})
                };
                let resource = serde_json::json!({
                    "kind": action_def.resource_kind.clone(),
                    "id": resource_id,
                    "attr": resource_attr
                });

                let principal = serde_json::to_value(&intent.principal)?;
                let context = serde_json::json!({
                    "environment": intent.environment.clone(),
                    "tenant_id": intent.tenant_id.clone(),
                    "intent_id": intent.intent_id.clone(),
                    "preview": intent.preview
                });

                let decision = self
                    .policy
                    .check(PolicyCheckInput {
                        principal,
                        resource,
                        action: action_def.policy_action.clone(),
                        context: context.clone(),
                    })
                    .await?;

                let mut decision_obj = serde_json::Map::new();
                decision_obj.insert(
                    "policy_action".to_string(),
                    serde_json::Value::String(action_def.policy_action.clone()),
                );
                decision_obj.insert(
                    "input".to_string(),
                    serde_json::json!({
                        "principal": serde_json::to_value(&intent.principal).unwrap_or_else(|_| serde_json::json!({})),
                        "resource_kind": resource_kind,
                        "resource_id": infer_resource_id(action_def, &step.inputs),
                        "context": context
                    }),
                );
                decision_obj.insert(
                    "output".to_string(),
                    serde_json::to_value(&decision).unwrap_or_else(|_| serde_json::json!({})),
                );

                let mut outcome_obj = serde_json::Map::new();
                outcome_obj.insert(
                    "stage".to_string(),
                    serde_json::Value::String("policy_checked".to_string()),
                );

                self.audit.record(new_event(
                    intent,
                    &mut seq,
                    AuditEventType::PolicyChecked,
                    &step.id,
                    &step.action,
                    Some(action_def.resource_kind.clone()),
                    Some(infer_resource_id(action_def, &step.inputs)),
                    decision.allow,
                    decision_obj,
                    outcome_obj,
                ));

                if !decision.allow {
                    let mut outcome_obj = serde_json::Map::new();
                    outcome_obj.insert(
                        "error".to_string(),
                        serde_json::Value::String(format!("Denied by policy at step {}", step.id)),
                    );
                    self.audit.record(new_event(
                        intent,
                        &mut seq,
                        AuditEventType::Failed,
                        &step.id,
                        &step.action,
                        Some(action_def.resource_kind.clone()),
                        Some(infer_resource_id(action_def, &step.inputs)),
                        false,
                        serde_json::Map::new(),
                        outcome_obj,
                    ));
                    return Err(anyhow::anyhow!("Denied by policy at step {}", step.id));
                }
            }

            let outcome = match self
                .adapter
                .execute_action(&intent.tenant_id, action_def, &step.inputs, intent.preview)
                .await
            {
                Ok(o) => o,
                Err(err) => {
                    let mut outcome_obj = serde_json::Map::new();
                    outcome_obj.insert(
                        "error".to_string(),
                        serde_json::Value::String(err.to_string()),
                    );
                    self.audit.record(new_event(
                        intent,
                        &mut seq,
                        AuditEventType::Failed,
                        &step.id,
                        &step.action,
                        Some(action_def.resource_kind.clone()),
                        Some(infer_resource_id(action_def, &step.inputs)),
                        false,
                        serde_json::Map::new(),
                        outcome_obj,
                    ));
                    return Err(err);
                }
            };

            let mut decision_obj = serde_json::Map::new();
            decision_obj.insert(
                "note".to_string(),
                serde_json::Value::String("policy_checked_or_not_required".to_string()),
            );
            let mut outcome_obj = serde_json::Map::new();
            outcome_obj.insert(
                "affected_count".to_string(),
                serde_json::Value::Number(serde_json::Number::from(outcome.affected_count)),
            );
            outcome_obj.insert(
                "preview_diff".to_string(),
                outcome
                    .preview_diff
                    .clone()
                    .unwrap_or(serde_json::json!(null)),
            );
            outcome_obj.insert("output".to_string(), outcome.output.clone());

            let step_event_type = if intent.preview {
                AuditEventType::ActionPreviewed
            } else {
                AuditEventType::ActionExecuted
            };

            self.audit.record(new_event(
                intent,
                &mut seq,
                step_event_type,
                &step.id,
                &step.action,
                Some(action_def.resource_kind.clone()),
                Some(infer_resource_id(action_def, &step.inputs)),
                true,
                decision_obj,
                outcome_obj,
            ));

            results.push(serde_json::json!({
                "step_id": step.id.clone(),
                "action": step.action.clone(),
                "affected_count": outcome.affected_count,
                "preview_diff": outcome.preview_diff,
                "output": outcome.output
            }));
        }

        if !intent.preview {
            self.audit.record(new_event(
                intent,
                &mut seq,
                AuditEventType::Committed,
                "__intent__",
                "__intent__",
                None,
                None,
                true,
                serde_json::Map::new(),
                serde_json::Map::new(),
            ));
        }

        Ok(serde_json::json!({ "intent_id": intent.intent_id.clone(), "results": results }))
    }
}

fn new_event(
    intent: &MutationIntent,
    seq: &mut u64,
    event_type: AuditEventType,
    step_id: &str,
    action: &str,
    resource_kind: Option<String>,
    resource_id: Option<String>,
    allowed: bool,
    decision: serde_json::Map<String, serde_json::Value>,
    outcome: serde_json::Map<String, serde_json::Value>,
) -> AuditEvent {
    let event = AuditEvent {
        event_id: Uuid::new_v4(),
        occurred_at: chrono::Utc::now(),
        sequence: Some(*seq),
        tenant_id: intent.tenant_id.clone(),
        intent_id: intent.intent_id.clone(),
        principal_id: intent.principal.id.clone(),
        step_id: step_id.to_string(),
        event_type,
        action: action.to_string(),
        resource_kind,
        resource_id,
        allowed,
        preview: Some(intent.preview),
        decision,
        outcome,
        integrity: None,
        meta: Some(serde_json::json!({
            "environment": intent.environment.clone()
        })),
    };
    *seq += 1;
    event
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
        if def.policy_action == "list" {
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
