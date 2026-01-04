//! Dashboard server implementation.

use cori_core::DashboardConfig;
use crate::error::DashboardError;
use crate::routes;
use tokio::net::TcpListener;

/// The dashboard server.
pub struct DashboardServer {
    config: DashboardConfig,
}

impl DashboardServer {
    /// Create a new dashboard server with the given configuration.
    pub fn new(config: DashboardConfig) -> Self {
        Self { config }
    }

    /// Start the dashboard server.
    pub async fn run(&self) -> Result<(), DashboardError> {
        let addr = format!("0.0.0.0:{}", self.config.listen_port);
        tracing::info!(address = %addr, "Starting Cori dashboard");

        let app = routes::create_router();

        let listener = TcpListener::bind(&addr)
            .await
            .map_err(|e| DashboardError::StartupFailed(e.to_string()))?;

        axum::serve(listener, app)
            .await
            .map_err(|e| DashboardError::StartupFailed(e.to_string()))?;

        Ok(())
    }

    /// Get the configured listen port.
    pub fn listen_port(&self) -> u16 {
        self.config.listen_port
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_server_creation() {
        let config = DashboardConfig::default();
        let server = DashboardServer::new(config);
        assert_eq!(server.listen_port(), 8080);
    }
}

