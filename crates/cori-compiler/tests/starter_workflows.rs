//! Static coverage for the ready-made workflows exposed by Cori Console.

use std::path::PathBuf;

use cori_compiler::compile;
use cori_protocol::StepKind;

const STARTERS: &[(&str, &str, usize)] = &[
    ("disk_space_snapshot", "df", 1),
    ("largest_files_report", "python3", 1),
    ("gws_meeting_prep", "gws", 1),
    ("gws_weekly_digest", "gws", 1),
    ("gws_sheet_range_snapshot", "gws", 2),
];

fn workflow_dir(name: &str) -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop();
    path.pop();
    path.push("examples/starter_workflows");
    path.push(name);
    path
}

#[test]
fn compiles_every_console_starter_workflow() {
    for (name, binary, step_count) in STARTERS {
        let workflow = compile(&workflow_dir(name))
            .unwrap_or_else(|errors| panic!("{name} should compile:\n{errors:#?}"));

        assert_eq!(workflow.manifest.id, *name);
        assert_eq!(workflow.manifest.tools_required, [*binary]);
        assert_eq!(workflow.steps.len(), *step_count);
        assert_eq!(workflow.steps[0].kind, StepKind::Cli);
        assert_eq!(
            workflow.steps[0]
                .metadata
                .get("binary")
                .and_then(|value| value.as_str()),
            Some(*binary),
        );
    }
}
