//! `cori demo` — Phase 8.
//!
//! Materialises the embedded `hello_world` runbook under
//! `~/.cori/runbooks/hello_world/`, (re-)registers it, and runs it. The
//! whole flow is meant to work on a fresh install with zero credentials —
//! the only host requirement is `curl` on `PATH`, which ships by default
//! on macOS and every mainstream Linux distro.
//!
//! Why extract instead of running from a temp directory? Two reasons:
//! 1. `cori workflows show hello_world` / `cori runs show ...` after the
//!    demo should print something useful; that means the runbook source
//!    has to live where the registry can keep pointing at it.
//! 2. The roadmap explicitly says runbooks live at
//!    `~/.cori/runbooks/<id>/` — the demo is a runbook like any other.

use anyhow::{Context, Result};
use serde_json::json;

use crate::{commands, embedded, paths, runtime};

pub fn run() -> Result<()> {
    // Make sure `~/.cori/` exists. Re-running `init` is idempotent.
    crate::commands::init::run(true).context("initialising ~/.cori before running demo")?;

    let runbooks = paths::runbooks_dir()?;
    let dest = runbooks.join("hello_world");
    let count = embedded::extract(embedded::hello_world::HELLO_WORLD_FILES, &dest)
        .context("extracting embedded hello_world runbook")?;
    println!(
        "✓ Extracted hello_world runbook ({count} files) to {}",
        dest.display()
    );

    // Make sure the Deno runtime is installed for the `code` steps.
    let _ = runtime::install().context("installing Deno runtime")?;

    // Register (or refresh) the workflow against the local SQLite registry.
    commands::workflows::register(&dest).context("registering hello_world")?;

    println!();

    // Run. Empty params — hello_world declares none.
    commands::run::execute_workflow("hello_world", json!({}), false, true, None)
        .context("running hello_world")?;

    println!();
    println!("Next: `cori skill install --agent claude-code` to wire up your agent.");
    Ok(())
}
