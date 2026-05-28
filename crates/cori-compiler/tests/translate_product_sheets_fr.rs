//! Golden integration test against the canonical example workflow.
//!
//! Acceptance criteria for the current compiler fixture: the compiler
//! invoked against `examples/translate_product_sheets_fr/` produces a
//! `CompiledWorkflow` with 5 steps in the right order, correct kinds, and a
//! single routing decision per step.

use std::path::PathBuf;

use cori_compiler::compile;
use cori_protocol::StepKind;

fn example_dir() -> PathBuf {
    // CARGO_MANIFEST_DIR points at crates/cori-compiler.
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop(); // crates/
    p.pop(); // workspace root
    p.push("examples");
    p.push("translate_product_sheets_fr");
    p
}

#[test]
fn compiles_translate_product_sheets_fr() {
    let workflow = match compile(&example_dir()) {
        Ok(w) => w,
        Err(es) => panic!("expected compile success, got errors:\n{es:#?}"),
    };

    assert_eq!(workflow.manifest.id, "translate_product_sheets_fr");
    assert_eq!(workflow.manifest.version, 1);
    assert_eq!(workflow.manifest.parameters.len(), 4);
    assert_eq!(workflow.manifest.tools_required, vec!["gws".to_string()]);

    assert_eq!(workflow.steps.len(), 5);

    let expected: &[(&str, StepKind)] = &[
        ("01_read_source_rows", StepKind::Cli),
        ("02_translate_rows", StepKind::Llm),
        ("03_check_gpsr", StepKind::Code),
        ("04_ensure_fr_tab", StepKind::Cli),
        ("05_write_results", StepKind::Cli),
    ];
    for (i, (id, kind)) in expected.iter().enumerate() {
        let s = &workflow.steps[i];
        assert_eq!(s.activity_id, *id, "step {i} id");
        assert_eq!(s.kind, *kind, "step {i} kind");
        assert_eq!(s.index, i as u32);
        if i == 0 {
            assert!(s.depends_on.is_empty());
        } else {
            assert_eq!(
                s.depends_on,
                vec![workflow.steps[i - 1].activity_id.clone()]
            );
        }
        assert!(!s.description.is_empty(), "step {i} description");
    }

    // CLI steps must record their binary as `gws`.
    for s in workflow.steps.iter().filter(|s| s.kind == StepKind::Cli) {
        let bin = s
            .metadata
            .get("binary")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        assert_eq!(bin, "gws", "{} binary", s.activity_id);
    }
    assert_eq!(workflow.required_cli_binaries, vec!["gws".to_string()]);

    // LLM step records its model.
    let llm = &workflow.steps[1];
    assert_eq!(
        llm.metadata.get("model").and_then(|v| v.as_str()),
        Some("gpt-4o-mini")
    );
}

#[test]
fn compiled_workflow_serializes_stably() {
    let mut workflow = compile(&example_dir()).expect("compile");
    // The manifest `body` (human-readable markdown) is intentionally not
    // serialized — drop it from the source side before round-tripping.
    workflow.manifest.body = String::new();
    let json = serde_json::to_string_pretty(&workflow).expect("serialize");
    let back: cori_protocol::CompiledWorkflow = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back, workflow);
}
