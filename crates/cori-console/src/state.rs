//! Shared application state for the Console HTTP server.

use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::RwLock;

use crate::runs::{RunRegistry, new_registry};

/// Console server state.
///
/// `master_token` is the random secret printed once in the startup URL.
/// It is **not** persisted to disk. The SPA exchanges it (via
/// `POST /api/session`) for a separate `session_value` that subsequent
/// GET requests carry as an `HttpOnly` cookie. State-changing endpoints
/// additionally require the bearer header carrying `master_token`.
#[derive(Clone)]
pub struct AppState {
    /// One-time master token. Compared by the session-exchange handler
    /// and the bearer middleware.
    pub master_token: Arc<String>,
    /// Session cookie value set after the master token is exchanged.
    /// `None` until the SPA calls `POST /api/session`.
    pub session_value: Arc<RwLock<Option<String>>>,
    /// `~/.cori/` root — pre-resolved at startup so endpoints don't have
    /// to re-read `$CORI_HOME` on every request.
    pub home: PathBuf,
    /// In-memory registry of Console-triggered runs keyed by `run_id`.
    /// Drives SSE event replay + live broadcast.
    pub runs: RunRegistry,
}

impl AppState {
    pub fn new(master_token: String, home: PathBuf) -> Self {
        Self {
            master_token: Arc::new(master_token),
            session_value: Arc::new(RwLock::new(None)),
            home,
            runs: new_registry(),
        }
    }
}
