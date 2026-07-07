//! "Install the `cori` command in PATH" — VS Code's `code` pattern.
//!
//! The app bundles the CLI as the `cori-cli` sidecar (the bare name
//! would collide with the `Cori` app binary on case-insensitive
//! filesystems). Installing exposes it as `cori`:
//!
//! - **Unix**: symlink `cori` → the bundled sidecar, in the first
//!   writable of `/usr/local/bin`, `~/.local/bin`, `~/bin` — the same
//!   candidate walk as `scripts/install.sh`. The symlink tracks app
//!   updates for free (the .app path is stable across releases).
//! - **Windows**: copy the sidecar to `%LOCALAPPDATA%\Cori\bin\cori.exe`
//!   and append that directory to the user `Path` (via .NET's
//!   `SetEnvironmentVariable`, which broadcasts the change). A copy —
//!   not a link — because symlinks need Developer Mode; the app
//!   refreshes it on every install click, and it goes stale (not
//!   broken) between app updates until re-installed.
//!
//! A `cori` that already resolves on PATH but wasn't placed by us
//! (e.g. `install.sh`) is left alone — status reports it and install
//! refuses to clobber it.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::error::{IpcError, IpcResult};
use crate::sidecars;

#[derive(Debug, Clone, Serialize)]
pub struct CliInstallStatus {
    /// The CLI sidecar is present in this app bundle (false in dev
    /// builds that haven't staged it).
    pub bundled: bool,
    /// Where `cori` currently resolves, if anywhere (PATH lookup —
    /// the launcher repairs PATH from the login shell at startup).
    pub installed_path: Option<String>,
    /// The installed `cori` is the one this app manages.
    pub managed: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct InstallCliResult {
    /// The `cori` entry point that now exists.
    pub path: String,
    /// False when a managed install was already in place.
    pub created: bool,
    /// The install directory is on the current (login-shell) PATH.
    /// When false the UI should tell the user to add it.
    pub on_path: bool,
}

#[tauri::command(rename_all = "snake_case")]
pub async fn cli_install_status() -> IpcResult<CliInstallStatus> {
    tokio::task::spawn_blocking(status_blocking)
        .await
        .map_err(|e| IpcError::Internal(anyhow::anyhow!("cli status task join: {e}")))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn install_cli() -> IpcResult<InstallCliResult> {
    tokio::task::spawn_blocking(install_blocking)
        .await
        .map_err(|e| IpcError::Internal(anyhow::anyhow!("cli install task join: {e}")))?
}

fn status_blocking() -> CliInstallStatus {
    let bundled = sidecars::bundled_cori_cli();
    let installed = find_on_path();
    let managed = match (&installed, &bundled) {
        (Some(found), Some(sidecar)) => is_managed(found, sidecar),
        _ => false,
    };
    CliInstallStatus {
        bundled: bundled.is_some(),
        installed_path: installed.map(|p| p.display().to_string()),
        managed,
    }
}

fn install_blocking() -> IpcResult<InstallCliResult> {
    let sidecar = sidecars::bundled_cori_cli().ok_or_else(|| {
        IpcError::BadRequest(
            "this build doesn't bundle the cori CLI (dev build?) — \
             use `cargo build -p cori-cli` or the install script instead"
                .into(),
        )
    })?;

    if let Some(found) = find_on_path() {
        if is_managed(&found, &sidecar) {
            // Unix: symlink already points at the sidecar — done.
            // Windows: refresh the copy so it matches this app version.
            #[cfg(windows)]
            std::fs::copy(&sidecar, &found)
                .map_err(|e| IpcError::Internal(anyhow::anyhow!("refreshing {found:?}: {e}")))?;
            return Ok(InstallCliResult {
                path: found.display().to_string(),
                created: false,
                on_path: true,
            });
        }
        // Somebody else's `cori` (install.sh, homebrew, …) — hands off.
        return Err(IpcError::BadRequest(format!(
            "`cori` is already installed at {} (not by this app) — \
             remove it first if you want the launcher to manage the CLI",
            found.display()
        )));
    }

    install_fresh(&sidecar)
}

/// The found `cori` is one we placed: a symlink resolving to a
/// `cori-cli` sidecar (unix) or a copy under our managed bin dir
/// (windows).
fn is_managed(found: &Path, sidecar: &Path) -> bool {
    #[cfg(unix)]
    {
        match std::fs::read_link(found) {
            Ok(target) => {
                target == *sidecar
                    || target.file_name().and_then(|n| n.to_str()) == Some("cori-cli")
            }
            Err(_) => false,
        }
    }
    #[cfg(windows)]
    {
        let _ = sidecar;
        managed_bin_dir().is_some_and(|dir| found.starts_with(dir))
    }
}

/// Resolve `cori` the way a terminal would: scan the process PATH
/// (repaired from the login shell at startup — see `lib.rs`).
fn find_on_path() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let file = if cfg!(windows) { "cori.exe" } else { "cori" };
    for dir in std::env::split_paths(&path) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        let candidate = dir.join(file);
        if is_executable_file(&candidate) {
            return Some(candidate);
        }
    }
    None
}

fn is_executable_file(p: &Path) -> bool {
    // `is_file` follows symlinks, so a dangling link reports false —
    // exactly what we want: a stale managed link is "not installed"
    // and install() recreates it.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        p.is_file()
            && std::fs::metadata(p)
                .map(|m| m.permissions().mode() & 0o111 != 0)
                .unwrap_or(false)
    }
    #[cfg(windows)]
    {
        p.is_file()
    }
}

fn dir_on_path(dir: &Path) -> bool {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d == dir))
        .unwrap_or(false)
}

// ---------- Unix ---------------------------------------------------------

#[cfg(unix)]
fn install_fresh(sidecar: &Path) -> IpcResult<InstallCliResult> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let mut candidates = vec![PathBuf::from("/usr/local/bin")];
    if let Some(h) = &home {
        candidates.push(h.join(".local/bin"));
        candidates.push(h.join("bin"));
    }

    let dir = candidates
        .into_iter()
        .find(|d| ensure_writable_dir(d))
        .ok_or_else(|| {
            IpcError::BadRequest(
                "no writable install directory found (tried /usr/local/bin, \
                 ~/.local/bin, ~/bin)"
                    .into(),
            )
        })?;

    let link = dir.join("cori");
    // A dangling or stale managed symlink survives find_on_path()'s
    // is_file() check — clear it before linking.
    if std::fs::symlink_metadata(&link).is_ok() {
        std::fs::remove_file(&link)
            .map_err(|e| IpcError::Internal(anyhow::anyhow!("removing stale {link:?}: {e}")))?;
    }
    std::os::unix::fs::symlink(sidecar, &link)
        .map_err(|e| IpcError::Internal(anyhow::anyhow!("symlinking {link:?}: {e}")))?;

    Ok(InstallCliResult {
        path: link.display().to_string(),
        created: true,
        on_path: dir_on_path(&dir),
    })
}

/// The directory exists (or was created, for $HOME candidates only)
/// and is writable.
#[cfg(unix)]
fn ensure_writable_dir(dir: &Path) -> bool {
    if !dir.is_dir() {
        // Only create directories under $HOME — never system paths.
        let under_home = std::env::var_os("HOME")
            .map(|h| dir.starts_with(PathBuf::from(h)))
            .unwrap_or(false);
        if !under_home || std::fs::create_dir_all(dir).is_err() {
            return false;
        }
    }
    // Probe with a real write — faccessat semantics differ across
    // platforms and euid/egid setups.
    let probe = dir.join(".cori-install-probe");
    match std::fs::File::create(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

// ---------- Windows ------------------------------------------------------

#[cfg(windows)]
fn managed_bin_dir() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("Cori").join("bin"))
}

#[cfg(windows)]
fn install_fresh(sidecar: &Path) -> IpcResult<InstallCliResult> {
    let dir = managed_bin_dir()
        .ok_or_else(|| IpcError::BadRequest("%LOCALAPPDATA% is not set".into()))?;
    std::fs::create_dir_all(&dir)
        .map_err(|e| IpcError::Internal(anyhow::anyhow!("creating {dir:?}: {e}")))?;

    let exe = dir.join("cori.exe");
    std::fs::copy(sidecar, &exe)
        .map_err(|e| IpcError::Internal(anyhow::anyhow!("copying CLI to {exe:?}: {e}")))?;

    let already_on_path = dir_on_path(&dir);
    if !already_on_path {
        add_to_user_path(&dir)?;
        // Also patch this process so find_on_path() sees it immediately.
        if let Some(current) = std::env::var_os("PATH") {
            let mut paths: Vec<_> = std::env::split_paths(&current).collect();
            paths.push(dir.clone());
            if let Ok(joined) = std::env::join_paths(paths) {
                // SAFETY: single mutation, matching the pattern used at
                // startup (lib.rs / worker.rs) — no reader races in
                // practice on this IPC-driven path.
                unsafe { std::env::set_var("PATH", joined) };
            }
        }
    }

    Ok(InstallCliResult {
        path: exe.display().to_string(),
        created: true,
        // Persisted to the user Path either way; new terminals see it.
        on_path: true,
    })
}

/// Append `dir` to the persistent user `Path` via .NET — unlike `setx`
/// this does not truncate at 1024 chars, and it broadcasts
/// `WM_SETTINGCHANGE` so new shells pick it up without relogin.
#[cfg(windows)]
fn add_to_user_path(dir: &Path) -> IpcResult<()> {
    let script = format!(
        "$d = '{}'; \
         $cur = [Environment]::GetEnvironmentVariable('Path', 'User'); \
         if (($cur -split ';') -notcontains $d) {{ \
             [Environment]::SetEnvironmentVariable('Path', ($cur.TrimEnd(';') + ';' + $d), 'User') \
         }}",
        dir.display()
    );
    let out = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output()
        .map_err(|e| IpcError::Internal(anyhow::anyhow!("spawning powershell: {e}")))?;
    if !out.status.success() {
        return Err(IpcError::Internal(anyhow::anyhow!(
            "updating user Path failed: {}",
            String::from_utf8_lossy(&out.stderr)
        )));
    }
    Ok(())
}
