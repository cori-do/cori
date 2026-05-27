//! Temporal runtime bootstrap.
//!
//! Wraps `CoreRuntime` + `Client` + `Connection` setup so callers (the CLI
//! and the long-running worker daemon) just call [`CoriTemporalRuntime::connect`]
//! to get a fully-wired handle.
//!
//! The runtime is a per-process singleton in practice: each `cori run`
//! spawns one, runs one workflow, and drops it. The long-running `cori
//! worker start` daemon owns a single runtime for its whole lifetime.

use std::sync::Arc;

use anyhow::{Context, Result};
use temporalio_client::{Client, ClientOptions, Connection, ConnectionOptions};
use temporalio_common::telemetry::TelemetryOptions;
use temporalio_sdk_core::{CoreRuntime, RuntimeOptions, Url};

/// Default Temporal task queue Cori uses for workflows and activities.
pub const DEFAULT_TASK_QUEUE: &str = "cori-default";

/// Default Temporal namespace Cori uses.
pub const DEFAULT_NAMESPACE: &str = "default";

/// Default endpoint when nothing is configured.
pub const DEFAULT_TEMPORAL_TARGET: &str = "http://localhost:7233";

/// A live Temporal connection plus the Core runtime that owns the gRPC
/// channels and metrics. Cheap to clone (`Arc` internals).
#[derive(Clone)]
pub struct CoriTemporalRuntime {
    pub core: Arc<CoreRuntime>,
    pub client: Arc<Client>,
    pub task_queue: String,
    pub namespace: String,
    pub target: String,
}

impl CoriTemporalRuntime {
    /// Connect to a Temporal endpoint. `target` is a full URL like
    /// `http://127.0.0.1:7233`. `namespace` is usually `"default"`.
    /// `task_queue` is the task queue this process will poll if it
    /// registers a worker.
    pub async fn connect(
        target: impl Into<String>,
        namespace: impl Into<String>,
        task_queue: impl Into<String>,
    ) -> Result<Self> {
        let target = target.into();
        let namespace = namespace.into();
        let task_queue = task_queue.into();

        let core = CoreRuntime::new_assume_tokio(
            RuntimeOptions::builder()
                .telemetry_options(TelemetryOptions::builder().build())
                .build()
                .map_err(anyhow::Error::msg)
                .context("building Temporal RuntimeOptions")?,
        )
        .context("creating Temporal CoreRuntime")?;

        let url = Url::parse(&target)
            .with_context(|| format!("invalid Temporal target URL `{target}`"))?;
        let conn_opts = ConnectionOptions::new(url).identity("cori").build();
        let connection = Connection::connect(conn_opts)
            .await
            .with_context(|| friendly_connect_error(&target))?;
        let client_opts = ClientOptions::new(namespace.clone()).build();
        let client =
            Client::new(connection, client_opts).context("constructing Temporal Client")?;

        Ok(Self {
            core: Arc::new(core),
            client: Arc::new(client),
            task_queue,
            namespace,
            target,
        })
    }
}

fn friendly_connect_error(target: &str) -> String {
    format!(
        "could not connect to Temporal at `{target}`. \
         Start one locally with:\n  temporal server start-dev\n\
         or run `cori start --local` (which supervises a bundled Temporal)."
    )
}

/// Cheap synchronous TCP preflight against the Temporal target.
///
/// Returns `Ok(())` if a TCP connection to `host:port` succeeds within
/// `timeout`; otherwise an error whose message is the user-friendly
/// "start temporal locally" hint. Use this before kicking off a `cori
/// run` so we can fail fast with a clean message instead of buried
/// inside a tonic transport error.
pub fn preflight_check(target: &str, timeout: std::time::Duration) -> Result<()> {
    use std::net::ToSocketAddrs;

    let url =
        Url::parse(target).with_context(|| format!("invalid Temporal target URL `{target}`"))?;
    let host = url
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("Temporal target `{target}` has no host"))?;
    let port = url
        .port_or_known_default()
        .ok_or_else(|| anyhow::anyhow!("Temporal target `{target}` has no port"))?;

    let addrs: Vec<_> = (host, port)
        .to_socket_addrs()
        .with_context(|| format!("resolving Temporal host `{host}:{port}`"))?
        .collect();
    if addrs.is_empty() {
        anyhow::bail!("Temporal host `{host}:{port}` resolved to no addresses");
    }

    let mut last_err: Option<std::io::Error> = None;
    for addr in addrs {
        match std::net::TcpStream::connect_timeout(&addr, timeout) {
            Ok(_) => return Ok(()),
            Err(e) => last_err = Some(e),
        }
    }
    let detail = last_err.map(|e| format!(" ({e})")).unwrap_or_default();
    anyhow::bail!("{}{}", friendly_connect_error(target), detail)
}
