//! Build-time guard for the embedded Console SPA.
//!
//! Two jobs:
//!   1. If `packages/console/build/client/index.html` is missing, emit
//!      a `cargo:warning` and write a placeholder so the Rust build
//!      still produces a working binary (with a "console not built"
//!      splash). The proper CI ordering is `pnpm --filter
//!      @cori-do/console build` → `cargo build -p cori-console`.
//!   2. Walk every file under `build/client/` and emit
//!      `cargo:rerun-if-changed=<path>` so any SPA rebuild (new PNGs,
//!      changed asset bundle, anything) invalidates this crate and
//!      `rust-embed` re-snapshots the folder.

use std::path::{Path, PathBuf};

fn main() {
    let manifest = std::env::var("CARGO_MANIFEST_DIR")
        .expect("CARGO_MANIFEST_DIR set by cargo");
    let asset_root = PathBuf::from(&manifest).join("../../packages/console/build/client");
    let index = asset_root.join("index.html");

    // Always re-evaluate on directory mtime changes (added/removed files).
    println!("cargo:rerun-if-changed={}", asset_root.display());

    if !index.is_file() {
        println!(
            "cargo:warning=Cori Console SPA assets missing at `{}` — \
             run `pnpm --filter @cori-do/console build` first. \
             Writing a placeholder so the Rust build still succeeds.",
            index.display()
        );
        let _ = std::fs::create_dir_all(&asset_root);
        let _ = std::fs::write(&index, PLACEHOLDER_HTML);
    }

    // Walk every file so content edits also trigger a rebuild.
    walk(&asset_root);
}

fn walk(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        println!("cargo:rerun-if-changed={}", path.display());
        if path.is_dir() {
            walk(&path);
        }
    }
}

const PLACEHOLDER_HTML: &str = r#"<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8">
    <title>Cori Console (not built)</title>
  </head>
  <body style="font-family: system-ui; padding: 32px; max-width: 600px; margin: 0 auto;">
    <h1>Cori Console isn't built.</h1>
    <p>The Rust binary was built without the SPA assets. Run:</p>
    <pre>pnpm --filter @cori-do/console build
cargo build -p cori-console</pre>
    <p>The API endpoints under <code>/api/*</code> still work.</p>
  </body>
</html>
"#;
