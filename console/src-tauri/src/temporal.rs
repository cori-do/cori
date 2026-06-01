//! Helpers for Temporal-endpoint reasoning in the Tauri app.
//!
//! We deliberately avoid `cori_run::temporal_endpoint::resolve()` here —
//! its third fallback step spawns `temporal server start-dev` as an
//! unowned child, which the Tauri app cannot supervise or kill on Quit.
//! The supervisor only adopts processes it spawned itself.

use std::net::TcpListener;
use std::time::Duration;

use cori_worker::runtime::CoriTemporalRuntime;

const PROBE_NAMESPACE: &str = "default";
const PROBE_QUEUE: &str = "cori.console.probe";

/// Returns the configured target if the user explicitly pointed us at
/// an external Temporal (env var or `~/.cori/config.toml`). Does not
/// probe anything. `None` means "we are free to spawn our own".
pub(crate) fn external_target_configured() -> Option<String> {
    if let Ok(env) = std::env::var("CORI_TEMPORAL_TARGET")
        && !env.is_empty()
    {
        return Some(normalize_url(&env));
    }
    if let Ok(cfg) = cori_run::config::Config::load()
        && let Some(host) = cfg.get("temporal.host").and_then(|v| v.as_str())
        && !host.is_empty()
    {
        return Some(normalize_url(host));
    }
    None
}

/// Probe whether the given URL is actually serving the Temporal gRPC
/// protocol — distinct from "anything is listening on the TCP port".
/// We do this with a real Temporal client handshake bounded by a 2 s
/// timeout. The connection is discarded immediately on success.
pub(crate) async fn is_temporal_listener(target: &str) -> bool {
    let target = target.to_string();
    let fut = CoriTemporalRuntime::connect(target, PROBE_NAMESPACE, PROBE_QUEUE);
    matches!(
        tokio::time::timeout(Duration::from_secs(2), fut).await,
        Ok(Ok(_))
    )
}

/// Ask the OS for a free TCP port on the loopback interface. There is
/// a small race between drop and the subsequent rebind by the child,
/// but it is acceptable in a local dev / single-user context.
pub(crate) fn find_free_local_port() -> std::io::Result<u16> {
    let l = TcpListener::bind("127.0.0.1:0")?;
    let port = l.local_addr()?.port();
    drop(l);
    Ok(port)
}

/// Returns `true` if we can grab `127.0.0.1:<port>` ourselves (i.e.
/// nothing is bound). Drops the listener immediately on success so the
/// caller can pass the port to the sidecar.
pub(crate) fn can_bind_port(port: u16) -> bool {
    TcpListener::bind(("127.0.0.1", port)).map(drop).is_ok()
}

fn normalize_url(s: &str) -> String {
    if s.starts_with("http://") || s.starts_with("https://") {
        s.to_string()
    } else {
        format!("http://{s}")
    }
}
