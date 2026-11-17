// SPDX-License-Identifier: Apache-2.0
//! Changes parked for a human decision.
//!
//! A create, update or delete that touches a `requires_approval` column is not executed: the executor returns it as a
//! [`ParkedChange`] (`ExecutionResult::parked`). The caller keeps it until a person decides (Cori's server keeps it in
//! its approvals table, so every replica sees the same pending change and one decision applies it once), and applies an
//! approved one with `ToolExecutor::execute_approved_on`, which checks first that the row it was parked on still reads
//! the same.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// A change waiting for a decision: what the executor needs to apply it later.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParkedChange {
    /// Identifies the change (a v4 UUID).
    pub id: String,
    /// The tool that was called (`updateTicket`).
    pub tool_name: String,
    /// Its arguments, as validated when the change was parked.
    pub arguments: Value,
    /// The columns that required the approval.
    pub approval_fields: Vec<String>,
    /// The caller: its tenant id and its Cori role.
    pub tenant_id: String,
    pub role: String,
    /// An update or delete of an existing row: the table, the key and the row as the role read it when the change was
    /// parked. The change applies only while the row still reads the same.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_table: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_pk: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_values: Option<Value>,
}

impl ParkedChange {
    /// A new change with a fresh id.
    pub fn new(tool_name: &str, arguments: Value, approval_fields: Vec<String>, tenant_id: &str, role: &str) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            tool_name: tool_name.to_string(),
            arguments,
            approval_fields,
            tenant_id: tenant_id.to_string(),
            role: role.to_string(),
            target_table: None,
            target_pk: None,
            original_values: None,
        }
    }

    /// The tool's answer to the caller: the change is pending.
    pub fn pending_response(&self) -> Value {
        json!({
            "status": "pending_approval",
            "approvalId": self.id,
            "message": format!("Action '{}' requires human approval. Approval ID: {}", self.tool_name, self.id),
            "approvalFields": self.approval_fields,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_parked_change_round_trips_and_answers_pending() {
        let mut c = ParkedChange::new("updateTicket", json!({ "id": "t1", "budget": 9 }), vec!["budget".into()], "alice", "member");
        c.target_table = Some("tickets".into());
        c.target_pk = Some(json!("t1"));
        c.original_values = Some(json!({ "id": "t1", "budget": 1 }));
        let back: ParkedChange = serde_json::from_value(serde_json::to_value(&c).unwrap()).unwrap();
        assert_eq!(back, c);
        let r = c.pending_response();
        assert_eq!((r["status"].as_str(), r["approvalId"].as_str()), (Some("pending_approval"), Some(c.id.as_str())));
        assert_eq!(r["approvalFields"], json!(["budget"]));
        // a create has no row to compare with: the snapshot fields are left out
        let create = serde_json::to_value(ParkedChange::new("createTicket", json!({}), vec![], "alice", "member")).unwrap();
        assert!(create.get("target_table").is_none() && create.get("original_values").is_none(), "{create}");
    }
}
