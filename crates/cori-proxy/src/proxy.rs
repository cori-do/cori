//! Main proxy server implementation.
//!
//! Implements a Postgres wire protocol proxy that:
//! - Accepts connections on a configurable port
//! - Authenticates clients using Biscuit tokens
//! - Parses and rewrites SQL with RLS injection
//! - Forwards queries to upstream Postgres
//! - Logs all activity for audit

use cori_core::config::proxy::{ProxyConfig, UpstreamConfig};
use crate::error::ProxyError;
use crate::handler::{BiscuitAuthStartupHandler, CoriQueryHandler, CoriServerHandlers, SessionContext};
use cori_audit::AuditLogger;
use cori_biscuit::TokenVerifier;
use cori_core::config::TenancyConfig;
use cori_rls::RlsInjector;
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::RwLock;

/// Role permission configuration for virtual schema filtering.
#[derive(Debug, Clone)]
pub struct RolePermissions {
    /// Tables accessible by this role.
    pub accessible_tables: Vec<String>,
    /// Readable columns per table.
    pub readable_columns: HashMap<String, Vec<String>>,
}

/// The main Cori proxy server.
pub struct CoriProxy {
    config: ProxyConfig,
    upstream: UpstreamConfig,
    verifier: Arc<TokenVerifier>,
    injector: RlsInjector,
    audit: Option<Arc<AuditLogger>>,
    /// Role permissions for virtual schema filtering.
    role_permissions: Arc<HashMap<String, RolePermissions>>,
}

impl CoriProxy {
    /// Create a new proxy with the given configuration.
    pub fn new(
        config: ProxyConfig,
        upstream: UpstreamConfig,
        verifier: TokenVerifier,
        tenancy_config: TenancyConfig,
        audit: Option<AuditLogger>,
    ) -> Self {
        let injector = RlsInjector::new(tenancy_config);

        Self {
            config,
            upstream,
            verifier: Arc::new(verifier),
            injector,
            audit: audit.map(Arc::new),
            role_permissions: Arc::new(HashMap::new()),
        }
    }

    /// Create a new proxy with role permissions for virtual schema filtering.
    pub fn with_role_permissions(
        config: ProxyConfig,
        upstream: UpstreamConfig,
        verifier: TokenVerifier,
        tenancy_config: TenancyConfig,
        audit: Option<AuditLogger>,
        role_permissions: HashMap<String, RolePermissions>,
    ) -> Self {
        let injector = RlsInjector::new(tenancy_config);

        Self {
            config,
            upstream,
            verifier: Arc::new(verifier),
            injector,
            audit: audit.map(Arc::new),
            role_permissions: Arc::new(role_permissions),
        }
    }

    /// Get a reference to the proxy configuration.
    pub fn config(&self) -> &ProxyConfig {
        &self.config
    }

    /// Create handlers for a new connection.
    /// Each connection gets its own session context that is shared between
    /// the startup handler (authentication) and query handler.
    fn create_connection_handlers(
        &self,
        pool: PgPool,
    ) -> Arc<CoriServerHandlers> {
        // Create a session context for this connection
        let session = Arc::new(RwLock::new(SessionContext::default()));

        // Create the startup handler (handles Biscuit token authentication)
        let startup_handler = Arc::new(BiscuitAuthStartupHandler::new_with_role_permissions(
            self.verifier.clone(),
            session.clone(),
            self.role_permissions.clone(),
        ));

        // Create the query handler (handles SQL queries with RLS injection)
        let query_handler = Arc::new(CoriQueryHandler::new_with_session(
            self.injector.clone(),
            self.verifier.clone(),
            pool,
            self.audit.clone(),
            session,
        ));

        Arc::new(CoriServerHandlers::new(query_handler, startup_handler))
    }

    /// Run the proxy server.
    pub async fn run(&self) -> Result<(), ProxyError> {
        let listen_addr = format!("{}:{}", self.config.listen_addr, self.config.listen_port);

        tracing::info!(
            listen_addr = %listen_addr,
            upstream = %self.upstream.connection_string(),
            "Starting Cori proxy server"
        );

        // Create upstream connection pool
        let pool = PgPoolOptions::new()
            .max_connections(self.config.max_connections)
            .connect(&self.upstream.connection_string())
            .await
            .map_err(|e| ProxyError::UpstreamConnectionFailed(e.to_string()))?;

        tracing::info!("Connected to upstream PostgreSQL");

        // Bind to the listen address
        let listener = TcpListener::bind(&listen_addr).await.map_err(|e| {
            ProxyError::BindFailed {
                address: listen_addr.clone(),
                source: e,
            }
        })?;

        tracing::info!(address = %listen_addr, "Proxy server listening");

        loop {
            let (socket, peer_addr) = match listener.accept().await {
                Ok(conn) => conn,
                Err(e) => {
                    tracing::error!(error = %e, "Failed to accept connection");
                    continue;
                }
            };

            tracing::debug!(peer = %peer_addr, "New connection");

            // Create new handlers for this connection (with its own session)
            let handlers = self.create_connection_handlers(pool.clone());

            tokio::spawn(async move {
                if let Err(e) = pgwire::tokio::process_socket(socket, None, handlers).await {
                    tracing::error!(peer = %peer_addr, error = ?e, "Connection error");
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cori_biscuit::KeyPair;

    #[test]
    fn test_proxy_creation() {
        let keypair = KeyPair::generate().unwrap();
        let verifier = TokenVerifier::new(keypair.public_key());
        let tenancy_config = TenancyConfig::default();
        let config = ProxyConfig {
            listen_addr: "127.0.0.1".to_string(),
            listen_port: 5433,
            max_connections: 10,
            connection_timeout: 30,
            tls: Default::default(),
        };
        let upstream = UpstreamConfig {
            host: "localhost".to_string(),
            port: 5432,
            database: "test".to_string(),
            username: "test".to_string(),
            password: None,
            credentials_env: None,
        };

        let proxy = CoriProxy::new(config, upstream, verifier, tenancy_config, None);
        assert_eq!(proxy.config.listen_port, 5433);
    }
}
