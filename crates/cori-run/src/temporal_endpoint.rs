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

    if pid_alive_from_file()? && preflight_check(DEV_TARGET, PREFLIGHT_TIMEOUT).is_ok() {
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
fn is_alive(_pid: u32) -> bool {
    false
}

fn spawn_dev_temporal() -> Result<()> {
    if which("temporal").is_none() {
        bail!(
            "Temporal CLI not found on PATH. Install: brew install temporal (mac) \
             / see https://docs.temporal.io/cli"
        );
    }

    let state = paths::state_dir()?;
    std::fs::create_dir_all(&state).with_context(|| format!("creating `{}`", state.display()))?;
    let version = temporal_version().context("reading the Temporal server version")?;
    let db = versioned_db_path(paths::home()?, server_version(&version.stdout));

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
    let output = Command::new("temporal")
        .arg("--version")
        .output()
        .context("running `temporal --version`")?;
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

fn versioned_db_path(home: PathBuf, version: Option<&str>) -> PathBuf {
    let suffix = version.unwrap_or("current");
    home.join(format!("temporal-dev-server-{suffix}.db"))
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
    use super::{server_version, versioned_db_path};
    use std::path::PathBuf;

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
            versioned_db_path(home, Some("1.31.1")),
            PathBuf::from("/tmp/cori-home/temporal-dev-server-1.31.1.db")
        );
    }
}
