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
//!
//! The auto-spawned child is **not** killed when `cori run` exits —
//! it's a shared dev resource (one per machine). On every fresh
//! `cori run` we re-check the PID file; if the process is still
//! alive we attach to its socket instead of spawning a new one.
//!
//! On the first auto-spawn per machine we print:
//!
//! ```text
//! Started local execution engine.
//! ```
//!
//! and touch `~/.cori/state/dev-engine-announced` so subsequent
//! invocations are silent.

use std::path::PathBuf;
use std::process::Command;
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
    /// Where the endpoint came from. Future phases will display this
    /// in `cori doctor` / status output; today nothing reads it.
    #[allow(dead_code)]
    pub source: EndpointSource,
}

/// Honour `$CORI_TEMPORAL_TARGET` (a back-door used by tests and the
/// pre-redesign documentation) before consulting config.toml.
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

    // Existing PID file with a live process? Attach without respawning.
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
    // `kill -0` checks for the process existence without sending a signal.
    unsafe { libc_kill(pid as i32, 0) == 0 }
}

#[cfg(unix)]
unsafe extern "C" {
    #[link_name = "kill"]
    fn libc_kill(pid: i32, sig: i32) -> i32;
}

#[cfg(not(unix))]
fn is_alive(_pid: u32) -> bool {
    // No portable way without an extra dep. Fall through to re-spawn;
    // the user will see the spawn fail if Temporal is already bound.
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
    let db = paths::home()?.join("temporal-dev.db");

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
        // Detach so the child outlives this `cori run` invocation.
        use std::os::unix::process::CommandExt;
        unsafe {
            cmd.pre_exec(|| {
                // New session — survives shell hangup, doesn't get our SIGINT.
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
    // Forget the child handle so its `Drop` doesn't reap it on our exit.
    std::mem::forget(child);

    // Wait for the gRPC port to accept connections.
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

    // One-line notice the first time per machine.
    let flag = announce_flag()?;
    if !flag.exists() {
        println!("Started local execution engine.");
        let _ = std::fs::write(&flag, "");
    }
    Ok(())
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
