//! Golden integration test for the read-only SAP reference workflow.
//!
//! The SAP boundary depends on compiler-frozen CLI metadata. Keep this test
//! close to the compiler so changes to parsing cannot silently relax the
//! binary, timeout, retry, or placement contract.

use std::path::PathBuf;

use cori_compiler::compile;
use cori_protocol::{Placement, StepKind};

fn example_dir() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop(); // crates/
    path.pop(); // workspace root
    path.push("examples");
    path.push("sap_purchase_order_read");
    path
}

#[test]
fn compiles_the_typed_read_only_sap_contract() {
    let workflow = compile(&example_dir()).unwrap_or_else(|errors| {
        panic!("expected SAP example to compile, got errors:\n{errors:#?}")
    });

    assert_eq!(workflow.manifest.id, "sap_purchase_order_read");
    assert_eq!(workflow.manifest.version, 1);
    assert_eq!(workflow.manifest.parameters.len(), 3);
    assert_eq!(workflow.manifest.tools_required, ["cori-sap"]);
    assert!(workflow.manifest.mcp_servers.is_empty());
    assert!(workflow.manifest.result.is_some());

    assert_eq!(workflow.steps.len(), 1);
    let step = &workflow.steps[0];
    assert_eq!(step.activity_id, "01_read_purchase_orders");
    assert_eq!(step.kind, StepKind::Cli);
    assert_eq!(step.placement, Placement::RequiresLocalFs);
    assert!(step.depends_on.is_empty());
    assert_eq!(
        step.metadata.get("binary").and_then(|value| value.as_str()),
        Some("cori-sap")
    );
    assert_eq!(
        step.metadata
            .get("timeout_ms")
            .and_then(|value| value.as_u64()),
        Some(60_000)
    );
    assert_eq!(
        step.metadata
            .get("retries")
            .and_then(|value| value.get("max"))
            .and_then(|value| value.as_u64()),
        Some(1)
    );
    assert_eq!(workflow.required_cli_binaries, ["cori-sap"]);
}
