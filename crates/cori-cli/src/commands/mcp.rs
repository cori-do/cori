//! MCP command implementation.
//!
//! This module provides the `cori mcp` command for running the MCP server
//! in standalone mode or testing tool availability.

use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use cori_biscuit::{keys::load_public_key_file, KeyPair, TokenVerifier};
use cori_core::config::{McpConfig, ReadableColumns, RoleConfig, TenancyConfig, Transport};
use cori_mcp::McpServer;
use serde::Deserialize;
use sqlx::postgres::PgPoolOptions;
use std::path::PathBuf;
use tracing::{info, warn};

/// Partial config file structure for MCP settings.
#[derive(Debug, Deserialize, Default)]
struct ConfigFile {
    #[serde(default)]
    mcp: McpConfigFile,
    #[serde(default)]
    biscuit: BiscuitConfigFile,
    #[serde(default)]
    roles_dir: Option<String>,
    #[serde(default)]
    upstream: UpstreamConfigFile,
    #[serde(default)]
    tenancy_file: Option<String>,
}

/// Upstream database section of the config file.
#[derive(Debug, Deserialize, Default)]
struct UpstreamConfigFile {
    #[serde(default)]
    host: Option<String>,
    #[serde(default = "default_port")]
    port: u16,
    #[serde(default)]
    database: Option<String>,
    #[serde(default)]
    username: Option<String>,
    #[serde(default)]
    password: Option<String>,
    #[serde(default)]
    credentials_env: Option<String>,
}

fn default_port() -> u16 {
    5432
}

/// MCP section of the config file.
#[derive(Debug, Deserialize, Default)]
struct McpConfigFile {
    #[serde(default = "default_enabled")]
    enabled: bool,
    #[serde(default)]
    transport: Option<String>,
    #[serde(default)]
    http_port: Option<u16>,
    #[serde(default = "default_dry_run")]
    dry_run_enabled: bool,
    #[serde(default)]
    require_approval: Vec<String>,
}

/// Biscuit section of the config file.
#[derive(Debug, Deserialize, Default)]
struct BiscuitConfigFile {
    #[serde(default)]
    public_key_file: Option<String>,
    #[serde(default)]
    public_key_env: Option<String>,
    #[serde(default)]
    private_key_file: Option<String>,
    #[serde(default)]
    private_key_env: Option<String>,
}

fn default_enabled() -> bool {
    true
}

fn default_dry_run() -> bool {
    true
}

/// MCP-related commands.
#[derive(Debug, Args)]
pub struct McpCommand {
    #[command(subcommand)]
    pub command: McpSubcommand,
}

/// MCP subcommands.
#[derive(Debug, Subcommand)]
pub enum McpSubcommand {
    /// Start the MCP server (standalone mode).
    #[command(name = "serve")]
    Serve(McpServeArgs),

    /// Test tool availability for a given token.
    #[command(name = "test")]
    Test(McpTestArgs),
}

/// Arguments for `cori mcp serve`.
#[derive(Debug, Args)]
pub struct McpServeArgs {
    /// Configuration file path.
    #[arg(short, long, default_value = "cori.yaml")]
    pub config: PathBuf,

    /// Token file (or use CORI_TOKEN env var). The role is inferred from the token.
    #[arg(short, long)]
    pub token: Option<PathBuf>,

    /// Public key file for token verification.
    #[arg(long)]
    pub public_key: Option<PathBuf>,

    /// Transport type (stdio or http). Overrides config file.
    #[arg(long)]
    pub transport: Option<String>,

    /// HTTP port (only for http transport). Overrides config file.
    #[arg(long)]
    pub port: Option<u16>,

    /// Roles directory to load role configurations from.
    #[arg(long, default_value = "roles")]
    pub roles_dir: PathBuf,
}

/// Arguments for `cori mcp test`.
#[derive(Debug, Args)]
pub struct McpTestArgs {
    /// Token file to test. The role is inferred from the token.
    #[arg(short, long)]
    pub token: PathBuf,

    /// Public key file for token verification.
    #[arg(long)]
    pub public_key: Option<PathBuf>,

    /// Roles directory to load role configurations from.
    #[arg(long, default_value = "roles")]
    pub roles_dir: PathBuf,

    /// Show detailed tool schemas.
    #[arg(long)]
    pub verbose: bool,
}

/// Execute the MCP command.
pub async fn execute(cmd: McpCommand) -> Result<()> {
    match cmd.command {
        McpSubcommand::Serve(args) => execute_serve(args).await,
        McpSubcommand::Test(args) => execute_test(args).await,
    }
}

async fn execute_serve(args: McpServeArgs) -> Result<()> {
    // Load config file if it exists
    let config_file: ConfigFile = if args.config.exists() {
        let content = std::fs::read_to_string(&args.config)
            .with_context(|| format!("Failed to read config file: {:?}", args.config))?;
        serde_yaml::from_str(&content)
            .with_context(|| format!("Failed to parse config file: {:?}", args.config))?
    } else {
        warn!(config = %args.config.display(), "Config file not found, using defaults");
        ConfigFile::default()
    };

    // Determine transport: CLI overrides config file
    let transport_str = args.transport
        .as_deref()
        .or(config_file.mcp.transport.as_deref())
        .unwrap_or("stdio");
    
    let transport = match transport_str {
        "stdio" => Transport::Stdio,
        "http" => Transport::Http,
        other => anyhow::bail!("Unknown transport: {}. Use 'stdio' or 'http'", other),
    };

    // Determine HTTP port: CLI overrides config file
    let http_port = args.port
        .or(config_file.mcp.http_port)
        .unwrap_or(3000);

    // Load token from file or environment
    let token = if let Some(token_path) = &args.token {
        std::fs::read_to_string(token_path)
            .with_context(|| format!("Failed to read token file: {:?}", token_path))?
            .trim()
            .to_string()
    } else if let Ok(token_env) = std::env::var("CORI_TOKEN") {
        // Decode from base64 if needed
        if token_env.contains('.') {
            // Already base64 biscuit format
            token_env
        } else {
            // Try to decode as base64
            String::from_utf8(
                base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &token_env)
                    .unwrap_or_else(|_| token_env.as_bytes().to_vec()),
            )
            .unwrap_or(token_env)
        }
    } else {
        warn!("No token provided. Running without authentication.");
        String::new()
    };

    // Verify token if we have a public key
    let mut tenant_id = None;
    let mut role_name = None;

    // Resolve public key: CLI arg > config file > env var
    let config_dir = args.config.parent().map(|p| p.to_path_buf());
    let public_key_path: Option<PathBuf> = args.public_key.clone()
        .or_else(|| {
            // Try config file public_key_file (relative to config file dir)
            config_file.biscuit.public_key_file.as_ref().map(|f| {
                let path = PathBuf::from(f);
                if path.is_absolute() {
                    path
                } else {
                    config_dir.as_ref().map(|dir| dir.join(&path)).unwrap_or(path)
                }
            })
        })
        .or_else(|| {
            // Try public_key_env 
            config_file.biscuit.public_key_env.as_ref()
                .and_then(|env_var| std::env::var(env_var).ok())
                .map(PathBuf::from)
        });

    if !token.is_empty() {
        if let Some(pk_path) = &public_key_path {
            let public_key = load_public_key_file(pk_path)
                .with_context(|| format!("Failed to load public key from {:?}", pk_path))?;

            let verifier = TokenVerifier::new(public_key);
            match verifier.verify(&token) {
                Ok(verified) => {
                    info!(role = %verified.role, tenant = ?verified.tenant, "Token verified");
                    role_name = Some(verified.role.clone());
                    tenant_id = verified.tenant.clone();
                }
                Err(e) => {
                    warn!("Token verification failed: {}", e);
                }
            }
        } else {
            anyhow::bail!(
                "Token provided but no public key found for verification.\n\
                 \n\
                 Provide a public key via one of:\n\
                 • --public-key <path>        Command line argument\n\
                 • biscuit.public_key_file    In config file ({})\n\
                 • biscuit.public_key_env     Environment variable name in config file",
                args.config.display()
            );
        }
    }

    // Resolve roles directory: CLI arg > config file > default
    let roles_dir = if args.roles_dir != PathBuf::from("roles") {
        // CLI provided a non-default value
        args.roles_dir.clone()
    } else if let Some(cfg_roles_dir) = &config_file.roles_dir {
        // Use config file value (relative to config file dir)
        let path = PathBuf::from(cfg_roles_dir);
        if path.is_absolute() {
            path
        } else {
            config_dir.as_ref().map(|dir| dir.join(&path)).unwrap_or(path)
        }
    } else {
        // Default: relative to config file dir
        config_dir.as_ref().map(|dir| dir.join("roles")).unwrap_or_else(|| PathBuf::from("roles"))
    };

    // Load role configuration from token's role claim
    let role_config = if let Some(name) = role_name {
        // Try to find role file based on token's role
        let role_path = roles_dir.join(format!("{}.yaml", name));
        if role_path.exists() {
            RoleConfig::from_file(&role_path)
                .with_context(|| format!("Failed to load role config: {:?}", role_path))?
        } else {
            warn!(role = %name, path = %role_path.display(), "No role configuration file found for role from token");
            // Create minimal role config
            RoleConfig {
                name,
                description: None,
                tables: std::collections::HashMap::new(),
                blocked_tables: Vec::new(),
                max_rows_per_query: Some(100),
                max_affected_rows: Some(10),
                blocked_operations: Vec::new(),
                custom_actions: Vec::new(),
                include_actions: Vec::new(),
            }
        }
    } else {
        anyhow::bail!(
            "No role found in token. The token must contain a role claim.\n\
             \n\
             Use `cori token inspect` or `cori token verify --key <public.key>` to check your token."
        );
    };

    // Configure MCP server using resolved config
    let mcp_config = McpConfig {
        enabled: config_file.mcp.enabled,
        transport,
        http_port,
        dry_run_enabled: config_file.mcp.dry_run_enabled,
        auto_generate_tools: true,
        require_approval: config_file.mcp.require_approval.clone(),
        approval_exceptions: Vec::new(),
    };

    info!(
        transport = ?mcp_config.transport,
        http_port = mcp_config.http_port,
        "MCP configuration loaded"
    );

    // Build database URL from config
    let database_url = build_database_url(&config_file.upstream)?;
    
    // Connect to database
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&database_url)
        .await
        .with_context(|| format!("Failed to connect to database"))?;

    info!("Connected to upstream database");

    // Load tenancy configuration (per-table tenant columns) using cori-core
    let config_dir = args.config.parent().unwrap_or(std::path::Path::new("."));
    let tenancy_config = if let Some(tenancy_file) = &config_file.tenancy_file {
        match TenancyConfig::load_from_path(tenancy_file, config_dir) {
            Ok(tenancy) => {
                info!(
                    default_column = %tenancy.default_column,
                    tables = tenancy.tables.len(),
                    "Loaded tenancy configuration"
                );
                tenancy
            }
            Err(e) => {
                warn!("Failed to load tenancy file: {}", e);
                TenancyConfig {
                    default_column: "organization_id".to_string(),
                    ..TenancyConfig::default()
                }
            }
        }
    } else {
        TenancyConfig {
            default_column: "organization_id".to_string(),
            ..TenancyConfig::default()
        }
    };

    // Load schema from snapshot file if available
    let schema_path = args.config.parent().unwrap_or(std::path::Path::new(".")).join("schema/snapshot.json");
    let schema = if schema_path.exists() {
        match std::fs::read_to_string(&schema_path) {
            Ok(content) => match serde_json::from_str::<serde_json::Value>(&content) {
                Ok(json) => {
                    match cori_mcp::schema::parse_schema_from_json(&json) {
                        Ok(s) => {
                            info!("Loaded schema from {}", schema_path.display());
                            Some(s)
                        }
                        Err(e) => {
                            warn!("Failed to parse schema: {:?}", e);
                            None
                        }
                    }
                }
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
    } else {
        info!("No schema file found at {}, using fallback primary key derivation", schema_path.display());
        None
    };

    // Create and run server
    let mut server = McpServer::new(mcp_config)
        .with_pool(pool)
        .with_tenancy_config(tenancy_config);

    // Add schema if loaded
    if let Some(s) = schema {
        server = server.with_schema(s);
    }

    // Add role config (must be after schema for proper executor setup)
    server = server.with_role_config(role_config);

    if let Some(tid) = tenant_id {
        server = server.with_tenant_id(tid);
    }

    // Generate tools from role config
    server.generate_tools();

    info!(
        tool_count = server.tools_mut().len(),
        "MCP server starting"
    );

    server.run().await?;

    Ok(())
}

/// Build database URL from upstream config.
fn build_database_url(upstream: &UpstreamConfigFile) -> Result<String> {
    // First check for credentials_env
    if let Some(env_var) = &upstream.credentials_env {
        if let Ok(url) = std::env::var(env_var) {
            return Ok(url);
        }
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

async fn execute_test(args: McpTestArgs) -> Result<()> {
    // Load token
    let token = std::fs::read_to_string(&args.token)
        .with_context(|| format!("Failed to read token file: {:?}", args.token))?
        .trim()
        .to_string();

    // Verify token - required to get role claim
    let verified_info = if let Some(pk_path) = &args.public_key {
        let keypair = KeyPair::load_from_file(pk_path)
            .with_context(|| format!("Failed to load key from {:?}", pk_path))?;

        let verifier = TokenVerifier::new(keypair.public_key());
        match verifier.verify(&token) {
            Ok(verified) => {
                Some((verified.role.clone(), verified.tenant.clone()))
            }
            Err(e) => {
                anyhow::bail!("Token verification failed: {}. Provide --public-key to verify the token.", e);
            }
        }
    } else {
        anyhow::bail!("Public key required to verify token and extract role. Use --public-key.");
    };

    let (role_name, tenant) = verified_info.unwrap();

    // Load role configuration from token's role claim
    let role_path = args.roles_dir.join(format!("{}.yaml", role_name));
    let role_config = if role_path.exists() {
        RoleConfig::from_file(&role_path)
            .with_context(|| format!("Failed to load role config: {:?}", role_path))?
    } else {
        anyhow::bail!(
            "No role configuration file found at {:?} for role '{}' from token",
            role_path,
            role_name
        );
    };

    // Print token info
    println!("\n🔑 Token Information:");
    println!("   Role: {}", role_name);
    if let Some(t) = &tenant {
        println!("   Tenant: {}", t);
    }

    // Create server and generate tools
    let mcp_config = McpConfig::default();
    let mut server = McpServer::new(mcp_config).with_role_config(role_config.clone());

    // Generate tools
    server.generate_tools();

    // Print available tools
    let tools = server.tools_mut().list();
    println!("\n🔧 Available Tools ({}):", tools.len());

    for tool in tools {
        let annotations = tool.annotations.as_ref();
        let read_only = annotations.map_or(false, |a| a.read_only == Some(true));
        let requires_approval = annotations.map_or(false, |a| a.requires_approval == Some(true));
        let dry_run = annotations.map_or(false, |a| a.dry_run_supported == Some(true));

        let mut badges = Vec::new();
        if read_only {
            badges.push("read");
        } else {
            badges.push("write");
        }
        if requires_approval {
            badges.push("approval");
        }
        if dry_run {
            badges.push("dry-run");
        }

        println!("   • {} ({})", tool.name, badges.join(", "));

        if let Some(desc) = &tool.description {
            println!("     {}", desc);
        }

        if args.verbose {
            println!(
                "     Schema: {}",
                serde_json::to_string_pretty(&tool.input_schema)?
            );
        }
    }

    // Print table access summary
    println!("\n📊 Table Access:");
    for (table, perms) in &role_config.tables {
        let can_read = !matches!(&perms.readable, ReadableColumns::List(cols) if cols.is_empty());
        let can_write = !perms.editable.is_empty();

        let mut access = Vec::new();
        if can_read {
            access.push("read");
        }
        if can_write {
            access.push("write");
        }

        if access.is_empty() {
            access.push("none");
        }

        println!("   • {} ({})", table, access.join(", "));
    }

    if !role_config.blocked_tables.is_empty() {
        println!("\n🚫 Blocked Tables:");
        for table in &role_config.blocked_tables {
            println!("   • {}", table);
        }
    }

    println!();

    Ok(())
}
