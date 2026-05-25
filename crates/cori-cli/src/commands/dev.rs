//! `cori dev`.
//!
//! Boots the worker daemon and the local HTTP server (`cori serve start`)
//! together so workflow authors only need to keep one terminal open. The
//! server runs on a dedicated thread with its own tokio runtime; the
//! worker takes the main thread and owns the SIGINT/SIGTERM handler. When
//! the worker returns, the process exits and the server thread is torn
//! down with it.

use anyhow::Result;

pub fn run() -> Result<()> {
    let _serve_thread = std::thread::Builder::new()
        .name("cori-serve".to_string())
        .spawn(|| {
            if let Err(e) = super::serve::start(None, false) {
                eprintln!("serve thread exited: {e}");
            }
        })?;
    super::worker::start()
}
