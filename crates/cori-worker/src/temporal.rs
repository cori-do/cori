//! Supervise the bundled Temporal CLI as a child process.
//!
//! Phase 6 §6.2: the Cori binary ships alongside the `temporal` CLI binary
//! (an external sibling, not embedded) and `cori worker start` is
//! responsible for spawning `temporal server start-dev` when no external
//! cluster is configured.
//!
//! The supervisor:
//!
//! 1. Reads `temporal.host` from CLI config; if set, skips spawning and
//!    just connects to the external endpoint.
//! 2. Probes the default local port to detect an already-running server
//!    (so a second `cori worker start` doesn't fight the first one for
//!    7233).
//! 3. Locates a `temporal` binary — first on `PATH`, then as a sibling of
//!    the running `cori` binary (the install layout).
//! 4. Spawns `temporal server start-dev` with a Cori-local SQLite DB.
//! 5. Waits up to 10s for the gRPC port to accept TCP connections.
//! 6. Registers a `Drop` impl that SIGTERMs the child so worker shutdown
//!    leaves no orphans.
//!
//! Real workflow execution against Temporal (registering workflow types,
//! polling activity tasks via `temporal-sdk-core`) lands on top of this
//! supervisor in a follow-up — the Phase 6 milestone is the supervision
//! contract.

use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use tracing::{debug, info, warn};

pub const DEFAULT_GRPC_PORT: u16 = 7233;
pub const DEFAULT_UI_PORT: u16 = 7234;
const HEALTH_CHECK_TIMEOUT: Duration = Duration::from_secs(10);
const HEALTH_CHECK_INTERVAL: Duration = Duration::from_millis(200);
const TCP_CONNECT_TIMEOUT: Duration = Duration::from_millis(500);

/// How Cori obtained the Temporal endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// We connected to an externally-managed cluster (config or pre-existing
    /// local server). No child process to clean up.
    External,
    /// We spawned `temporal server start-dev` ourselves and own the child.
    Bundled,
}

/// A live (or already-attached) Temporal endpoint.
pub struct Supervisor {
    pub host: String,
    pub grpc_port: u16,
    pub ui_port: Option<u16>,
    pub source: Source,
    child: Option<Child>,
}

impl Supervisor {
    /// Connect to or spawn a Temporal server.
    ///
    /// `configured_host` is `Some("host:port")` when the user set
    /// `temporal.host` in `~/.cori/config.toml`. `state_dir` is the directory
    /// to keep the local SQLite DB in (`~/.cori/state/temporal/`).
    pub fn start(configured_host: Option<&str>, state_dir: &std::path::Path) -> Result<Self> {
        // 1. External cluster wins.
        if let Some(host) = configured_host {
            let (h, p) = parse_host(host)?;
            wait_for_port(&h, p, HEALTH_CHECK_TIMEOUT).with_context(|| {
                format!("could not reach external Temporal cluster at `{host}`")
            })?;
            info!(host = host, "connected to external Temporal cluster");
            return Ok(Self {
                host: h,
                grpc_port: p,
                ui_port: None,
                source: Source::External,
                child: None,
            });
        }

        // 2. Already running locally?
        if probe_port("127.0.0.1", DEFAULT_GRPC_PORT) {
            info!(
                port = DEFAULT_GRPC_PORT,
                "attaching to existing Temporal server on 127.0.0.1"
            );
            return Ok(Self {
                host: "127.0.0.1".to_string(),
                grpc_port: DEFAULT_GRPC_PORT,
                ui_port: Some(DEFAULT_UI_PORT),
                source: Source::External,
                child: None,
            });
        }

        // 3. Spawn a fresh dev server.
        let temporal_bin = locate_temporal_binary().ok_or_else(|| {
            anyhow::anyhow!(
                "could not find `temporal` binary on PATH or alongside `cori`.\n\
                 Install Temporal CLI from https://docs.temporal.io/cli, or set \
                 `temporal.host` to point at an existing cluster:\n  \
                 cori config set temporal.host 127.0.0.1:7233"
            )
        })?;

        std::fs::create_dir_all(state_dir).with_context(|| {
            format!(
                "creating Temporal state directory `{}`",
                state_dir.display()
            )
        })?;
        let db_file = state_dir.join("dev.db");

        debug!(
            bin = %temporal_bin.display(),
            db = %db_file.display(),
            "spawning temporal server start-dev",
        );

        let log_file = state_dir.join("temporal.log");
        let stdout = std::fs::File::create(&log_file)
            .with_context(|| format!("opening Temporal log file `{}`", log_file.display()))?;
        let stderr = stdout
            .try_clone()
            .context("duplicating Temporal log file handle")?;

        let child = Command::new(&temporal_bin)
            .arg("server")
            .arg("start-dev")
            .arg("--db-filename")
            .arg(&db_file)
            .arg("--port")
            .arg(DEFAULT_GRPC_PORT.to_string())
            .arg("--ui-port")
            .arg(DEFAULT_UI_PORT.to_string())
            .arg("--log-level")
            .arg("warn")
            .stdin(Stdio::null())
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr))
            .spawn()
            .with_context(|| format!("spawning `{}`", temporal_bin.display()))?;

        let pid = child.id();
        let mut sup = Self {
            host: "127.0.0.1".to_string(),
            grpc_port: DEFAULT_GRPC_PORT,
            ui_port: Some(DEFAULT_UI_PORT),
            source: Source::Bundled,
            child: Some(child),
        };

        if let Err(e) = wait_for_port(&sup.host, sup.grpc_port, HEALTH_CHECK_TIMEOUT) {
            // Kill the child we just spawned so we don't leave it
            // dangling on a startup failure.
            sup.shutdown();
            let log_excerpt = std::fs::read_to_string(&log_file)
                .map(|s| {
                    s.lines()
                        .rev()
                        .take(20)
                        .collect::<Vec<_>>()
                        .into_iter()
                        .rev()
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default();
            bail!(
                "Temporal server (pid {pid}) did not become healthy on port {} within {}s: {e}\n\
                 --- last lines of {} ---\n{}",
                sup.grpc_port,
                HEALTH_CHECK_TIMEOUT.as_secs(),
                log_file.display(),
                log_excerpt,
            );
        }

        info!(
            pid,
            host = sup.host,
            port = sup.grpc_port,
            ui = sup.ui_port.unwrap_or(0),
            "bundled Temporal server is ready",
        );
        Ok(sup)
    }

    /// Send SIGTERM (or a TerminateProcess on Windows) to the child and
    /// wait briefly for it. Safe to call multiple times.
    pub fn shutdown(&mut self) {
        let Some(mut child) = self.child.take() else {
            return;
        };
        let pid = child.id();
        debug!(pid, "terminating bundled Temporal child");

        #[cfg(unix)]
        {
            // Polite SIGTERM first.
            unsafe {
                libc_kill(pid as i32, 15 /* SIGTERM */);
            }
        }
        #[cfg(not(unix))]
        {
            let _ = child.kill();
        }

        // Give Temporal up to 3s to flush and exit cleanly; SIGKILL after.
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    debug!(pid, ?status, "bundled Temporal child exited");
                    return;
                }
                Ok(None) if Instant::now() >= deadline => {
                    warn!(
                        pid,
                        "bundled Temporal did not exit within 3s — sending SIGKILL"
                    );
                    let _ = child.kill();
                    let _ = child.wait();
                    return;
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(100)),
                Err(e) => {
                    warn!(pid, error = %e, "error waiting for Temporal child");
                    let _ = child.kill();
                    return;
                }
            }
        }
    }

    pub fn endpoint(&self) -> String {
        format!("{}:{}", self.host, self.grpc_port)
    }

    pub fn ui_url(&self) -> Option<String> {
        self.ui_port.map(|p| format!("http://{}:{p}", self.host))
    }
}

impl Drop for Supervisor {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(unix)]
unsafe extern "C" {
    fn kill(pid: i32, sig: i32) -> i32;
}

#[cfg(unix)]
#[allow(non_snake_case)]
unsafe fn libc_kill(pid: i32, sig: i32) -> i32 {
    unsafe { kill(pid, sig) }
}

fn parse_host(s: &str) -> Result<(String, u16)> {
    let s = s
        .trim()
        .trim_start_matches("http://")
        .trim_start_matches("https://");
    let s = s.trim_end_matches('/');
    let (host, port) = s
        .rsplit_once(':')
        .with_context(|| format!("`temporal.host` must be `host:port` (got `{s}`)"))?;
    let port: u16 = port
        .parse()
        .with_context(|| format!("invalid port in `temporal.host` (got `{port}`)"))?;
    Ok((host.to_string(), port))
}

fn probe_port(host: &str, port: u16) -> bool {
    let addr: SocketAddr = match format!("{host}:{port}").parse() {
        Ok(a) => a,
        Err(_) => return false,
    };
    TcpStream::connect_timeout(&addr, TCP_CONNECT_TIMEOUT).is_ok()
}

fn wait_for_port(host: &str, port: u16, timeout: Duration) -> Result<()> {
    let deadline = Instant::now() + timeout;
    let mut last_err: Option<std::io::Error> = None;
    while Instant::now() < deadline {
        let addr: SocketAddr = format!("{host}:{port}")
            .parse()
            .with_context(|| format!("invalid `{host}:{port}` socket address"))?;
        match TcpStream::connect_timeout(&addr, TCP_CONNECT_TIMEOUT) {
            Ok(_) => return Ok(()),
            Err(e) => last_err = Some(e),
        }
        std::thread::sleep(HEALTH_CHECK_INTERVAL);
    }
    match last_err {
        Some(e) => Err(anyhow::Error::new(e)),
        None => bail!("port {port} on {host} never accepted a connection"),
    }
}

/// Find a `temporal` binary on PATH first, then as a sibling of the
/// currently running `cori` executable (the install layout).
fn locate_temporal_binary() -> Option<PathBuf> {
    let exe_name = if cfg!(windows) {
        "temporal.exe"
    } else {
        "temporal"
    };
    // 1. PATH.
    if let Some(path_var) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path_var) {
            let cand = dir.join(exe_name);
            if cand.is_file() {
                return Some(cand);
            }
        }
    }
    // 2. Sibling of the current executable.
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            let cand = parent.join(exe_name);
            if cand.is_file() {
                return Some(cand);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_host_accepts_plain_and_schemes() {
        assert_eq!(
            parse_host("127.0.0.1:7233").unwrap(),
            ("127.0.0.1".into(), 7233)
        );
        assert_eq!(
            parse_host("http://localhost:7233/").unwrap(),
            ("localhost".into(), 7233)
        );
    }

    #[test]
    fn parse_host_rejects_missing_port() {
        assert!(parse_host("127.0.0.1").is_err());
    }
}
