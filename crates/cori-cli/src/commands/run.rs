//! Run command for starting Cori.
//!
//! `cori run` - Start the MCP server and dashboard (HTTP mode by default).
//! `cori run --stdio` - Start in stdio mode with a baked-in token.
//!
//! The dashboard always runs by default (use --no-dashboard to disable).

use anyhow::{Context, Result};
use cori_audit::AuditLogger;
use cori_biscuit::{keys::load_public_key_file, PublicKey, TokenVerifier};
use cori_core::config::role_definition::RoleDefinition;
use cori_core::config::rules_definition::RulesDefinition;
use cori_core::config::{
    AuditConfig, DashboardConfig, McpConfig, Transport, UpstreamConfig,
};
use cori_dashboard::DashboardServer;
use cori_mcp::approval::ApprovalManager;
use cori_mcp::McpServer;
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

    /// MCP server configuration.
    #[serde(default)]
    pub mcp: McpConfigFile,

    /// Dashboard configuration.
    #[serde(default)]
    pub dashboard: DashboardConfigFile,

    /// Directory containing role definition files (each .yaml file = one role).
    #[serde(default)]
    pub roles_dir: Option<PathBuf>,

    /// List of individual role definition files.
    #[serde(default)]
    pub role_files: Vec<PathBuf>,

    /// Inline role definitions (can be combined with roles_dir/role_files).
    #[serde(default)]
    pub roles: HashMap<String, RoleConfigFile>,
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

/// Role configuration loaded from a file or inline.
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct RoleConfigFile {
    /// Role name (optional in inline definitions, required in file definitions).
    #[serde(default)]
    pub name: Option<String>,

    /// Description of the role.
    #[serde(default)]
    pub description: Option<String>,

    /// Table access configuration.
    #[serde(default)]
    pub tables: HashMap<String, TableConfigFile>,

    /// Blocked SQL operations.
    #[serde(default)]
    pub blocked_operations: Vec<String>,
}

/// Table access configuration within a role.
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct TableConfigFile {
    /// Columns that can be read. Can be a list of column names or "*" for all.
    #[serde(default)]
    pub readable: ReadableColumnsConfig,

    /// Columns that can be edited, with optional constraints.
    #[serde(default)]
    pub editable: EditableColumnsConfig,

    /// Tenant column for this table (overrides default).
    #[serde(default)]
    pub tenant_column: Option<String>,
}

/// Configuration for readable columns - can be a list or "*" for all.
#[derive(Debug, Clone, Default)]
#[allow(dead_code)]
pub enum ReadableColumnsConfig {
    #[default]
    None,
    All,
    List(Vec<String>),
}

impl<'de> serde::Deserialize<'de> for ReadableColumnsConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::{self, Visitor};

        struct ReadableColumnsVisitor;

        impl<'de> Visitor<'de> for ReadableColumnsVisitor {
            type Value = ReadableColumnsConfig;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str(r#"a list of column names or "*""#)
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                if v == "*" {
                    Ok(ReadableColumnsConfig::All)
                } else {
                    Err(de::Error::custom(format!(
                        r#"expected "*" or a list, got "{}""#,
                        v
                    )))
                }
            }

            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: de::SeqAccess<'de>,
            {
                let mut columns = Vec::new();
                while let Some(col) = seq.next_element::<String>()? {
                    columns.push(col);
                }
                Ok(ReadableColumnsConfig::List(columns))
            }
        }

        deserializer.deserialize_any(ReadableColumnsVisitor)
    }
}

/// Configuration for editable columns - can be a map, empty list, or "*" for all.
#[derive(Debug, Clone, Default)]
#[allow(dead_code)]
pub enum EditableColumnsConfig {
    #[default]
    None,
    All,
    Map(HashMap<String, ColumnConstraintsConfig>),
}

impl<'de> serde::Deserialize<'de> for EditableColumnsConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::{self, Visitor};

        struct EditableColumnsVisitor;

        impl<'de> Visitor<'de> for EditableColumnsVisitor {
            type Value = EditableColumnsConfig;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str(r#"a map of column constraints, an empty list, or "*""#)
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                if v == "*" {
                    Ok(EditableColumnsConfig::All)
                } else {
                    Err(de::Error::custom(format!(
                        r#"expected "*" or a map, got "{}""#,
                        v
                    )))
                }
            }

            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: de::SeqAccess<'de>,
            {
                // Empty list means no editable columns
                let _: Option<serde::de::IgnoredAny> = seq.next_element()?;
                Ok(EditableColumnsConfig::None)
            }

            fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
            where
                M: de::MapAccess<'de>,
            {
                let mut constraints = HashMap::new();
                while let Some((key, value)) =
                    map.next_entry::<String, ColumnConstraintsConfig>()?
                {
                    constraints.insert(key, value);
                }
                Ok(EditableColumnsConfig::Map(constraints))
            }
        }

        deserializer.deserialize_any(EditableColumnsVisitor)
    }
}

/// Constraints on an editable column.
#[derive(Debug, Clone, Deserialize, Default)]
#[allow(dead_code)]
pub struct ColumnConstraintsConfig {
    /// Allowed values (whitelist).
    #[serde(default)]
    pub allowed_values: Option<Vec<String>>,

    /// Regex pattern the value must match.
    #[serde(default)]
    pub pattern: Option<String>,

    /// Minimum value (for numeric columns).
    #[serde(default)]
    pub min: Option<f64>,

    /// Maximum value (for numeric columns).
    #[serde(default)]
    pub max: Option<f64>,

    /// Whether changes require human approval.
    #[serde(default)]
    pub requires_approval: bool,
}

/// Loaded role with resolved name.
#[derive(Debug, Clone)]
pub struct LoadedRole {
    /// Role name.
    pub name: String,
    /// Role configuration.
    pub config: RoleConfigFile,
}

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
/// * `http` - Use HTTP transport for MCP (default for multi-tenant)
/// * `stdio` - Use stdio transport for MCP (requires token)
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
    // Determine transport mode
    // If both --http and --stdio are specified, or neither, default to HTTP
    let use_stdio = stdio && !http;

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

    // Load configuration
    let config_str = fs::read_to_string(&config_path)
        .with_context(|| format!("Failed to read config file: {}", config_path.display()))?;

    // Support both YAML and TOML
    let run_config: RunConfig =
        if config_path.extension().map(|e| e == "toml").unwrap_or(false) {
            toml::from_str(&config_str)?
        } else {
            serde_yaml::from_str(&config_str)?
        };

    info!(config = %config_path.display(), "Loading configuration");

    // Get the directory containing the config file for resolving relative paths
    let config_dir = config_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));

    // Build database URL
    let database_url = build_database_url(&run_config.upstream)?;

    // Load roles from configuration
    let roles = load_roles(&run_config, &config_dir)?;
    for role in &roles {
        info!(
            role = %role.name,
            tables = role.config.tables.len(),
            "Role loaded"
        );
    }

    // Resolve Biscuit public key for MCP server authentication
    let mcp_public_key: Option<PublicKey> = match resolve_public_key(&run_config.biscuit, &config_dir) {
        Ok(pk) => {
            info!("Loaded Biscuit public key for MCP authentication");
            Some(pk)
        }
        Err(e) => {
            if use_stdio {
                // Stdio mode requires public key for token verification
                anyhow::bail!("Stdio mode requires a public key for token verification: {}", e);
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
        let audit_config = AuditConfig {
            enabled: run_config.audit.enabled,
            directory: run_config.audit.directory.clone(),
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
    let approval_manager = Arc::new(ApprovalManager::default());

    // Convert loaded roles to cori_core RoleDefinition for MCP
    let core_roles: HashMap<String, RoleDefinition> = roles
        .iter()
        .map(|r| (r.name.clone(), convert_to_role_definition(r)))
        .collect();

    // Load schema for MCP tool generation
    let schema_path = config_dir.join("schema/snapshot.json");
    let schema = load_schema(&schema_path);

    // Determine effective ports
    let effective_mcp_port = mcp_port.unwrap_or(run_config.mcp.http_port);
    let effective_dashboard_port = dashboard_port.unwrap_or(run_config.dashboard.listen_port);
    let dashboard_enabled = !no_dashboard && run_config.dashboard.enabled;

    info!(
        mcp_transport = if use_stdio { "stdio" } else { "http" },
        mcp_port = effective_mcp_port,
        dashboard_enabled = dashboard_enabled,
        dashboard_port = effective_dashboard_port,
        roles_loaded = roles.len(),
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
    schema: Option<cori_mcp::schema::DatabaseSchema>,
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

    // 1. Start MCP Server (if enabled and transport is HTTP)
    if run_config.mcp.enabled {
        let transport = run_config.mcp.transport.as_deref().unwrap_or("http");

        if transport == "http" {
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

                        if let Some(s) = schema {
                            server = server.with_schema(s);
                        }

                        // If there's a default role, use it for tool generation
                        if let Some((_name, role_config)) = core_roles.iter().next() {
                            server = server.with_role(role_config.clone());
                            server.generate_tools();
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
        } else {
            tracing::info!(
                transport = transport,
                "MCP server configured for stdio transport - not starting in serve mode"
            );
        }
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
            info!(
                port = dashboard.listen_port(),
                "Starting admin dashboard"
            );
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
    schema: Option<cori_mcp::schema::DatabaseSchema>,
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
    let verified = verifier.verify(&token)
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

    // Add schema if loaded
    if let Some(s) = schema {
        server = server.with_schema(s);
    }

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

/// Load schema from snapshot file if available.
fn load_schema(schema_path: &Path) -> Option<cori_mcp::schema::DatabaseSchema> {
    if !schema_path.exists() {
        info!("No schema file found at {}", schema_path.display());
        return None;
    }

    match fs::read_to_string(schema_path) {
        Ok(content) => match serde_json::from_str::<serde_json::Value>(&content) {
            Ok(json) => match cori_mcp::schema::parse_schema_from_json(&json) {
                Ok(s) => {
                    info!("Loaded schema from {}", schema_path.display());
                    Some(s)
                }
                Err(e) => {
                    warn!("Failed to parse schema: {:?}", e);
                    None
                }
            },
            Err(e) => {
                warn!("Failed to parse schema file as JSON: {}", e);
                None
            }
        },
        Err(e) => {
            warn!("Failed to read schema file: {}", e);
            None
        }
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
    let database = upstream.database.as_deref()
        .ok_or_else(|| anyhow::anyhow!("Database name required in upstream config or DATABASE_URL env var"))?;
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

/// Load roles from configuration (directory, individual files, or inline).
fn load_roles(config: &RunConfig, config_dir: &Path) -> Result<Vec<LoadedRole>> {
    let mut roles = Vec::new();

    // Load from roles directory if specified or default
    let roles_dir = config.roles_dir.as_ref()
        .map(|p| if p.is_absolute() { p.clone() } else { config_dir.join(p) })
        .unwrap_or_else(|| config_dir.join("roles"));

    if roles_dir.exists() && roles_dir.is_dir() {
        for entry in fs::read_dir(&roles_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().map(|e| e == "yaml" || e == "yml").unwrap_or(false) {
                match load_role_from_file(&path) {
                    Ok(role) => roles.push(role),
                    Err(e) => warn!(path = %path.display(), error = %e, "Failed to load role"),
                }
            }
        }
    }

    // Load from individual role files
    for path in &config.role_files {
        let full_path = if path.is_absolute() {
            path.clone()
        } else {
            config_dir.join(path)
        };
        match load_role_from_file(&full_path) {
            Ok(role) => roles.push(role),
            Err(e) => warn!(path = %full_path.display(), error = %e, "Failed to load role"),
        }
    }

    // Load inline roles
    for (name, config) in &config.roles {
        roles.push(LoadedRole {
            name: name.clone(),
            config: config.clone(),
        });
    }

    Ok(roles)
}

/// Load a single role from a YAML file.
fn load_role_from_file(path: &Path) -> Result<LoadedRole> {
    let content = fs::read_to_string(path)?;
    let config: RoleConfigFile = serde_yaml::from_str(&content)?;
    
    // Derive role name from filename if not specified
    let name = config.name.clone().unwrap_or_else(|| {
        path.file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unnamed")
            .to_string()
    });

    Ok(LoadedRole { name, config })
}

/// Convert a LoadedRole to cori_core RoleDefinition.
fn convert_to_role_definition(loaded: &LoadedRole) -> RoleDefinition {
    use cori_core::config::role_definition::{
        AllColumns, CreatableColumns, CreatableColumnConstraints,
        DeletablePermission, ReadableConfig, TablePermissions, UpdatableColumns,
        UpdatableColumnConstraints, ApprovalRequirement,
    };

    let mut tables = HashMap::new();

    for (table_name, table_config) in &loaded.config.tables {
        let readable = match &table_config.readable {
            ReadableColumnsConfig::All => ReadableConfig::All(AllColumns),
            ReadableColumnsConfig::List(cols) => ReadableConfig::List(cols.clone()),
            ReadableColumnsConfig::None => ReadableConfig::List(Vec::new()),
        };

        // Convert editable to both creatable and updatable (old model didn't distinguish)
        let (creatable, updatable) = match &table_config.editable {
            EditableColumnsConfig::All => (
                CreatableColumns::All(AllColumns),
                UpdatableColumns::All(AllColumns),
            ),
            EditableColumnsConfig::Map(map) => {
                let creatable_map: HashMap<String, CreatableColumnConstraints> = map
                    .iter()
                    .map(|(col, constraints)| {
                        let restrict_to = constraints.allowed_values.as_ref().map(|vals| {
                            vals.iter().map(|v| serde_json::Value::String(v.clone())).collect()
                        });
                        (
                            col.clone(),
                            CreatableColumnConstraints {
                                required: false,
                                default: None,
                                restrict_to,
                                requires_approval: if constraints.requires_approval { 
                                    Some(ApprovalRequirement::Simple(true)) 
                                } else { 
                                    None 
                                },
                                guidance: None,
                            },
                        )
                    })
                    .collect();
                let updatable_map: HashMap<String, UpdatableColumnConstraints> = map
                    .iter()
                    .map(|(col, constraints)| {
                        // Convert allowed_values to only_when with new.<col>: [values] pattern
                        let only_when = constraints.allowed_values.as_ref().map(|vals| {
                            let key = format!("new.{}", col);
                            let values: Vec<serde_json::Value> = vals.iter()
                                .map(|v| serde_json::Value::String(v.clone()))
                                .collect();
                            let mut conditions = HashMap::new();
                            conditions.insert(key, cori_core::config::role_definition::ColumnCondition::In(values));
                            cori_core::config::role_definition::OnlyWhen::Single(conditions)
                        });
                        (
                            col.clone(),
                            UpdatableColumnConstraints {
                                only_when,
                                requires_approval: if constraints.requires_approval { 
                                    Some(ApprovalRequirement::Simple(true)) 
                                } else { 
                                    None 
                                },
                                guidance: None,
                            },
                        )
                    })
                    .collect();
                (
                    CreatableColumns::Map(creatable_map),
                    UpdatableColumns::Map(updatable_map),
                )
            }
            EditableColumnsConfig::None => (
                CreatableColumns::Map(HashMap::new()),
                UpdatableColumns::Map(HashMap::new()),
            ),
        };

        tables.insert(
            table_name.clone(),
            TablePermissions {
                readable,
                creatable,
                updatable,
                deletable: DeletablePermission::Allowed(false), // Default to no delete
            },
        );
    }

    RoleDefinition {
        name: loaded.name.clone(),
        description: loaded.config.description.clone(),
        approvals: None,
        tables,
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

    anyhow::bail!("No Biscuit public key configured. Set biscuit.public_key_file or biscuit.public_key_env")
}

/// Resolve keypair from config for the dashboard.
fn resolve_keypair(config: &BiscuitConfig, config_dir: &Path) -> Result<cori_biscuit::keys::KeyPair> {
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

    anyhow::bail!("No Biscuit private key found. Set biscuit.private_key_file or place key in keys/private.key")
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
