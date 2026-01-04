//! Dashboard server implementation.

use cori_core::config::CoriConfig;
use cori_core::DashboardConfig;
use cori_biscuit::keys::KeyPair;
use cori_audit::AuditLogger;
use cori_mcp::approval::ApprovalManager;
use crate::error::DashboardError;
use crate::routes;
use crate::state::AppState;
use std::sync::Arc;
use tokio::net::TcpListener;

/// The dashboard server.
pub struct DashboardServer {
    config: DashboardConfig,
    state: Option<AppState>,
}

impl DashboardServer {
    /// Create a new dashboard server with the given configuration.
    pub fn new(config: DashboardConfig) -> Self {
        Self { config, state: None }
    }

    /// Create a new dashboard server with full application state.
    pub fn with_state(
        dashboard_config: DashboardConfig,
        cori_config: CoriConfig,
        _keypair: KeyPair,
        audit_logger: Arc<AuditLogger>,
        approval_manager: Arc<ApprovalManager>,
    ) -> Self {
        let state = AppState::new(cori_config)
            .with_audit_logger(audit_logger)
            .with_approval_manager(approval_manager);
        Self {
            config: dashboard_config,
            state: Some(state),
        }
    }

    /// Start the dashboard server.
    pub async fn run(&self) -> Result<(), DashboardError> {
        let addr = format!("0.0.0.0:{}", self.config.listen_port);
        tracing::info!(address = %addr, "Starting Cori dashboard");

        let app = if let Some(state) = &self.state {
            // Try to load schema at startup if database URL is configured
            if let Some(db_url) = state.database_url() {
                match cori_adapter_pg::introspect::introspect_schema_json(db_url).await {
                    Ok(schema_json) => {
                        match crate::schema_converter::json_to_schema_info(&schema_json) {
                            Ok(schema_info) => {
                                state.set_schema_cache(schema_info);
                                tracing::info!("Database schema loaded successfully at startup");
                            }
                            Err(e) => {
                                tracing::warn!(error = %e, "Failed to parse database schema at startup");
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "Failed to introspect database schema at startup");
                    }
                }
            }
            routes::create_router_with_state(state.clone())
        } else {
            // Fallback to empty router for testing
            routes::create_router()
        };

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
    
    /// Get a reference to the application state if available.
    pub fn state(&self) -> Option<&AppState> {
        self.state.as_ref()
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

