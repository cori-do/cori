//! Run command for starting Cori.
//!
//! `cori run` - Start the MCP server and dashboard (HTTP mode by default).
//! `cori run --stdio` - Start in stdio mode with a baked-in token.
//!
//! The dashboard always runs by default (use --no-dashboard to disable).

use anyhow::{Context, Result};
use cori_audit::AuditLogger;
use cori_biscuit::{PublicKey, TokenVerifier, keys::load_public_key_file};
use cori_core::config::role_definition::RoleDefinition;
use cori_core::config::rules_definition::RulesDefinition;
use cori_core::config::{AuditConfig, DashboardConfig, McpConfig, Transport, UpstreamConfig};
use cori_dashboard::DashboardServer;
use cori_mcp::McpServer;
use cori_mcp::approval::ApprovalManager;
use serde::Deserialize;
use sqlx::postgres::PgPoolOptions;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::{info, warn};

/// Configuration file structure for `cori run`.
#[derive(Debug, Deserialize)]
pub struct RunConfig {
    /// Upstream Postgres connection.
    pub upstream: UpstreamConfigFile,

    /// Biscuit configuration.
    pub biscuit: BiscuitConfig,

    /// Audit configuration.
    #[serde(default)]
    pub audit: AuditConfigFile,

    /// Approvals configuration.
    #[serde(default)]
    pub approvals: ApprovalsConfigFile,

    /// MCP server configuration.
    #[serde(default)]
    pub mcp: McpConfigFile,

    /// Dashboard configuration.
    #[serde(default)]
    pub dashboard: DashboardConfigFile,
}

#[derive(Debug, Deserialize)]
pub struct UpstreamConfigFile {
    /// Hostname (optional if database_url_env is set)
    pub host: Option<String>,
    #[serde(default = "default_upstream_port")]
    pub port: u16,
    /// Database name (optional if database_url_env is set)
    pub database: Option<String>,
    /// Username (optional if database_url_env is set)
    pub username: Option<String>,
    pub password: Option<String>,
    /// Environment variable containing DATABASE_URL (recommended)
    pub database_url_env: Option<String>,
    /// Direct database URL (for development only)
    pub database_url: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct BiscuitConfig {
    #[allow(dead_code)]
    pub private_key_env: Option<String>,
    pub public_key_env: Option<String>,
    #[allow(dead_code)]
    pub private_key_file: Option<PathBuf>,
    pub public_key_file: Option<PathBuf>,
}

#[derive(Debug, Deserialize, Default)]
pub struct AuditConfigFile {
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default = "default_directory")]
    pub directory: String,
    #[serde(default)]
    pub stdout: bool,
    #[serde(default = "default_retention_days")]
    pub retention_days: u32,
    #[serde(default = "default_enabled")]
    pub log_queries: bool,
    #[serde(default)]
    pub log_results: bool,
}

/// Approvals configuration from config file.
#[derive(Debug, Deserialize)]
pub struct ApprovalsConfigFile {
    /// Whether approvals are enabled.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// Directory for approval storage files.
    #[serde(default = "default_approvals_directory")]
    pub directory: String,
    /// Default TTL for approval requests in hours.
    #[serde(default = "default_ttl_hours")]
    pub ttl_hours: u64,
}

impl Default for ApprovalsConfigFile {
    fn default() -> Self {
        Self {
            enabled: true,
            directory: default_approvals_directory(),
            ttl_hours: default_ttl_hours(),
        }
    }
}

fn default_approvals_directory() -> String {
    "approvals".to_string()
}

fn default_ttl_hours() -> u64 {
    24
}

/// MCP server configuration from config file.
#[derive(Debug, Deserialize)]
pub struct McpConfigFile {
    /// Whether the MCP server is enabled.
    #[serde(default = "default_enabled")]
    pub enabled: bool,

    /// Transport type: "stdio" or "http".
    #[serde(default)]
    pub transport: Option<String>,

    /// HTTP port (only used when transport is HTTP).
    #[serde(default = "default_mcp_port")]
    pub http_port: u16,
}

impl Default for McpConfigFile {
    fn default() -> Self {
        Self {
            enabled: true,
            transport: Some("http".to_string()),
            http_port: default_mcp_port(),
        }
    }
}

/// Dashboard configuration from config file.
#[derive(Debug, Deserialize)]
pub struct DashboardConfigFile {
    /// Whether the dashboard is enabled.
    #[serde(default = "default_enabled")]
    pub enabled: bool,

    /// Port to listen on.
    #[serde(default = "default_dashboard_port")]
    pub listen_port: u16,
}

impl Default for DashboardConfigFile {
    fn default() -> Self {
        Self {
            enabled: true,
            listen_port: default_dashboard_port(),
        }
    }
}

// NOTE: Role definitions use cori_core::RoleDefinition directly.
// No duplicate config types here - follow the canonical model in cori-core.

fn default_upstream_port() -> u16 {
    5432
}

fn default_enabled() -> bool {
    true
}

fn default_directory() -> String {
    "logs/".to_string()
}

fn default_retention_days() -> u32 {
    90
}

fn default_mcp_port() -> u16 {
    3000
}

fn default_dashboard_port() -> u16 {
    8080
}

/// Start Cori with the specified options.
///
/// # Arguments
/// * `config_path` - Path to configuration file (default: cori.yaml)
/// * `http` - Use HTTP transport for MCP (override config)
/// * `stdio` - Use stdio transport for MCP (override config, requires token)
/// * `token` - Token file for stdio mode (or use CORI_TOKEN env var)
/// * `mcp_port` - Override MCP HTTP port
/// * `dashboard_port` - Override dashboard port
/// * `no_dashboard` - Disable dashboard
#[allow(clippy::too_many_arguments)]
pub async fn run(
    config_path: PathBuf,
    http: bool,
    stdio: bool,
    token: Option<PathBuf>,
    mcp_port: Option<u16>,
    dashboard_port: Option<u16>,
    no_dashboard: bool,
) -> Result<()> {
    // Load configuration first (needed to determine default transport)
    let config_str = fs::read_to_string(&config_path)
        .with_context(|| format!("Failed to read config file: {}", config_path.display()))?;

    // Support both YAML and TOML
    let run_config: RunConfig = if config_path
        .extension()
        .map(|e| e == "toml")
        .unwrap_or(false)
    {
        toml::from_str(&config_str)?
    } else {
        serde_yaml::from_str(&config_str)?
    };

    // Determine transport mode:
    // 1. CLI flags (--http or --stdio) override config if explicitly provided
    // 2. Otherwise, use cori.yaml mcp.transport setting
    // 3. Default to "http" if nothing is specified
    let (use_stdio, transport_source) = if stdio {
        // --stdio CLI flag explicitly passed → use stdio
        (true, "CLI flag --stdio")
    } else if http {
        // --http CLI flag explicitly passed → use HTTP
        (false, "CLI flag --http")
    } else {
        // Neither CLI flag passed → use config file setting
        let config_transport = run_config.mcp.transport.as_deref().unwrap_or("http");
        (config_transport == "stdio", "config file (mcp.transport)")
    };

    info!(
        transport = if use_stdio { "stdio" } else { "http" },
        source = transport_source,
        "Transport mode determined"
    );

    // For stdio mode, we need a token
    if use_stdio {
        let has_token = token.is_some() || std::env::var("CORI_TOKEN").is_ok();
        if !has_token {
            anyhow::bail!(
                "Stdio mode requires a token. Provide one via:\n\
                 • --token <file>          Token file path\n\
                 • CORI_TOKEN env var      Base64-encoded token"
            );
        }
    }

    info!(config = %config_path.display(), "Loading configuration");

    // Get the directory containing the config file for resolving relative paths
    let config_dir = config_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));

    // Build database URL
    let database_url = build_database_url(&run_config.upstream)?;

    // Load roles from CoriConfig using the same approach as CLI tools command
    // This ensures consistent role definition parsing with creatable/updatable
    let cori_config = cori_core::config::CoriConfig::load_with_context(&config_path)
        .with_context(|| format!("Failed to load CoriConfig from {:?}", config_path))?;

    // Get roles directly from CoriConfig (already properly parsed as RoleDefinition)
    let core_roles: HashMap<String, RoleDefinition> = cori_config.roles().clone();
    for (role_name, role) in &core_roles {
        info!(
            role = %role_name,
            tables = role.tables.len(),
            "Role loaded"
        );
    }

    // Resolve Biscuit public key for MCP server authentication
    let mcp_public_key: Option<PublicKey> = match resolve_public_key(
        &run_config.biscuit,
        &config_dir,
    ) {
        Ok(pk) => {
            info!("Loaded Biscuit public key for MCP authentication");
            Some(pk)
        }
        Err(e) => {
            if use_stdio {
                // Stdio mode requires public key for token verification
                anyhow::bail!(
                    "Stdio mode requires a public key for token verification: {}",
                    e
                );
            }
            warn!(
                error = %e,
                "No Biscuit public key configured - MCP HTTP server will run WITHOUT authentication"
            );
            None
        }
    };

    // Load rules configuration from schema/rules.yaml
    let rules = load_rules(&config_dir)?;

    // Build audit logger if enabled
    let audit_logger = if run_config.audit.enabled {
        // Resolve audit directory relative to config_dir if it's a relative path
        let audit_dir = {
            let dir_path = PathBuf::from(&run_config.audit.directory);
            if dir_path.is_absolute() {
                run_config.audit.directory.clone()
            } else {
                config_dir
                    .join(&run_config.audit.directory)
                    .to_string_lossy()
                    .to_string()
            }
        };

        let audit_config = AuditConfig {
            enabled: run_config.audit.enabled,
            directory: audit_dir.clone(),
            stdout: run_config.audit.stdout,
            retention_days: run_config.audit.retention_days,
            log_queries: run_config.audit.log_queries,
            log_results: run_config.audit.log_results,
            ..Default::default()
        };

        tracing::info!(
            enabled = audit_config.enabled,
            stdout = audit_config.stdout,
            directory = %audit_config.directory,
            "Creating audit logger"
        );

        Some(Arc::new(AuditLogger::new(audit_config)?))
    } else {
        None
    };

    // Create shared approval manager for MCP and dashboard
    let approval_manager = if run_config.approvals.enabled {
        // Resolve approvals directory relative to config directory
        let approvals_dir = config_dir.join(&run_config.approvals.directory);
        let ttl = chrono::Duration::hours(run_config.approvals.ttl_hours as i64);

        match ApprovalManager::with_file_storage(&approvals_dir, ttl) {
            Ok(manager) => {
                tracing::info!(
                    directory = %approvals_dir.display(),
                    ttl_hours = run_config.approvals.ttl_hours,
                    "Created file-based approval manager"
                );
                Arc::new(manager)
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "Failed to create file-based approval manager, falling back to in-memory"
                );
                Arc::new(ApprovalManager::default())
            }
        }
    } else {
        Arc::new(ApprovalManager::default())
    };

    // Load schema for MCP tool generation using CoriConfig (required)
    // This ensures we use the same from_schema_definition path as CLI tools command
    let schema = {
        let schema_def = cori_config.get_schema().ok_or_else(|| {
            anyhow::anyhow!("No schema found. Run 'cori db sync' to generate schema/schema.yaml")
        })?;
        cori_mcp::schema::from_schema_definition(schema_def)
    };
    info!("Loaded schema from schema/schema.yaml");

    // Determine effective ports
    let effective_mcp_port = mcp_port.unwrap_or(run_config.mcp.http_port);
    let effective_dashboard_port = dashboard_port.unwrap_or(run_config.dashboard.listen_port);
    let dashboard_enabled = !no_dashboard && run_config.dashboard.enabled;

    info!(
        mcp_transport = if use_stdio { "stdio" } else { "http" },
        mcp_port = effective_mcp_port,
        dashboard_enabled = dashboard_enabled,
        dashboard_port = effective_dashboard_port,
        roles_loaded = core_roles.len(),
        "Starting Cori"
    );

    if use_stdio {
        // Stdio mode: single-tenant with baked-in token
        run_stdio_mode(
            &config_path,
            &config_dir,
            token,
            mcp_public_key.unwrap(),
            &database_url,
            rules,
            schema,
            approval_manager.clone(),
            run_config,
            core_roles,
            audit_logger,
            dashboard_enabled,
            effective_dashboard_port,
        )
        .await
    } else {
        // HTTP mode: multi-tenant with token per request
        run_http_mode(
            &config_dir,
            mcp_public_key,
            &database_url,
            rules,
            schema,
            approval_manager.clone(),
            run_config,
            core_roles,
            audit_logger,
            effective_mcp_port,
            dashboard_enabled,
            effective_dashboard_port,
        )
        .await
    }
}

/// Run in HTTP mode (multi-tenant, token per request).
#[allow(clippy::too_many_arguments)]
async fn run_http_mode(
    config_dir: &Path,
    mcp_public_key: Option<PublicKey>,
    database_url: &str,
    rules: Option<RulesDefinition>,
    schema: cori_mcp::schema::DatabaseSchema,
    approval_manager: Arc<ApprovalManager>,
    run_config: RunConfig,
    core_roles: HashMap<String, RoleDefinition>,
    audit_logger: Option<Arc<AuditLogger>>,
    mcp_port: u16,
    dashboard_enabled: bool,
    dashboard_port: u16,
) -> Result<()> {
    let mut handles = Vec::new();

    // ============================================
    // START ALL SERVICES CONCURRENTLY
    // ============================================

    // 1. Start MCP Server (if enabled)
    // Note: Transport mode was already determined by CLI flags or config,
    // so we're guaranteed to be in HTTP mode here.
    if run_config.mcp.enabled {
        let mcp_config = McpConfig {
            enabled: true,
            transport: Transport::Http,
            host: "127.0.0.1".to_string(),
            port: mcp_port,
        };

        let database_url = database_url.to_string();
        let rules_for_mcp = rules.clone();
        let schema = schema.clone();
        let core_roles = core_roles.clone();
        let approval_manager = approval_manager.clone();
        let public_key = mcp_public_key.clone();
        let audit_logger_for_mcp = audit_logger.clone();

        let handle = tokio::spawn(async move {
            tracing::info!(
                port = mcp_config.port,
                auth_enabled = public_key.is_some(),
                audit_enabled = audit_logger_for_mcp.is_some(),
                "Starting MCP HTTP server"
            );

            // Connect to database
            match PgPoolOptions::new()
                .max_connections(5)
                .connect(&database_url)
                .await
            {
                Ok(pool) => {
                    // Create MCP server
                    let mut server = McpServer::new(mcp_config)
                        .with_pool(pool)
                        .with_approval_manager(approval_manager);

                    // Add audit logger if enabled
                    if let Some(logger) = audit_logger_for_mcp {
                        server = server.with_audit_logger(logger);
                    }

                    // Add rules if loaded
                    if let Some(r) = rules_for_mcp {
                        server = server.with_rules(r);
                    }

                    // Add public key for authentication if available
                    if let Some(pk) = public_key {
                        server = server.with_public_key(pk);
                    }

                    // Add schema (must be before with_roles for proper tool generation)
                    server = server.with_schema(schema);

                    // Pre-generate tools for all roles at startup (HTTP mode)
                    // Each request will use the pre-generated tools for its token's role
                    if !core_roles.is_empty() {
                        server = server.with_roles(core_roles);
                    }

                    // Use run_http directly
                    if let Err(e) = server.run_http().await {
                        tracing::error!(error = %e, "MCP server error");
                    }
                }
                Err(e) => {
                    tracing::error!(error = %e, "Failed to connect to database for MCP server");
                }
            }
        });
        handles.push(handle);
    }

    // Start Dashboard (if enabled)
    if dashboard_enabled {
        let dashboard_config = DashboardConfig {
            enabled: true,
            host: "127.0.0.1".to_string(),
            port: dashboard_port,
            ..Default::default()
        };

        // Build CoriConfig for dashboard state
        let cori_config = build_cori_config(&run_config, config_dir, &rules, &core_roles)?;

        let audit_logger_for_dashboard = audit_logger
            .clone()
            .unwrap_or_else(|| Arc::new(AuditLogger::new(AuditConfig::default()).unwrap()));

        // Load keypair for dashboard (for token minting)
        let keypair = resolve_keypair(&run_config.biscuit, config_dir)?;

        let dashboard = DashboardServer::with_state(
            dashboard_config,
            cori_config,
            keypair,
            audit_logger_for_dashboard,
            approval_manager.clone(),
        );

        let handle = tokio::spawn(async move {
            info!(port = dashboard.listen_port(), "Starting admin dashboard");
            if let Err(e) = dashboard.run().await {
                tracing::error!(error = %e, "Dashboard error");
            }
        });
        handles.push(handle);
    }

    // Wait for any service to finish (they should run indefinitely)
    for handle in handles {
        if let Err(e) = handle.await {
            tracing::error!(error = %e, "Service task failed");
        }
    }

    Ok(())
}

/// Run in stdio mode (single-tenant with baked-in token).
#[allow(clippy::too_many_arguments)]
async fn run_stdio_mode(
    config_path: &Path,
    config_dir: &Path,
    token_file: Option<PathBuf>,
    public_key: PublicKey,
    database_url: &str,
    rules: Option<RulesDefinition>,
    schema: cori_mcp::schema::DatabaseSchema,
    approval_manager: Arc<ApprovalManager>,
    run_config: RunConfig,
    core_roles: HashMap<String, RoleDefinition>,
    audit_logger: Option<Arc<AuditLogger>>,
    dashboard_enabled: bool,
    dashboard_port: u16,
) -> Result<()> {
    // Load token from file or environment
    let token = load_token(token_file)?;

    // Verify token and extract role/tenant
    let verifier = TokenVerifier::new(public_key);
    let verified = verifier
        .verify(&token)
        .context("Token verification failed")?;

    info!(role = %verified.role, tenant = ?verified.tenant, "Token verified");

    let role_name = verified.role.clone();
    let tenant_id = verified.tenant.clone();

    // Load role configuration from token's role claim
    let roles_dir = config_dir.join("roles");
    let role_config = if let Some(config) = core_roles.get(&role_name) {
        config.clone()
    } else {
        // Try to find role file based on token's role
        let role_path = roles_dir.join(format!("{}.yaml", role_name));
        if role_path.exists() {
            RoleDefinition::from_file(&role_path)
                .with_context(|| format!("Failed to load role config: {:?}", role_path))?
        } else {
            warn!(role = %role_name, path = %role_path.display(), "No role configuration file found for role from token");
            // Create minimal role config
            RoleDefinition {
                name: role_name.clone(),
                description: None,
                approvals: None,
                tables: HashMap::new(),
            }
        }
    };

    // Start dashboard in background if enabled
    let dashboard_handle = if dashboard_enabled {
        let dashboard_config = DashboardConfig {
            enabled: true,
            host: "127.0.0.1".to_string(),
            port: dashboard_port,
            ..Default::default()
        };

        let cori_config = build_cori_config(&run_config, config_dir, &rules, &core_roles)?;
        let audit_logger_for_dashboard = audit_logger
            .unwrap_or_else(|| Arc::new(AuditLogger::new(AuditConfig::default()).unwrap()));
        let keypair = resolve_keypair(&run_config.biscuit, config_dir)?;

        let dashboard = DashboardServer::with_state(
            dashboard_config,
            cori_config,
            keypair,
            audit_logger_for_dashboard,
            approval_manager.clone(),
        );

        Some(tokio::spawn(async move {
            info!(port = dashboard.listen_port(), "Starting admin dashboard");
            if let Err(e) = dashboard.run().await {
                tracing::error!(error = %e, "Dashboard error");
            }
        }))
    } else {
        None
    };

    // Connect to database
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(database_url)
        .await
        .context("Failed to connect to database")?;

    info!("Connected to upstream database");

    // Create and configure MCP server for stdio
    let mcp_config = McpConfig {
        enabled: true,
        transport: Transport::Stdio,
        port: 0, // Not used for stdio
        ..Default::default()
    };

    let mut server = McpServer::new(mcp_config)
        .with_pool(pool)
        .with_approval_manager(approval_manager);

    // Add rules if loaded
    if let Some(r) = rules {
        server = server.with_rules(r);
    }

    // Add schema (required for tool generation)
    server = server.with_schema(schema);

    // Add role (must be after schema for proper executor setup)
    server = server.with_role(role_config);

    if let Some(tid) = tenant_id {
        server = server.with_tenant_id(tid);
    }

    // Generate tools from role config
    server.generate_tools();

    info!(
        tool_count = server.tools_mut().len(),
        config = %config_path.display(),
        "MCP stdio server starting"
    );

    // Run the MCP stdio server (blocks until input ends)
    server.run().await?;

    // If dashboard was running, wait for it
    if let Some(handle) = dashboard_handle {
        handle.abort();
    }

    Ok(())
}

/// Load token from file or CORI_TOKEN environment variable.
fn load_token(token_file: Option<PathBuf>) -> Result<String> {
    if let Some(path) = token_file {
        fs::read_to_string(&path)
            .map(|s| s.trim().to_string())
            .with_context(|| format!("Failed to read token file: {:?}", path))
    } else if let Ok(token_env) = std::env::var("CORI_TOKEN") {
        // Decode from base64 if needed
        if token_env.contains('.') {
            // Already base64 biscuit format
            Ok(token_env)
        } else {
            // Try to decode as base64
            String::from_utf8(
                base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &token_env)
                    .unwrap_or_else(|_| token_env.as_bytes().to_vec()),
            )
            .map_err(|_| anyhow::anyhow!("Invalid UTF-8 in CORI_TOKEN"))
        }
    } else {
        anyhow::bail!("No token provided. Use --token <file> or set CORI_TOKEN env var")
    }
}

/// Build database URL from upstream config.
fn build_database_url(upstream: &UpstreamConfigFile) -> Result<String> {
    // First check for database_url_env
    if let Some(env_var) = &upstream.database_url_env {
        if let Ok(url) = std::env::var(env_var) {
            return Ok(url);
        }
    }

    // Check for direct database_url
    if let Some(url) = &upstream.database_url {
        return Ok(url.clone());
    }

    // Also check DATABASE_URL directly
    if let Ok(url) = std::env::var("DATABASE_URL") {
        return Ok(url);
    }

    // Build from individual components
    let host = upstream.host.as_deref().unwrap_or("localhost");
    let port = upstream.port;
    let database = upstream.database.as_deref().ok_or_else(|| {
        anyhow::anyhow!("Database name required in upstream config or DATABASE_URL env var")
    })?;
    let username = upstream.username.as_deref().unwrap_or("postgres");
    let password = upstream.password.as_deref().unwrap_or("");

    Ok(format!(
        "postgres://{}:{}@{}:{}/{}",
        username, password, host, port, database
    ))
}

/// Load rules configuration from schema/rules.yaml.
fn load_rules(config_dir: &Path) -> Result<Option<RulesDefinition>> {
    let rules_path = config_dir.join("schema/rules.yaml");
    if rules_path.exists() {
        match RulesDefinition::from_file(&rules_path) {
            Ok(rules) => {
                info!(
                    tables = rules.tables.len(),
                    path = %rules_path.display(),
                    "Loaded rules configuration"
                );
                Ok(Some(rules))
            }
            Err(e) => {
                warn!("Failed to load rules file: {}", e);
                Ok(None)
            }
        }
    } else {
        info!("No rules file found at {}", rules_path.display());
        Ok(None)
    }
}

/// Resolve public key from config.
fn resolve_public_key(config: &BiscuitConfig, config_dir: &Path) -> Result<PublicKey> {
    // Try public key file first
    if let Some(public_key_file) = &config.public_key_file {
        let path = if public_key_file.is_absolute() {
            public_key_file.clone()
        } else {
            config_dir.join(public_key_file)
        };
        return load_public_key_file(&path)
            .with_context(|| format!("Failed to load public key from {:?}", path));
    }

    // Try environment variable
    if let Some(env_var) = &config.public_key_env {
        if let Ok(hex) = std::env::var(env_var) {
            return cori_biscuit::keys::load_public_key_hex(&hex)
                .context("Failed to parse public key from environment variable");
        }
    }

    anyhow::bail!(
        "No Biscuit public key configured. Set biscuit.public_key_file or biscuit.public_key_env"
    )
}

/// Resolve keypair from config for the dashboard.
fn resolve_keypair(
    config: &BiscuitConfig,
    config_dir: &Path,
) -> Result<cori_biscuit::keys::KeyPair> {
    // Try private key file first
    if let Some(private_key_file) = &config.private_key_file {
        let path = if private_key_file.is_absolute() {
            private_key_file.clone()
        } else {
            config_dir.join(private_key_file)
        };
        return cori_biscuit::keys::KeyPair::load_from_file(&path)
            .with_context(|| format!("Failed to load private key from {:?}", path));
    }

    // Try environment variable
    if let Some(env_var) = &config.private_key_env {
        if let Ok(hex) = std::env::var(env_var) {
            return cori_biscuit::keys::KeyPair::from_private_key_hex(&hex)
                .context("Failed to parse private key from environment variable");
        }
    }

    // Try default path
    let default_path = config_dir.join("keys/private.key");
    if default_path.exists() {
        return cori_biscuit::keys::KeyPair::load_from_file(&default_path)
            .with_context(|| format!("Failed to load private key from {:?}", default_path));
    }

    anyhow::bail!(
        "No Biscuit private key found. Set biscuit.private_key_file or place key in keys/private.key"
    )
}

/// Build a CoriConfig for the dashboard state.
fn build_cori_config(
    run_config: &RunConfig,
    _config_dir: &Path,
    rules: &Option<RulesDefinition>,
    roles: &HashMap<String, RoleDefinition>,
) -> Result<cori_core::config::CoriConfig> {
    use cori_core::config::CoriConfig;

    let upstream = UpstreamConfig {
        host: run_config
            .upstream
            .host
            .clone()
            .unwrap_or_else(|| "localhost".to_string()),
        port: run_config.upstream.port,
        database: run_config
            .upstream
            .database
            .clone()
            .unwrap_or_else(|| "postgres".to_string()),
        username: run_config
            .upstream
            .username
            .clone()
            .unwrap_or_else(|| "postgres".to_string()),
        password: run_config.upstream.password.clone(),
        database_url_env: run_config.upstream.database_url_env.clone(),
        database_url: run_config.upstream.database_url.clone(),
        ..Default::default()
    };

    let mcp = McpConfig {
        enabled: run_config.mcp.enabled,
        transport: match run_config.mcp.transport.as_deref() {
            Some("http") => Transport::Http,
            _ => Transport::Stdio,
        },
        host: "127.0.0.1".to_string(),
        port: run_config.mcp.http_port,
    };

    let dashboard = DashboardConfig {
        enabled: run_config.dashboard.enabled,
        host: "127.0.0.1".to_string(),
        port: run_config.dashboard.listen_port,
        ..Default::default()
    };

    let audit = AuditConfig {
        enabled: run_config.audit.enabled,
        log_queries: run_config.audit.log_queries,
        log_results: run_config.audit.log_results,
        ..Default::default()
    };

    Ok(CoriConfig {
        project: None,
        version: None,
        upstream,
        biscuit: cori_core::config::BiscuitConfig {
            public_key_env: run_config.biscuit.public_key_env.clone(),
            public_key_file: run_config.biscuit.public_key_file.clone(),
            private_key_env: run_config.biscuit.private_key_env.clone(),
            private_key_file: run_config.biscuit.private_key_file.clone(),
            ..Default::default()
        },
        mcp,
        dashboard,
        audit,
        rules: rules.clone(),
        virtual_schema: Default::default(),
        guardrails: Default::default(),
        observability: Default::default(),
        roles: roles.clone(),
        ..Default::default()
    })
}
