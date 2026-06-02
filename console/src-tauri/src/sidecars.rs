//! Tauri sidecar path discovery for binaries that the broker invokes
//! itself (not through `app.shell().sidecar(...)`). The Temporal CLI is
//! still spawned via the shell plugin (see `supervisor.rs`); this
//! module exists for **deno**, which the in-process worker hands off
//! to via `std::process::Command` deep inside the broker — that crate
//! is Tauri-agnostic and can't reach the shell plugin.
//!
//! Path resolution mirrors what `tauri_plugin_shell` does internally:
//! `<exe_dir>/<name>[.exe]` for production bundles (Tauri strips the
//! target-triple suffix when staging the final app), with a fallback
//! to `<exe_dir>/<name>-<TARGET_TRIPLE>[.exe]` for `cargo tauri dev`
//! which keeps the suffix.

use std::path::PathBuf;

/// Cargo target triple captured at build time by `build.rs`.
const TARGET_TRIPLE: Option<&str> = option_env!("TARGET_TRIPLE");

/// Locate the bundled Deno binary, if any. Returns `None` when no
/// sidecar was staged (typically: running outside the Tauri-built
/// shell, or a dev build that hasn't run `fetch-deno-binaries.sh`).
pub fn bundled_deno() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let suffix = if cfg!(windows) { ".exe" } else { "" };

    // Production layout (`tauri build` output).
    let bare = dir.join(format!("deno{suffix}"));
    if bare.is_file() {
        return Some(bare);
    }

    // Dev layout (`tauri dev` — sidecar keeps its triple suffix).
    if let Some(triple) = TARGET_TRIPLE {
        let with_triple = dir.join(format!("deno-{triple}{suffix}"));
        if with_triple.is_file() {
            return Some(with_triple);
        }
    }
    None
}
