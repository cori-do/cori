//! Resolve a Temporal endpoint for `cori run` / `cori work`.
//!
//! Rules, in priority order:
//!
//! 1. If `config.toml` has `temporal.host`, use it. `source = Configured`.
//! 2. Otherwise try a 200ms TCP preflight against `127.0.0.1:7233`.
//!    If reachable, use it (someone else already runs Temporal).
//!    `source = Configured`.
//! 3. Otherwise spawn `temporal server start-dev` as a long-lived child,
//!    write its PID to `~/.cori/state/temporal-dev.pid`, and wait up
//!    to 10s for the gRPC port to accept connections.
//!    `source = AutoSpawnedDev`.

use std::path::PathBuf;
use std::process::{Command, Output};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use cori_worker::runtime::preflight_check;

use crate::{config::Config, paths};

const DEV_TARGET: &str = "http://127.0.0.1:7233";
const PREFLIGHT_TIMEOUT: Duration = Duration::from_millis(200);
const SPAWN_WAIT: Duration = Duration::from_secs(10);
const ALLOW_NEW_DB_ENV: &str = "CORI_TEMPORAL_ALLOW_NEW_DB";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndpointSource {
    Configured,
    AutoSpawnedDev,
}

pub struct ResolvedEndpoint {
    pub target: String,
    #[allow(dead_code)]
    pub source: EndpointSource,
}

/// Honour `$CORI_TEMPORAL_TARGET` before consulting config.toml.
pub fn resolve() -> Result<ResolvedEndpoint> {
    if let Ok(env) = std::env::var("CORI_TEMPORAL_TARGET")
        && !env.is_empty()
    {
        return Ok(ResolvedEndpoint {
            target: env,
            source: EndpointSource::Configured,
        });
    }

    let cfg = Config::load().ok();
    if let Some(host) = cfg
        .as_ref()
        .and_then(|c| c.get("temporal.host"))
        .and_then(|v| v.as_str())
    {
        return Ok(ResolvedEndpoint {
            target: host.to_string(),
            source: EndpointSource::Configured,
        });
    }

    if preflight_check(DEV_TARGET, PREFLIGHT_TIMEOUT).is_ok() {
        return Ok(ResolvedEndpoint {
            target: DEV_TARGET.to_string(),
            source: EndpointSource::Configured,
        });
    }

    let state = paths::state_dir()?;
    std::fs::create_dir_all(&state).with_context(|| format!("creating `{}`", state.display()))?;
    // Hold one cross-process lock from the second endpoint probe through DB
    // migration, child spawn, and readiness. Temporal task startup is a
    // machine-level singleton operation, not a per-CLI-process race.
    let _startup_lock =
        crate::remote::git::FileLock::acquire(&state.join("temporal-dev-start.lock"))
            .context("locking local Temporal startup")?;

    if preflight_check(DEV_TARGET, PREFLIGHT_TIMEOUT).is_ok() {
        return Ok(ResolvedEndpoint {
            target: DEV_TARGET.to_string(),
            source: EndpointSource::AutoSpawnedDev,
        });
    }

    if pid_alive_from_file()? {
        wait_for_existing_dev_temporal()?;
        return Ok(ResolvedEndpoint {
            target: DEV_TARGET.to_string(),
            source: EndpointSource::AutoSpawnedDev,
        });
    }

    spawn_dev_temporal()?;
    Ok(ResolvedEndpoint {
        target: DEV_TARGET.to_string(),
        source: EndpointSource::AutoSpawnedDev,
    })
}

fn pid_file() -> Result<PathBuf> {
    Ok(paths::state_dir()?.join("temporal-dev.pid"))
}

fn announce_flag() -> Result<PathBuf> {
    Ok(paths::state_dir()?.join("dev-engine-announced"))
}

fn pid_alive_from_file() -> Result<bool> {
    let path = pid_file()?;
    let Ok(s) = std::fs::read_to_string(&path) else {
        return Ok(false);
    };
    let Ok(pid) = s.trim().parse::<u32>() else {
        return Ok(false);
    };
    Ok(is_alive(pid))
}

#[cfg(unix)]
fn is_alive(pid: u32) -> bool {
    unsafe { libc_kill(pid as i32, 0) == 0 }
}

#[cfg(unix)]
unsafe extern "C" {
    #[link_name = "kill"]
    fn libc_kill(pid: i32, sig: i32) -> i32;
}

#[cfg(not(unix))]
fn is_alive(pid: u32) -> bool {
    #[cfg(windows)]
    {
        const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
        const STILL_ACTIVE: u32 = 259;
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if handle.is_null() {
            return false;
        }
        let mut exit_code = 0_u32;
        let queried = unsafe { GetExitCodeProcess(handle, &mut exit_code) } != 0;
        unsafe {
            CloseHandle(handle);
        }
        queried && exit_code == STILL_ACTIVE
    }
    #[cfg(not(windows))]
    {
        let _ = pid;
        false
    }
}

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn OpenProcess(
        desired_access: u32,
        inherit_handle: i32,
        process_id: u32,
    ) -> *mut std::ffi::c_void;
    fn GetExitCodeProcess(process: *mut std::ffi::c_void, exit_code: *mut u32) -> i32;
    fn CloseHandle(object: *mut std::ffi::c_void) -> i32;
}

fn wait_for_existing_dev_temporal() -> Result<()> {
    let started = Instant::now();
    while started.elapsed() < SPAWN_WAIT {
        if preflight_check(DEV_TARGET, PREFLIGHT_TIMEOUT).is_ok() {
            return Ok(());
        }
        if !pid_alive_from_file()? {
            bail!("the recorded Temporal dev process exited before its endpoint became ready");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    bail!(
        "Temporal dev process is running but {} did not become ready within {}s; inspect `{}` before removing a stale PID file",
        DEV_TARGET,
        SPAWN_WAIT.as_secs(),
        pid_file()?.display()
    )
}

fn spawn_dev_temporal() -> Result<()> {
    if which("temporal").is_none() {
        bail!(
            "Temporal CLI not found on PATH. Install: brew install temporal (mac) \
             / see https://docs.temporal.io/cli"
        );
    }

    let version = temporal_version().context("reading the Temporal server version")?;
    let server_version = server_version(&version.stdout).ok_or_else(|| {
        anyhow::anyhow!(
            "could not determine the Temporal server schema version from `temporal --version` output: `{}`; refusing to select a local history database",
            String::from_utf8_lossy(&version.stdout).trim()
        )
    })?;
    let db = prepare_versioned_db(paths::home()?, server_version)?;

    let mut cmd = Command::new("temporal");
    cmd.args([
        "server",
        "start-dev",
        "--port",
        "7233",
        "--ui-port",
        "7234",
        "--headless",
        "--db-filename",
    ])
    .arg(&db)
    .args(["--log-level", "error"])
    .stdin(std::process::Stdio::null())
    .stdout(std::process::Stdio::null())
    .stderr(std::process::Stdio::null());
    cori_broker::process::hide_console_window(&mut cmd);

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            cmd.pre_exec(|| {
                if libc_setsid() < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }

    let child = cmd
        .spawn()
        .context("spawning `temporal server start-dev`")?;
    std::fs::write(pid_file()?, child.id().to_string())
        .with_context(|| "writing temporal-dev.pid")?;
    std::mem::forget(child);

    let started = Instant::now();
    loop {
        if preflight_check(DEV_TARGET, Duration::from_millis(200)).is_ok() {
            break;
        }
        if started.elapsed() > SPAWN_WAIT {
            bail!(
                "spawned `temporal server start-dev` but it did not accept connections within {}s",
                SPAWN_WAIT.as_secs()
            );
        }
        std::thread::sleep(Duration::from_millis(150));
    }

    let flag = announce_flag()?;
    if !flag.exists() {
        println!("Started local execution engine.");
        let _ = std::fs::write(&flag, "");
    }
    Ok(())
}

fn temporal_version() -> Result<Output> {
    let mut command = Command::new("temporal");
    command.arg("--version");
    cori_broker::process::hide_console_window(&mut command);
    let output = command.output().context("running `temporal --version`")?;
    if !output.status.success() {
        bail!(
            "`temporal --version` failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(output)
}

fn server_version(output: &[u8]) -> Option<&str> {
    let output = std::str::from_utf8(output).ok()?;
    let version = output
        .split("Server ")
        .nth(1)?
        .split([',', ')', ' '])
        .next()?;
    (!version.is_empty()
        && version
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_')))
    .then_some(version)
}

fn versioned_db_path(home: PathBuf, version: &str) -> PathBuf {
    home.join(format!("temporal-dev-server-{version}.db"))
}

/// Preserve the original unversioned dev database while seeding the
/// server-version-specific path introduced by newer Cori releases.
///
/// SQLite may have committed pages in `-wal`, so the whole file family is
/// copied. The target base file is renamed last and acts as the migration
/// commit marker. The legacy family remains untouched as a recoverable backup.
fn prepare_versioned_db(home: PathBuf, version: &str) -> Result<PathBuf> {
    let allow_new_database = std::env::var(ALLOW_NEW_DB_ENV).as_deref() == Ok("1");
    prepare_versioned_db_with_policy(home, version, allow_new_database)
}

fn prepare_versioned_db_with_policy(
    home: PathBuf,
    version: &str,
    allow_new_database: bool,
) -> Result<PathBuf> {
    let target = versioned_db_path(home.clone(), version);
    if target.exists() {
        return Ok(target);
    }

    let other_versioned = other_versioned_databases(&home, &target)?;
    if !other_versioned.is_empty() && !allow_new_database {
        let found = other_versioned
            .iter()
            .map(|path| format!("  - {}", path.display()))
            .collect::<Vec<_>>()
            .join("\n");
        bail!(
            "refusing to start a fresh local Temporal database for server version `{}` because \
             existing versioned history was found:\n{found}\n\nTemporal database files are tied \
            to the server schema. Use a Temporal CLI with the matching server version to retain \
             that history, or explicitly acknowledge a fresh local engine with \
             `{ALLOW_NEW_DB_ENV}=1`. Existing database files will not be deleted.",
            version,
        );
    }
    if !other_versioned.is_empty() {
        tracing::warn!(
            existing = ?other_versioned,
            target = %target.display(),
            opt_in = ALLOW_NEW_DB_ENV,
            "starting a fresh Temporal dev database after explicit opt-in"
        );
        return Ok(target);
    }

    let legacy = home.join("temporal-dev.db");
    if !legacy.is_file() {
        return Ok(target);
    }

    copy_sqlite_family(&legacy, &target).with_context(|| {
        format!(
            "migrating legacy Temporal database `{}` to `{}`; the legacy files were left untouched",
            legacy.display(),
            target.display()
        )
    })?;
    tracing::info!(
        legacy = %legacy.display(),
        versioned = %target.display(),
        "seeded versioned Temporal dev database from legacy files"
    );
    Ok(target)
}

fn other_versioned_databases(
    home: &std::path::Path,
    target: &std::path::Path,
) -> Result<Vec<PathBuf>> {
    let mut databases = Vec::new();
    let entries = match std::fs::read_dir(home) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(databases),
        Err(error) => {
            return Err(error).with_context(|| format!("reading `{}`", home.display()));
        }
    };
    for entry in entries {
        let entry =
            entry.with_context(|| format!("reading an entry under `{}`", home.display()))?;
        let path = entry.path();
        if path == target {
            continue;
        }
        if !entry
            .file_type()
            .with_context(|| format!("reading file type for `{}`", path.display()))?
            .is_file()
        {
            continue;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if name.starts_with("temporal-dev-server-") && name.ends_with(".db") {
            databases.push(path);
        }
    }
    databases.sort();
    Ok(databases)
}

fn copy_sqlite_family(source: &std::path::Path, target: &std::path::Path) -> Result<()> {
    struct PendingCopy {
        temp: PathBuf,
        destination: PathBuf,
        is_base: bool,
    }

    let migration_suffix = format!(".migrating-{}", std::process::id());
    let mut copies = Vec::new();
    for suffix in ["", "-wal", "-shm"] {
        let source_file = append_path_suffix(source, suffix);
        if !source_file.is_file() {
            continue;
        }
        let destination = append_path_suffix(target, suffix);
        let temp = append_path_suffix(&destination, &migration_suffix);
        if temp.exists() {
            std::fs::remove_file(&temp)
                .with_context(|| format!("removing stale `{}`", temp.display()))?;
        }
        std::fs::copy(&source_file, &temp).with_context(|| {
            format!(
                "copying `{}` to temporary migration file `{}`",
                source_file.display(),
                temp.display()
            )
        })?;
        copies.push(PendingCopy {
            temp,
            destination,
            is_base: suffix.is_empty(),
        });
    }

    let install = (|| -> Result<()> {
        // Install companions first and the base database last. A target base
        // therefore means the complete family was installed successfully.
        copies.sort_by_key(|copy| copy.is_base);
        for copy in &copies {
            if copy.destination.exists() {
                std::fs::remove_file(&copy.destination).with_context(|| {
                    format!(
                        "removing incomplete migration file `{}`",
                        copy.destination.display()
                    )
                })?;
            }
            std::fs::rename(&copy.temp, &copy.destination).with_context(|| {
                format!(
                    "installing migrated Temporal database file `{}`",
                    copy.destination.display()
                )
            })?;
        }
        Ok(())
    })();

    if let Err(error) = install {
        for copy in &copies {
            let _ = std::fs::remove_file(&copy.temp);
            let _ = std::fs::remove_file(&copy.destination);
        }
        return Err(error);
    }
    Ok(())
}

fn append_path_suffix(path: &std::path::Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(suffix);
    PathBuf::from(value)
}

#[cfg(unix)]
unsafe extern "C" {
    #[link_name = "setsid"]
    fn libc_setsid() -> i32;
}

fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let suffixes: &[&str] = if cfg!(windows) {
        &["", ".exe", ".cmd", ".bat"]
    } else {
        &[""]
    };
    for dir in std::env::split_paths(&path) {
        for sfx in suffixes {
            let cand = dir.join(format!("{name}{sfx}"));
            if cand.is_file() {
                return Some(cand);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{
        append_path_suffix, prepare_versioned_db, prepare_versioned_db_with_policy, server_version,
        versioned_db_path,
    };
    use std::path::PathBuf;
    use tempfile::tempdir;

    #[test]
    fn parses_server_version_from_temporal_cli_output() {
        assert_eq!(
            server_version(b"temporal version 1.7.2 (Server 1.31.1, UI 2.49.1)\n"),
            Some("1.31.1")
        );
        assert_eq!(server_version(b"temporal version unknown\n"), None);
    }

    #[test]
    fn isolates_dev_databases_by_server_schema_version() {
        let home = PathBuf::from("/tmp/cori-home");
        assert_eq!(
            versioned_db_path(home, "1.31.1"),
            PathBuf::from("/tmp/cori-home/temporal-dev-server-1.31.1.db")
        );
    }

    #[test]
    fn seeds_versioned_database_without_removing_legacy_family() {
        let temp = tempdir().expect("temporary Cori home");
        let legacy = temp.path().join("temporal-dev.db");
        std::fs::write(&legacy, b"base").expect("legacy base");
        std::fs::write(append_path_suffix(&legacy, "-wal"), b"wal").expect("legacy wal");
        std::fs::write(append_path_suffix(&legacy, "-shm"), b"shm").expect("legacy shm");

        let target =
            prepare_versioned_db(temp.path().to_path_buf(), "1.31.1").expect("database migration");

        assert_eq!(std::fs::read(&target).expect("target base"), b"base");
        assert_eq!(
            std::fs::read(append_path_suffix(&target, "-wal")).expect("target wal"),
            b"wal"
        );
        assert_eq!(
            std::fs::read(append_path_suffix(&target, "-shm")).expect("target shm"),
            b"shm"
        );
        assert_eq!(std::fs::read(&legacy).expect("legacy retained"), b"base");
        assert!(append_path_suffix(&legacy, "-wal").is_file());
        assert!(append_path_suffix(&legacy, "-shm").is_file());
    }

    #[test]
    fn existing_versioned_database_is_never_overwritten() {
        let temp = tempdir().expect("temporary Cori home");
        std::fs::write(temp.path().join("temporal-dev.db"), b"legacy").expect("legacy base");
        let target = versioned_db_path(temp.path().to_path_buf(), "1.31.1");
        std::fs::write(&target, b"current").expect("current base");

        let resolved =
            prepare_versioned_db(temp.path().to_path_buf(), "1.31.1").expect("existing database");

        assert_eq!(resolved, target);
        assert_eq!(std::fs::read(target).expect("current retained"), b"current");
    }

    #[test]
    fn refuses_silent_database_switch_between_server_versions() {
        let temp = tempdir().expect("temporary Cori home");
        let previous = versioned_db_path(temp.path().to_path_buf(), "1.30.0");
        std::fs::write(&previous, b"history").expect("previous database");

        let error = prepare_versioned_db_with_policy(temp.path().to_path_buf(), "1.31.1", false)
            .expect_err("version transition must require acknowledgement");

        assert!(error.to_string().contains("refusing to start a fresh"));
        assert!(error.to_string().contains("CORI_TEMPORAL_ALLOW_NEW_DB=1"));
        assert_eq!(
            std::fs::read(previous).expect("previous history retained"),
            b"history"
        );
    }

    #[test]
    fn explicit_opt_in_allows_a_fresh_versioned_database() {
        let temp = tempdir().expect("temporary Cori home");
        let previous = versioned_db_path(temp.path().to_path_buf(), "1.30.0");
        std::fs::write(&previous, b"history").expect("previous database");

        let target = prepare_versioned_db_with_policy(temp.path().to_path_buf(), "1.31.1", true)
            .expect("acknowledged version transition");

        assert_eq!(
            target,
            versioned_db_path(temp.path().to_path_buf(), "1.31.1")
        );
        assert!(
            !target.exists(),
            "Temporal should create the fresh database"
        );
        assert_eq!(
            std::fs::read(previous).expect("previous history retained"),
            b"history"
        );
    }
}
