use clap::{Parser, Subcommand};

use serde::Deserialize;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

// MCP commands module
mod commands;

#[derive(Parser, Debug)]
#[command(name = "cori", version, about = "Cori CLI")]
struct Cli {
    #[command(subcommand)]
    cmd: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Initialize a Cori MCP server project from an existing database.
    ///
    /// This command introspects your database schema and creates a complete
    /// project structure with:
    /// - Configuration file (cori.yaml) with tenant isolation settings
    /// - Biscuit keypair for token authentication (keys/)
    /// - Sample role definitions (roles/)
    /// - Schema snapshot (schema/)
    /// - Proper .gitignore for security
    Init {
        /// Database URL (Postgres), e.g. postgres://user:pass@host:5432/db
        #[arg(long = "from-db")]
        from_db: String,

        /// Project name (also used as output directory)
        #[arg(long)]
        project: String,

        /// Overwrite if the project directory already exists
        #[arg(long, default_value_t = false)]
        force: bool,
    },

    /// Database schema management.
    Db {
        #[command(subcommand)]
        cmd: DbCommand,
    },

    // ===== Phase 1: Core Commands =====

    /// Generate Biscuit keypair for token signing.
    Keys {
        #[command(subcommand)]
        cmd: KeysCommand,
    },

    /// Biscuit token management (mint, attenuate, inspect, verify).
    Token {
        #[command(subcommand)]
        cmd: TokenCommand,
    },

    /// Start the Cori MCP server and dashboard.
    Serve {
        /// Path to configuration file (YAML or TOML).
        #[arg(long, short, default_value = "cori.yaml")]
        config: PathBuf,
    },

    /// MCP server for AI agent integration.
    Mcp(commands::mcp::McpCommand),

    /// Validate configuration files for consistency and correctness.
    ///
    /// This command performs comprehensive validation:
    /// - JSON Schema validation against schemas in schemas/
    /// - Cross-file consistency checks (tables, columns, groups)
    /// - Best practice warnings (soft delete, approval groups)
    Check {
        /// Path to configuration file (YAML).
        #[arg(long, short, default_value = "cori.yaml")]
        config: PathBuf,
    },
}

// ===== Phase 1: Core Command Definitions =====

#[derive(Subcommand, Debug)]
enum KeysCommand {
    /// Generate a new Ed25519 keypair for Biscuit token signing.
    Generate {
        /// Output directory for key files. If not specified, prints to stdout.
        #[arg(long, short)]
        output: Option<PathBuf>,
    },
}

#[derive(Subcommand, Debug)]
enum TokenCommand {
    /// Mint a new role token (or agent token if --tenant is specified).
    Mint {
        /// Path to private key file. Falls back to BISCUIT_PRIVATE_KEY env var if not provided.
        #[arg(long, env = "BISCUIT_PRIVATE_KEY")]
        key: Option<String>,

        /// Role name for the token.
        #[arg(long)]
        role: String,

        /// Tenant ID (if specified, creates an attenuated agent token).
        #[arg(long)]
        tenant: Option<String>,

        /// Expiration duration (e.g., "24h", "7d", "1h").
        #[arg(long)]
        expires: Option<String>,

        /// Tables to grant access to (format: "table:col1,col2" or just "table").
        #[arg(long = "table")]
        tables: Vec<String>,

        /// Output file path. If not specified, prints to stdout.
        #[arg(long, short)]
        output: Option<PathBuf>,
    },

    /// Attenuate a role token with tenant restriction and expiration.
    Attenuate {
        /// Path to private key file. Falls back to BISCUIT_PRIVATE_KEY env var if not provided.
        #[arg(long, env = "BISCUIT_PRIVATE_KEY")]
        key: Option<String>,

        /// Path to base role token file.
        #[arg(long)]
        base: PathBuf,

        /// Tenant ID to restrict the token to.
        #[arg(long)]
        tenant: String,

        /// Expiration duration (e.g., "24h", "7d").
        #[arg(long)]
        expires: Option<String>,

        /// Output file path. If not specified, prints to stdout.
        #[arg(long, short)]
        output: Option<PathBuf>,
    },

    /// Inspect a token without verification.
    Inspect {
        /// Token string or path to token file.
        token: String,
    },

    /// Verify a token is valid.
    Verify {
        /// Path to public key file. Falls back to BISCUIT_PUBLIC_KEY env var if not provided.
        #[arg(long, env = "BISCUIT_PUBLIC_KEY")]
        key: Option<String>,

        /// Token string or path to token file.
        token: String,
    },
}

#[derive(Subcommand, Debug)]
enum DbCommand {
    /// Sync database schema to schema/schema.yaml.
    ///
    /// Introspects the configured database and generates a YAML schema definition
    /// that can be used for role-based access control configuration.
    Sync,
}

/// Run configuration check as a pre-hook before commands that depend on config.
/// Only runs if cori.yaml exists in the current directory.
async fn run_pre_hook_check() -> anyhow::Result<()> {
    let default_config = PathBuf::from("cori.yaml");
    if default_config.exists() {
        commands::check::run_pre_hook(&default_config).await?;
    }
    Ok(())
}

/// Run configuration check as a pre-hook with a custom config path.
async fn run_pre_hook_check_with_config(config_path: &Path) -> anyhow::Result<()> {
    commands::check::run_pre_hook(config_path).await
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Configure tracing to write to stderr (not stdout) to avoid contaminating
    // JSON-RPC messages when using stdio transport for MCP
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_ansi(false) // Disable ANSI colors to avoid escape codes
        .with_env_filter("info")
        .init();

    let cli = Cli::parse();

    match cli.cmd {
        Command::Init {
            from_db,
            project,
            force,
        } => commands::init::run(&from_db, &project, force).await?,

        Command::Db { cmd } => {
            run_pre_hook_check().await?;
            run_db(cmd).await?
        }

        // ===== Phase 1: Core Command Handlers =====

        Command::Keys { cmd } => match cmd {
            KeysCommand::Generate { output } => {
                commands::keys::generate(output)?;
            }
        },

        Command::Token { cmd } => match cmd {
            TokenCommand::Mint {
                key,
                role,
                tenant,
                expires,
                tables,
                output,
            } => {
                commands::token::mint(key, role, tenant, expires, tables, output)?;
            }
            TokenCommand::Attenuate {
                key,
                base,
                tenant,
                expires,
                output,
            } => {
                commands::token::attenuate(key, base, tenant, expires, output)?;
            }
            TokenCommand::Inspect { token } => {
                commands::token::inspect(token)?;
            }
            TokenCommand::Verify { key, token } => {
                commands::token::verify(key, token)?;
            }
        },

        Command::Serve { config } => {
            run_pre_hook_check_with_config(&config).await?;
            commands::serve::serve(config).await?;
        }

        Command::Mcp(cmd) => {
            // For MCP, check the config path from the command args if available
            let config_path = cmd.get_config_path();
            run_pre_hook_check_with_config(&config_path).await?;
            commands::mcp::execute(cmd).await?;
        }

        Command::Check { config } => {
            commands::check::run(&config).await?;
        }
    }

    Ok(())
}

// -----------------------------
// db commands
// -----------------------------

async fn run_db(cmd: DbCommand) -> anyhow::Result<()> {
    let cfg = load_cori_config_from_cwd()?;
    ensure_postgres_adapter(&cfg)?;

    let db_url = resolve_database_url(&cfg)?;

    match cmd {
        DbCommand::Sync => {
            fs::create_dir_all("schema")?;
            let schema_path = PathBuf::from("schema").join("schema.yaml");
            
            // Introspect database schema
            let snapshot = cori_adapter_pg::introspect::introspect_schema_json(&db_url).await?;
            
            // Convert to YAML format
            let yaml = serde_yaml::to_string(&snapshot)?;
            fs::write(&schema_path, yaml)?;
            println!("✔ Wrote schema definition: {}", schema_path.display());
        }
    }

    Ok(())
}

// -----------------------------
// config + shared IO helpers
// -----------------------------

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct CoriConfig {
    #[allow(dead_code)]
    project: Option<String>,
    adapter: Option<String>,
    database_url_env: Option<String>,
    environment: Option<String>,
    #[allow(dead_code)]
    max_affected_rows: Option<u64>,
    #[allow(dead_code)]
    preview_row_limit: Option<u32>,
}

fn load_cori_config_from_cwd() -> anyhow::Result<CoriConfig> {
    let path = PathBuf::from("cori.yaml");
    if !path.exists() {
        return Err(anyhow::anyhow!(
            "cori.yaml not found in current directory. Run this inside a Cori project."
        ));
    }
    let contents = fs::read_to_string(path)?;
    let cfg: CoriConfig = serde_yaml::from_str(&contents)?;
    Ok(cfg)
}

fn ensure_postgres_adapter(cfg: &CoriConfig) -> anyhow::Result<()> {
    let adapter = cfg.adapter.as_deref().unwrap_or("postgres");
    if adapter != "postgres" {
        return Err(anyhow::anyhow!(
            "Only adapter=postgres is supported right now (found '{}').",
            adapter
        ));
    }
    Ok(())
}

fn resolve_database_url(cfg: &CoriConfig) -> anyhow::Result<String> {
    let env_name = cfg
        .database_url_env
        .as_deref()
        .unwrap_or("DATABASE_URL")
        .to_string();

    env::var(&env_name).map_err(|_| {
        anyhow::anyhow!(
            "Environment variable '{}' is not set. Export it with your DB URL.",
            env_name
        )
    })
}
