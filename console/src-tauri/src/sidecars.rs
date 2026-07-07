//! Tauri sidecar path discovery for binaries the app resolves by path
//! itself (not through `app.shell().sidecar(...)`). The Temporal CLI is
//! still spawned via the shell plugin (see `supervisor.rs`); this
//! module exists for **deno**, which the in-process worker hands off
//! to via `std::process::Command` deep inside the broker — that crate
//! is Tauri-agnostic and can't reach the shell plugin — and for the
//! **cori CLI**, which `cli_install.rs` links into the user's PATH.
//!
//! Path resolution mirrors what `tauri_plugin_shell` does internally:
//! `<exe_dir>/<name>[.exe]` for production bundles (Tauri strips the
//! target-triple suffix when staging the final app), with a fallback
//! to `<exe_dir>/<name>-<TARGET_TRIPLE>[.exe]` for `cargo tauri dev`
//! which keeps the suffix.

use std::path::PathBuf;

/// Cargo target triple captured at build time by `build.rs`.
const TARGET_TRIPLE: Option<&str> = option_env!("TARGET_TRIPLE");

/// Locate a bundled sidecar by bare name, if any. Returns `None` when
/// the sidecar wasn't staged (typically: running outside the
/// Tauri-built shell, or a dev build that hasn't staged it).
fn bundled(name: &str) -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let suffix = if cfg!(windows) { ".exe" } else { "" };

    // Production layout (`tauri build` output).
    let bare = dir.join(format!("{name}{suffix}"));
    if bare.is_file() {
        return Some(bare);
    }

    // Dev layout (`tauri dev` — sidecar keeps its triple suffix).
    if let Some(triple) = TARGET_TRIPLE {
        let with_triple = dir.join(format!("{name}-{triple}{suffix}"));
        if with_triple.is_file() {
            return Some(with_triple);
        }
    }
    None
}

/// Locate the bundled Deno binary (dev: `fetch-deno-binaries.sh`).
pub fn bundled_deno() -> Option<PathBuf> {
    bundled("deno")
}

/// Locate the bundled `cori` CLI (dev: `build-cli-binary.sh`). Staged
/// as `cori-cli` because the bare name would collide with the `Cori`
/// app binary on case-insensitive filesystems (macOS, Windows); the
/// PATH install in `cli_install.rs` exposes it as `cori`.
pub fn bundled_cori_cli() -> Option<PathBuf> {
    bundled("cori-cli")
}
