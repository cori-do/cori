//! Build-time guard for the embedded Console SPA.
//!
//! If `packages/console/build/client/index.html` is missing we emit
//! a `cargo:warning` and write a tiny placeholder so the Rust build
//! still produces a working binary. The proper CI ordering is
//! `pnpm --filter @cori-do/console build` → `cargo build -p cori-console`;
//! the placeholder exists only to keep `cargo build --workspace`
//! green during pure-backend development.

use std::path::PathBuf;

fn main() {
    let manifest = std::env::var("CARGO_MANIFEST_DIR")
        .expect("CARGO_MANIFEST_DIR set by cargo");
    let asset_root = PathBuf::from(&manifest)
        .join("../../packages/console/build/client");
    let index = asset_root.join("index.html");

    // Re-run whenever the assets land or vanish.
    println!("cargo:rerun-if-changed={}", index.display());
    println!(
        "cargo:rerun-if-changed={}",
        asset_root.join("assets").display()
    );

    if !index.is_file() {
        println!(
            "cargo:warning=Cori Console SPA assets missing at `{}` — \
             run `pnpm --filter @cori-do/console build` first. \
             Writing a placeholder so the Rust build still succeeds.",
            index.display()
        );
        let _ = std::fs::create_dir_all(&asset_root);
        let _ = std::fs::write(
            &index,
            PLACEHOLDER_HTML,
        );
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
