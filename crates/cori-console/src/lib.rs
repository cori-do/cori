//! `cori-console` — local web UI served by `cori work`.
//!
//! Bound to `127.0.0.1` only. Single-user, single-token gated. Reads
//! the same `~/.cori/` state the CLI does (run traces, cluster reports,
//! pinned remotes) via `cori-run::paths`. Trigger / SSE / schedules
//! land in later phases; Phase 1 provides the server skeleton and
//! read endpoints (`/api/status`, `/api/runs`, `/api/runs/:key/:filename`,
//! `/api/workflows/recent`).

pub mod api;
pub mod auth;
pub mod error;
pub mod state;
pub mod token;

use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;

use anyhow::{Result, bail};

pub use state::AppState;
pub use token::{generate_session_value, generate_token};

/// Find an available port. Prefers `preferred`; if it's taken (or
/// `0`), asks the OS for any free port. Races with concurrent binders
/// in principle, but for a localhost single-user tool that's fine.
pub fn find_available_port(preferred: u16) -> Result<u16> {
    use std::net::TcpListener;
    if preferred != 0
        && let Ok(l) = TcpListener::bind(("127.0.0.1", preferred))
    {
        let port = l.local_addr()?.port();
        drop(l);
        return Ok(port);
    }
    let l = TcpListener::bind(("127.0.0.1", 0))?;
    let port = l.local_addr()?.port();
    Ok(port)
}

/// Serve the Console on `127.0.0.1:<port>` until the future is cancelled.
///
/// Panics if `port == 0` (caller must resolve via [`find_available_port`]
/// first). Rejects non-loopback bind addresses with a hard error — the
/// Console is single-user, localhost only by design.
pub async fn serve(port: u16, master_token: String, home: PathBuf) -> Result<()> {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    if !addr.ip().is_loopback() {
        bail!("Cori Console refuses to bind a non-loopback address ({addr})");
    }
    serve_at(addr, master_token, home).await
}

/// Like [`serve`] but takes a pre-built `SocketAddr`. Also asserts loopback.
pub async fn serve_at(addr: SocketAddr, master_token: String, home: PathBuf) -> Result<()> {
    let ip: IpAddr = addr.ip();
    if !ip.is_loopback() {
        bail!("Cori Console refuses to bind a non-loopback address ({addr})");
    }
    let state = AppState::new(master_token, home);
    let router = api::build_router(state);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, "Cori Console listening");
    axum::serve(listener, router).await?;
    Ok(())
}
