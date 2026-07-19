//! Static coverage for the benchmark's maintainer-authored reference workflows.

use std::path::PathBuf;

use cori_compiler::compile;

const TASKS: &[&str] = &[
    "support_inbox_triage",
    "sla_breach_pack",
    "lead_follow_up_queue",
    "customer_meeting_prep",
    "new_hire_onboarding_pack",
    "preapproved_pto_processing",
    "weekly_operating_review",
    "meeting_action_register",
    "expense_policy_audit",
    "budget_variance_deck",
];

fn workflow_dir(task: &str) -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop();
    path.pop();
    path.push("benchmarks/workflow-capture/reference-workflows");
    path.push(task);
    path
}

#[test]
fn compiles_every_workflow_capture_reference() {
    for task in TASKS {
        let workflow = compile(&workflow_dir(task))
            .unwrap_or_else(|errors| panic!("{task} should compile:\n{errors:#?}"));
        assert_eq!(workflow.manifest.id, *task);
        assert_eq!(workflow.manifest.tools_required, ["gws"]);
        assert!(
            workflow.steps.len() >= 3,
            "{task} should have a multi-step reference"
        );
        assert!(
            workflow
                .steps
                .iter()
                .any(|step| step.kind == cori_protocol::StepKind::Cli)
        );
        for step in workflow
            .steps
            .iter()
            .filter(|step| step.kind == cori_protocol::StepKind::Cli)
        {
            assert_eq!(
                step.metadata.get("binary").and_then(|value| value.as_str()),
                Some("gws")
            );
        }
    }
}
