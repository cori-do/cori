//! Serve command for starting the Cori MCP server and dashboard.
//!
//! `cori serve` - Start the MCP server and admin dashboard.

use cori_audit::AuditLogger;
use cori_biscuit::PublicKey;
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

/// Configuration file structure for `cori serve`.
#[derive(Debug, Deserialize)]
pub struct ServeConfig {
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

    /// Tables that are explicitly blocked.
    #[serde(default)]
    pub blocked_tables: Vec<String>,

    /// Maximum rows per query.
    #[serde(default)]
    pub max_rows_per_query: Option<u64>,

    /// Maximum affected rows for mutations.
    #[serde(default)]
    pub max_affected_rows: Option<u64>,

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

fn default_mcp_port() -> u16 {
    3000
}

fn default_dashboard_port() -> u16 {
    8080
}

/// Start the Cori MCP server and dashboard.
///
/// This function starts all enabled services based on the configuration:
/// - MCP server (if mcp.enabled is true)
/// - Admin dashboard (if dashboard.enabled is true)
pub async fn serve(config_path: PathBuf) -> anyhow::Result<()> {
    // Load configuration
    let config_str = fs::read_to_string(&config_path)?;

    // Support both YAML and TOML
    let serve_config: ServeConfig =
        if config_path.extension().map(|e| e == "toml").unwrap_or(false) {
            toml::from_str(&config_str)?
        } else {
            serde_yaml::from_str(&config_str)?
        };

    tracing::info!(config = %config_path.display(), "Loading configuration");

    // Get the directory containing the config file for resolving relative paths
    let config_dir = config_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));

    // Load roles from configuration
    let roles = load_roles(&serve_config, &config_dir)?;
    for role in &roles {
        tracing::info!(
            role = %role.name,
            tables = role.config.tables.len(),
            blocked_tables = role.config.blocked_tables.len(),
            "Role loaded"
        );
    }

    // Resolve Biscuit public key for MCP server authentication
    let mcp_public_key: Option<PublicKey> = match resolve_public_key(&serve_config.biscuit) {
        Ok(hex) => {
            match cori_biscuit::keys::load_public_key_hex(&hex) {
                Ok(pk) => {
                    tracing::info!("Loaded Biscuit public key for MCP authentication");
                    Some(pk)
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "Failed to parse public key - MCP HTTP server will run WITHOUT authentication"
                    );
                    None
                }
            }
        }
        Err(e) => {
            tracing::warn!(
                error = %e,
                "No Biscuit public key configured - MCP HTTP server will run WITHOUT authentication"
            );
            None
        }
    };

    // Load rules configuration from schema/rules.yaml
    let rules = load_rules(&config_dir)?;

    // Build audit logger if enabled
    let audit_logger = if serve_config.audit.enabled {
        let audit_config = AuditConfig {
            enabled: serve_config.audit.enabled,
            log_queries: serve_config.audit.log_queries,
            log_results: serve_config.audit.log_results,
            ..Default::default()
        };
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

    // Log startup information
    tracing::info!(
        mcp_enabled = serve_config.mcp.enabled,
        mcp_port = serve_config.mcp.http_port,
        dashboard_enabled = serve_config.dashboard.enabled,
        dashboard_port = serve_config.dashboard.listen_port,
        roles_loaded = roles.len(),
        "Starting Cori services"
    );

    // Build database URL for MCP server
    let database_url = build_database_url(&serve_config.upstream)?;

    // Load schema for MCP tool generation
    let schema_path = config_dir.join("schema/snapshot.json");
    let schema = if schema_path.exists() {
        match std::fs::read_to_string(&schema_path) {
            Ok(content) => match serde_json::from_str::<serde_json::Value>(&content) {
                Ok(json) => {
                    match cori_mcp::schema::parse_schema_from_json(&json) {
                        Ok(s) => {
                            tracing::info!("Loaded schema from {}", schema_path.display());
                            Some(s)
                        }
                        Err(e) => {
                            tracing::warn!("Failed to parse schema: {:?}", e);
                            None
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!("Failed to parse schema file as JSON: {}", e);
                    None
                }
            },
            Err(e) => {
                tracing::warn!("Failed to read schema file: {}", e);
                None
            }
        }
    } else {
        tracing::info!("No schema file found at {}", schema_path.display());
        None
    };

    // ============================================
    // START ALL SERVICES CONCURRENTLY
    // ============================================

    let mut handles = Vec::new();

    // 1. Start MCP Server (if enabled and transport is HTTP)
    if serve_config.mcp.enabled {
        let transport = serve_config
            .mcp
            .transport
            .as_deref()
            .unwrap_or("http");

        if transport == "http" {
            let mcp_config = McpConfig {
                enabled: true,
                transport: Transport::Http,
                host: "127.0.0.1".to_string(),
                port: serve_config.mcp.http_port,
            };

            let database_url = database_url.clone();
            let rules_for_mcp = rules.clone();
            let schema = schema.clone();
            let core_roles = core_roles.clone();
            let approval_manager = approval_manager.clone();
            let public_key = mcp_public_key.clone();

            let handle = tokio::spawn(async move {
                tracing::info!(
                    port = mcp_config.port,
                    auth_enabled = public_key.is_some(),
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

    // 2. Start Dashboard (if enabled)
    if serve_config.dashboard.enabled {
        let dashboard_config = DashboardConfig {
            enabled: true,
            host: "127.0.0.1".to_string(),
            port: serve_config.dashboard.listen_port,
            ..Default::default()
        };

        // Build CoriConfig for dashboard state
        let cori_config = build_cori_config(&serve_config, &config_dir, &rules, &core_roles)?;

        let audit_logger_for_dashboard = audit_logger
            .clone()
            .unwrap_or_else(|| Arc::new(AuditLogger::new(AuditConfig::default()).unwrap()));

        // Load keypair for dashboard (for token minting)
        let keypair = resolve_keypair(&serve_config.biscuit, &config_dir)?;

        let dashboard = DashboardServer::with_state(
            dashboard_config,
            cori_config,
            keypair,
            audit_logger_for_dashboard,
            approval_manager.clone(),
        );

        let handle = tokio::spawn(async move {
            tracing::info!(
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

/// Build database URL from upstream config.
fn build_database_url(upstream: &UpstreamConfigFile) -> anyhow::Result<String> {
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

/// Convert a LoadedRole to cori_core RoleDefinition.
fn convert_to_role_definition(loaded: &LoadedRole) -> RoleDefinition {
    use cori_core::config::role_definition::{
        AllColumns, ColumnList, CreatableColumns, CreatableColumnConstraints,
        DeletablePermission, TablePermissions, UpdatableColumns,
        UpdatableColumnConstraints,
    };

    let mut tables = HashMap::new();

    for (table_name, table_config) in &loaded.config.tables {
        let readable = match &table_config.readable {
            ReadableColumnsConfig::All => ColumnList::All(AllColumns),
            ReadableColumnsConfig::List(cols) => ColumnList::List(cols.clone()),
            ReadableColumnsConfig::None => ColumnList::List(Vec::new()),
        };

        // Convert editable to both creatable and updatable (old model didn't distinguish)
        let (creatable, updatable) = match &table_config.editable {
            EditableColumnsConfig::All => (
                CreatableColumns::All(AllColumns),
                UpdatableColumns::All(AllColumns),
            ),
            EditableColumnsConfig::Map(map) => {
                use cori_core::config::role_definition::ApprovalRequirement;
                
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
                        let restrict_to = constraints.allowed_values.as_ref().map(|vals| {
                            vals.iter().map(|v| serde_json::Value::String(v.clone())).collect()
                        });
                        (
                            col.clone(),
                            UpdatableColumnConstraints {
                                restrict_to,
                                transitions: None,
                                only_when: None,
                                increment_only: false,
                                append_only: false,
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
        blocked_tables: loaded.config.blocked_tables.clone(),
        max_rows_per_query: loaded.config.max_rows_per_query,
        max_affected_rows: loaded.config.max_affected_rows,
    }
}

/// Build a CoriConfig for the dashboard state.
fn build_cori_config(
    serve_config: &ServeConfig,
    _config_dir: &Path,
    rules: &Option<RulesDefinition>,
    roles: &HashMap<String, RoleDefinition>,
) -> anyhow::Result<cori_core::config::CoriConfig> {
    use cori_core::config::CoriConfig;

    let upstream = UpstreamConfig {
        host: serve_config
            .upstream
            .host
            .clone()
            .unwrap_or_else(|| "localhost".to_string()),
        port: serve_config.upstream.port,
        database: serve_config
            .upstream
            .database
            .clone()
            .unwrap_or_else(|| "postgres".to_string()),
        username: serve_config
            .upstream
            .username
            .clone()
            .unwrap_or_else(|| "postgres".to_string()),
        password: serve_config.upstream.password.clone(),
        database_url_env: serve_config.upstream.database_url_env.clone(),
        database_url: serve_config.upstream.database_url.clone(),
        ..Default::default()
    };

    let mcp = McpConfig {
        enabled: serve_config.mcp.enabled,
        transport: match serve_config.mcp.transport.as_deref() {
            Some("http") => Transport::Http,
            _ => Transport::Stdio,
        },
        host: "127.0.0.1".to_string(),
        port: serve_config.mcp.http_port,
    };

    let dashboard = DashboardConfig {
        enabled: serve_config.dashboard.enabled,
        host: "127.0.0.1".to_string(),
        port: serve_config.dashboard.listen_port,
        ..Default::default()
    };

    let audit = AuditConfig {
        enabled: serve_config.audit.enabled,
        log_queries: serve_config.audit.log_queries,
        log_results: serve_config.audit.log_results,
        ..Default::default()
    };

    Ok(CoriConfig {
        project: None,
        version: None,
        upstream,
        biscuit: cori_core::config::BiscuitConfig {
            public_key_env: serve_config.biscuit.public_key_env.clone(),
            public_key_file: serve_config.biscuit.public_key_file.clone(),
            private_key_env: serve_config.biscuit.private_key_env.clone(),
            private_key_file: serve_config.biscuit.private_key_file.clone(),
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

/// Resolve keypair from config for the dashboard.
fn resolve_keypair(config: &BiscuitConfig, config_dir: &Path) -> anyhow::Result<cori_biscuit::keys::KeyPair> {
    // Try private key file first
    if let Some(private_key_file) = &config.private_key_file {
        let path = if private_key_file.is_absolute() {
            private_key_file.clone()
        } else {
            config_dir.join(private_key_file)
        };

        if path.exists() {
            return cori_biscuit::keys::KeyPair::load_from_file(&path)
                .map_err(|e| anyhow::anyhow!("Failed to load keypair from {}: {}", path.display(), e));
        }
    }

    // Try private key env
    if let Some(env_var) = &config.private_key_env {
        if let Ok(key_hex) = std::env::var(env_var) {
            return cori_biscuit::keys::KeyPair::from_private_key_hex(&key_hex)
                .map_err(|e| anyhow::anyhow!("Failed to load keypair from env {}: {}", env_var, e));
        }
    }

    // Try default BISCUIT_PRIVATE_KEY
    if let Ok(key_hex) = std::env::var("BISCUIT_PRIVATE_KEY") {
        return cori_biscuit::keys::KeyPair::from_private_key_hex(&key_hex)
            .map_err(|e| anyhow::anyhow!("Failed to load keypair from BISCUIT_PRIVATE_KEY: {}", e));
    }

    // Generate a new keypair if none found (for dashboard-only usage)
    tracing::warn!("No Biscuit private key found, generating ephemeral keypair for dashboard");
    cori_biscuit::keys::KeyPair::generate()
        .map_err(|e| anyhow::anyhow!("Failed to generate ephemeral keypair: {}", e))
}

/// Load rules definition from schema/rules.yaml.
fn load_rules(config_dir: &Path) -> anyhow::Result<Option<RulesDefinition>> {
    let rules_path = config_dir.join("schema/rules.yaml");
    if rules_path.exists() {
        match RulesDefinition::from_file(&rules_path) {
            Ok(rules) => {
                tracing::info!(
                    tables = rules.tables.len(),
                    path = %rules_path.display(),
                    "Loaded rules configuration"
                );
                Ok(Some(rules))
            }
            Err(e) => {
                tracing::warn!("Failed to load rules file: {}", e);
                Ok(None)
            }
        }
    } else {
        tracing::debug!("No rules file found at {}", rules_path.display());
        Ok(None)
    }
}

/// Resolve the Biscuit public key from config.
fn resolve_public_key(config: &BiscuitConfig) -> anyhow::Result<String> {
    // Try environment variable first
    if let Some(env_var) = &config.public_key_env {
        if let Ok(key) = std::env::var(env_var) {
            return Ok(key);
        }
    }

    // Try file
    if let Some(file_path) = &config.public_key_file {
        let key = fs::read_to_string(file_path)?;
        return Ok(key.trim().to_string());
    }

    // Try default environment variable
    if let Ok(key) = std::env::var("BISCUIT_PUBLIC_KEY") {
        return Ok(key);
    }

    anyhow::bail!(
        "Biscuit public key not found. Set BISCUIT_PUBLIC_KEY environment variable, \
        or configure biscuit.public_key_env or biscuit.public_key_file in the config file."
    )
}

/// Load roles from configuration.
pub fn load_roles(config: &ServeConfig, config_dir: &Path) -> anyhow::Result<Vec<LoadedRole>> {
    let mut roles: HashMap<String, RoleConfigFile> = HashMap::new();

    // 1. Load from roles_dir if specified, otherwise try convention default "roles/"
    let roles_dir = config
        .roles_dir
        .clone()
        .or_else(|| Some(PathBuf::from("roles")));
    
    if let Some(roles_dir) = &roles_dir {
        let resolved_dir = if roles_dir.is_absolute() {
            roles_dir.clone()
        } else {
            config_dir.join(roles_dir)
        };

        if resolved_dir.exists() && resolved_dir.is_dir() {
            tracing::info!(dir = %resolved_dir.display(), "Loading roles from directory");
            let dir_roles = load_roles_from_directory(&resolved_dir)?;
            for role in dir_roles {
                roles.insert(role.name.clone(), role.config);
            }
        } else if config.roles_dir.is_some() {
            // Only warn if explicitly configured (not just default)
            tracing::warn!(
                dir = %resolved_dir.display(),
                "Roles directory does not exist or is not a directory"
            );
        } else {
            // Debug only for default directory not existing
            tracing::debug!(
                dir = %resolved_dir.display(),
                "Default roles directory not found, skipping"
            );
        }
    }

    // 2. Load from role_files if specified
    for role_file in &config.role_files {
        let resolved_path = if role_file.is_absolute() {
            role_file.clone()
        } else {
            config_dir.join(role_file)
        };

        if resolved_path.exists() {
            let role = load_role_from_file(&resolved_path)?;
            tracing::debug!(
                role = %role.name,
                file = %resolved_path.display(),
                "Loaded role from file"
            );
            roles.insert(role.name.clone(), role.config);
        } else {
            tracing::warn!(
                file = %resolved_path.display(),
                "Role file does not exist"
            );
        }
    }

    // 3. Load inline roles (these take precedence)
    for (name, role_config) in &config.roles {
        tracing::debug!(role = %name, "Loaded inline role");
        let mut config = role_config.clone();
        config.name = Some(name.clone());
        roles.insert(name.clone(), config);
    }

    // Convert to Vec<LoadedRole>
    let loaded_roles: Vec<LoadedRole> = roles
        .into_iter()
        .map(|(name, config)| LoadedRole { name, config })
        .collect();

    tracing::info!(count = loaded_roles.len(), "Total roles loaded");
    Ok(loaded_roles)
}

/// Load all role files from a directory.
fn load_roles_from_directory(dir: &Path) -> anyhow::Result<Vec<LoadedRole>> {
    let mut roles = Vec::new();

    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();

        // Only process .yaml and .yml files
        if path.is_file() {
            if let Some(ext) = path.extension() {
                if ext == "yaml" || ext == "yml" {
                    match load_role_from_file(&path) {
                        Ok(role) => {
                            tracing::debug!(
                                role = %role.name,
                                file = %path.display(),
                                "Loaded role from directory"
                            );
                            roles.push(role);
                        }
                        Err(e) => {
                            tracing::warn!(
                                file = %path.display(),
                                error = %e,
                                "Failed to load role file"
                            );
                        }
                    }
                }
            }
        }
    }

    Ok(roles)
}

/// Load a single role from a YAML file.
fn load_role_from_file(path: &Path) -> anyhow::Result<LoadedRole> {
    let contents = fs::read_to_string(path)?;
    let config: RoleConfigFile = serde_yaml::from_str(&contents)?;

    // Determine role name: from config.name, or from filename
    let name = config.name.clone().unwrap_or_else(|| {
        path.file_stem()
            .and_then(|s| s.to_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| "unknown".to_string())
    });

    Ok(LoadedRole { name, config })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_yaml_config() {
        let config = r#"
upstream:
  host: localhost
  port: 5432
  database: mydb
  username: myuser
  password: mypass

biscuit:
  public_key_env: BISCUIT_PUBLIC_KEY

audit:
  enabled: true
  log_queries: true
  log_results: false
"#;

        let serve_config: ServeConfig = serde_yaml::from_str(config).unwrap();
        assert_eq!(serve_config.upstream.host.as_deref(), Some("localhost"));
        // Note: tenancy config is now in schema/rules.yaml, not in ServeConfig
    }

    #[test]
    fn test_defaults() {
        let config = r#"
upstream:
  host: localhost
  database: mydb

biscuit:
  public_key_file: /path/to/key
"#;

        let serve_config: ServeConfig = serde_yaml::from_str(config).unwrap();
        assert_eq!(serve_config.upstream.port, 5432);
        // Note: tenancy config is now in schema/rules.yaml, not in ServeConfig
    }

    #[test]
    fn test_parse_roles_dir() {
        let config = r#"
upstream:
  host: localhost
  database: mydb

biscuit:
  public_key_file: /path/to/key

roles_dir: roles
"#;

        let serve_config: ServeConfig = serde_yaml::from_str(config).unwrap();
        assert_eq!(serve_config.roles_dir, Some(PathBuf::from("roles")));
    }

    #[test]
    fn test_parse_inline_roles() {
        let config = r#"
upstream:
  host: localhost
  database: mydb

biscuit:
  public_key_file: /path/to/key

roles:
  support_agent:
    description: "Support agent role"
    tables:
      customers:
        readable: [id, name, email]
        editable: []
      tickets:
        readable: "*"
        editable:
          status:
            allowed_values: [open, closed]
    blocked_tables:
      - users
      - api_keys
    max_rows_per_query: 100
"#;

        let serve_config: ServeConfig = serde_yaml::from_str(config).unwrap();
        assert_eq!(serve_config.roles.len(), 1);
        assert!(serve_config.roles.contains_key("support_agent"));

        let support = serve_config.roles.get("support_agent").unwrap();
        assert_eq!(
            support.description,
            Some("Support agent role".to_string())
        );
        assert_eq!(support.tables.len(), 2);
        assert_eq!(support.blocked_tables.len(), 2);
        assert_eq!(support.max_rows_per_query, Some(100));
    }
}
