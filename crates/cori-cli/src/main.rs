use clap::{Parser, Subcommand};
use cori_adapter_pg::PostgresAdapter;
use cori_core::{
    ActionDefinition, AuditEvent, AuditEventType, MutationIntent, Plan, Principal, Step, StepKind,
};
use cori_policy::PolicyClient;
use cori_runtime::{audit::StdoutAuditSink, orchestrator::Orchestrator};

use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use cori_runtime::audit::{AuditEvent as RtAuditEvent, AuditSink};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use uuid::Uuid;

// Proxy commands module
mod commands;

#[derive(Parser, Debug)]
#[command(name = "cori", version, about = "Cori CLI")]
struct Cli {
    #[command(subcommand)]
    cmd: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Initialize a Cori AI Database Proxy project from an existing database.
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

    /// Schema management (snapshot/diff/inspect)
    Schema {
        #[command(subcommand)]
        cmd: SchemaCommand,
    },

    /// Code/artifact generation
    Generate {
        #[command(subcommand)]
        cmd: GenerateCommand,
    },

    /// Actions management (list/describe/validate)
    Actions {
        #[command(subcommand)]
        cmd: ActionsCommand,
    },

    /// Plan operations (validate/preview)
    Plan {
        #[command(subcommand)]
        cmd: PlanCommand,
    },

    /// Apply a plan: creates an intent on disk. If --preview, runs a dry-run and saves report.
    Apply {
        file: PathBuf,
        #[arg(long, default_value_t = false)]
        preview: bool,
    },

    /// Approve an intent (required before execute when preview=false)
    Approve {
        intent_id: String,
        #[arg(long)]
        reason: String,
        /// Optional principal id for the approver (e.g. "user:alice"). Defaults to "user:local".
        #[arg(long = "as")]
        as_principal: Option<String>,
    },

    /// Execute an approved intent
    Execute { intent_id: String },

    /// Show status of an intent
    Status { intent_id: String },

    /// Run a trivial built-in intent (stub) to verify wiring.
    Smoke {
        #[arg(long, default_value = "acme")]
        tenant: String,
        #[arg(long, default_value = "dev")]
        env: String,
        #[arg(long)]
        preview: bool,
        #[arg(
            long,
            default_value = "postgres://postgres:postgres@localhost:5432/crm"
        )]
        database_url: String,
    },

    // ===== Phase 1: Core Proxy Commands =====

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

    /// Start the Cori Postgres proxy server.
    Serve {
        /// Path to configuration file (YAML or TOML).
        #[arg(long, short, default_value = "cori.yaml")]
        config: PathBuf,
    },

    /// Proxy utilities (test, explain).
    Proxy {
        #[command(subcommand)]
        cmd: ProxyCommand,
    },

    /// MCP server for AI agent integration.
    Mcp(commands::mcp::McpCommand),
}

// ===== Phase 1: Core Proxy Command Definitions =====

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
enum ProxyCommand {
    /// Explain what RLS predicates would be injected for a query.
    Explain {
        /// The SQL query to explain.
        #[arg(long)]
        query: String,

        /// Tenant ID to use for RLS injection.
        #[arg(long)]
        tenant: String,

        /// Tenant column name (default: tenant_id).
        #[arg(long, default_value = "tenant_id")]
        tenant_column: String,
    },
}

#[derive(Subcommand, Debug)]
enum SchemaCommand {
    /// Capture a schema snapshot from the configured database into schema/snapshot.json
    Snapshot,

    /// Compare the saved snapshot (schema/snapshot.json) to the live DB schema
    Diff,

    /// Inspect the snapshot. With no --entity, lists entities. With --entity, prints details.
    Inspect {
        /// Table/entity name. Accepts "table" or "schema.table"
        #[arg(long)]
        entity: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
enum GenerateCommand {
    /// Generate ActionDefinition files from schema snapshot into actions/
    Actions {
        /// Overwrite existing generated action files and catalog
        #[arg(long, default_value_t = false)]
        force: bool,
    },
}

#[derive(Subcommand, Debug)]
enum ActionsCommand {
    /// List actions from actions/catalog.json
    List,

    /// Describe one action by name
    Describe { action_name: String },

    /// Validate actions against catalog + (optional) schema snapshot linkage
    Validate,
}

#[derive(Subcommand, Debug)]
enum PlanCommand {
    /// Validate a Plan (YAML/JSON) against actions/catalog.json + action input schemas
    Validate {
        /// Path to plan.yaml or plan.json
        file: PathBuf,
    },

    /// Preview a plan (dry-run) using the adapter in preview mode + policy checks
    Preview { file: PathBuf },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_env_filter("info").init();

    let cli = Cli::parse();

    match cli.cmd {
        Command::Init {
            from_db,
            project,
            force,
        } => commands::init::run(&from_db, &project, force).await?,

        Command::Schema { cmd } => run_schema(cmd).await?,

        Command::Generate { cmd } => run_generate(cmd).await?,

        Command::Actions { cmd } => run_actions(cmd).await?,

        Command::Plan { cmd } => run_plan(cmd).await?,

        Command::Apply { file, preview } => run_apply(&file, preview).await?,
        Command::Approve {
            intent_id,
            reason,
            as_principal,
        } => run_approve(&intent_id, &reason, as_principal.as_deref()).await?,
        Command::Execute { intent_id } => run_execute(&intent_id).await?,
        Command::Status { intent_id } => run_status(&intent_id).await?,

        Command::Smoke {
            tenant,
            env,
            preview,
            database_url,
        } => {
            // Smoke intentionally runs outside a Cori project directory, so we don't have
            // access to `actions/catalog.json` or `cori.yaml`. Use a minimal built-in action
            // definition and allow-all policy to verify wiring.
            let mut actions = BTreeMap::new();
            actions.insert(
                "ListCustomers".to_string(),
                cori_core::ActionDefinition {
                    name: "ListCustomers".to_string(),
                    version: None,
                    description: None,
                    kind: StepKind::Query,
                    resource_kind: "customers".to_string(),
                    policy_action: "list".to_string(),
                    input_schema: json!({
                        "type": "object",
                        "required": ["limit"],
                        "additionalProperties": false,
                        "properties": {
                            "limit": { "type": "integer", "minimum": 1, "maximum": 1000 },
                            "cursor": { "anyOf": [ { "type": "null" }, { "type": "string" } ], "default": null }
                        }
                    }),
                    effects: None,
                    meta: json!({
                        "generated": false,
                        "pg": {
                            "schema": "public",
                            "table": "customers",
                            "primary_key": [],
                            "tenant_column": null,
                            "version_column": null,
                            "deleted_at_column": null,
                            "deleted_by_column": null,
                            "delete_reason_column": null,
                            "columns": [],
                            "updatable_columns": []
                        }
                    }),
                },
            );

            let intent = MutationIntent {
                schema_version: "0.1.0".to_string(),
                intent_id: "smoke-0001".to_string(),
                tenant_id: tenant,
                environment: env,
                preview,
                principal: Principal {
                    id: "user:local".to_string(),
                    roles: vec!["admin".to_string()],
                    attrs: json!({}),
                },
                plan: Plan {
                    plan_id: None,
                    name: None,
                    summary: None,
                    steps: vec![Step {
                        id: "s1".to_string(),
                        kind: StepKind::Query,
                        action: "ListCustomers".to_string(),
                        inputs: json!({
                            "limit": 1,
                            "cursor": null
                        }),
                        depends_on: None,
                        meta: serde_json::Value::Null,
                    }],
                    meta: serde_json::Value::Null,
                },
                request: serde_json::Value::Null,
                meta: serde_json::Value::Null,
            };

            let adapter = PostgresAdapter::new(
                &database_url,
                cori_adapter_pg::PostgresAdapterOptions::default(),
            )
            .await?;
            let policy: Arc<dyn PolicyClient> = Arc::new(cori_policy::BiscuitPolicyClient::new());
            let audit = StdoutAuditSink;

            let orchestrator = Orchestrator::new(policy, adapter, audit, actions);
            let result = orchestrator.run(&intent).await?;

            println!("{}", serde_json::to_string_pretty(&result)?);
        }

        // ===== Phase 1: Core Proxy Command Handlers =====

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
            commands::serve::serve(config).await?;
        }

        Command::Proxy { cmd } => match cmd {
            ProxyCommand::Explain {
                query,
                tenant,
                tenant_column,
            } => {
                run_proxy_explain(&query, &tenant, &tenant_column)?;
            }
        },

        Command::Mcp(cmd) => {
            commands::mcp::execute(cmd).await?;
        }
    }

    Ok(())
}

/// Explain RLS injection for a query.
fn run_proxy_explain(query: &str, tenant: &str, tenant_column: &str) -> anyhow::Result<()> {
    use cori_core::config::TenancyConfig;
    use cori_rls::RlsInjector;
    use std::collections::HashMap;

    let tables = HashMap::new();
    // Use default tenant column for all tables
    let config = TenancyConfig {
        default_column: tenant_column.to_string(),
        tables,
        ..Default::default()
    };

    let injector = RlsInjector::new(config);
    let explanation = injector.explain(query, tenant)?;

    println!("RLS Injection Explanation:");
    println!("  Original: {}", explanation.original_sql);
    println!("  Rewritten: {}", explanation.rewritten_sql);
    println!("  Tenant: {}", explanation.tenant_id);
    println!("  Tables scoped: {:?}", explanation.tables_scoped);
    println!("  Predicates added: {:?}", explanation.predicates_added);

    Ok(())
}

// -----------------------------
// schema commands
// -----------------------------

async fn run_schema(cmd: SchemaCommand) -> anyhow::Result<()> {
    let cfg = load_cori_config_from_cwd()?;
    ensure_postgres_adapter(&cfg)?;

    let db_url = resolve_database_url(&cfg)?;
    let snapshot_path = PathBuf::from("schema").join("snapshot.json");

    match cmd {
        SchemaCommand::Snapshot => {
            fs::create_dir_all("schema")?;
            let snapshot = cori_adapter_pg::introspect::introspect_schema_json(&db_url).await?;
            fs::write(&snapshot_path, serde_json::to_vec_pretty(&snapshot)?)?;
            println!("Wrote schema snapshot: {}", snapshot_path.display());
        }

        SchemaCommand::Diff => {
            if !snapshot_path.exists() {
                return Err(anyhow::anyhow!(
                    "Missing {}. Run `cori schema snapshot` first.",
                    snapshot_path.display()
                ));
            }

            let old = read_json(&snapshot_path)?;
            let live = cori_adapter_pg::introspect::introspect_schema_json(&db_url).await?;

            let diff = diff_snapshots(&old, &live);
            print_schema_diff(&diff);
        }

        SchemaCommand::Inspect { entity } => {
            if !snapshot_path.exists() {
                return Err(anyhow::anyhow!(
                    "Missing {}. Run `cori schema snapshot` first.",
                    snapshot_path.display()
                ));
            }
            let snap = read_json(&snapshot_path)?;
            inspect_snapshot(&snap, entity.as_deref())?;
        }
    }

    Ok(())
}

// -----------------------------
// generate commands
// -----------------------------

async fn run_generate(cmd: GenerateCommand) -> anyhow::Result<()> {
    match cmd {
        GenerateCommand::Actions { force } => run_generate_actions(force).await,
    }
}

async fn run_generate_actions(force: bool) -> anyhow::Result<()> {
    let cfg = load_cori_config_from_cwd()?;
    ensure_postgres_adapter(&cfg)?;

    let snapshot_path = PathBuf::from("schema").join("snapshot.json");
    if !snapshot_path.exists() {
        return Err(anyhow::anyhow!(
            "Missing {}. Run `cori schema snapshot` first.",
            snapshot_path.display()
        ));
    }

    let snap_v = read_json(&snapshot_path)?;
    let snap = parse_snapshot(&snap_v)?;

    fs::create_dir_all("actions")?;

    let catalog_path = PathBuf::from("actions").join("catalog.json");
    if catalog_path.exists() && !force {
        return Err(anyhow::anyhow!(
            "actions/catalog.json already exists. Use --force to overwrite."
        ));
    }

    // handle possible collisions: same table name in multiple schemas
    let mut name_counts: BTreeMap<String, usize> = BTreeMap::new();
    for t in &snap.tables {
        *name_counts.entry(t.name.clone()).or_insert(0) += 1;
    }

    let mut all_actions: Vec<ActionDefinition> = Vec::new();
    let mut written_files: Vec<String> = Vec::new();

    for t in &snap.tables {
        let table_key = format!("{}.{}", t.schema, t.name);
        let entity_base = if name_counts.get(&t.name).copied().unwrap_or(0) > 1 {
            pascal_case(&format!("{}_{}", t.schema, t.name))
        } else {
            pascal_case(&t.name)
        };

        let has_tenant = t.columns.iter().any(|c| c.name == "tenant_id");
        let has_version = t.columns.iter().any(|c| c.name == "version");
        let has_deleted_at = t.columns.iter().any(|c| c.name == "deleted_at");
        let has_deleted_by = t.columns.iter().any(|c| c.name == "deleted_by");
        let has_delete_reason = t.columns.iter().any(|c| c.name == "delete_reason");

        // Execution metadata for adapters (non-authoritative but generated deterministically).
        // This enables safe SQL construction without requiring re-introspection at runtime.
        let mut updatable_columns: Vec<String> = Vec::new();
        for c in &t.columns {
            if t.primary_key.contains(&c.name) {
                continue;
            }
            if c.name == "tenant_id"
                || c.name == "version"
                || c.name == "deleted_at"
                || c.name == "deleted_by"
                || c.name == "delete_reason"
            {
                continue;
            }
            updatable_columns.push(c.name.clone());
        }
        updatable_columns.sort();

        let meta = json!({
            "generated": true,
            "source_table": table_key,
            "pg": {
                "schema": t.schema,
                "table": t.name,
                "primary_key": t.primary_key,
                "tenant_column": if has_tenant { json!("tenant_id") } else { serde_json::Value::Null },
                "version_column": if has_version { json!("version") } else { serde_json::Value::Null },
                "deleted_at_column": if has_deleted_at { json!("deleted_at") } else { serde_json::Value::Null },
                "deleted_by_column": if has_deleted_by { json!("deleted_by") } else { serde_json::Value::Null },
                "delete_reason_column": if has_delete_reason { json!("delete_reason") } else { serde_json::Value::Null },
                "columns": t.columns.iter().map(|c| json!({
                    "name": c.name,
                    "data_type": c.data_type,
                    "nullable": c.nullable
                })).collect::<Vec<_>>(),
                "updatable_columns": updatable_columns
            }
        });

        // GetById (if PK exists)
        if !t.primary_key.is_empty() {
            let name = format!("Get{}ById", entity_base);
            let def = ActionDefinition {
                name: name.clone(),
                version: Some("0.1".into()),
                description: Some(format!("Generated action for {}", table_key)),
                kind: StepKind::Query,
                resource_kind: resource_kind_for(t, &entity_base),
                policy_action: "get".into(),
                input_schema: build_get_by_id_input_schema(t, has_tenant),
                effects: Some(vec!["Read one row by primary key.".to_string()]),
                meta: meta.clone(),
            };
            write_action_file(&def, &name, force)?;
            written_files.push(format!("actions/{}.action.json", name));
            all_actions.push(def);
        }

        // List
        {
            let name = format!("List{}", entity_base);
            let def = ActionDefinition {
                name: name.clone(),
                version: Some("0.1".into()),
                description: Some(format!("Generated action for {}", table_key)),
                kind: StepKind::Query,
                resource_kind: resource_kind_for(t, &entity_base),
                policy_action: "list".into(),
                input_schema: build_list_input_schema(t, has_tenant),
                effects: Some(vec!["List rows (paged).".to_string()]),
                meta: meta.clone(),
            };
            write_action_file(&def, &name, force)?;
            written_files.push(format!("actions/{}.action.json", name));
            all_actions.push(def);
        }

        // UpdateFields (if PK exists)
        if !t.primary_key.is_empty() {
            let name = format!("Update{}Fields", entity_base);
            let def = ActionDefinition {
                name: name.clone(),
                version: Some("0.1".into()),
                description: Some(format!("Generated action for {}", table_key)),
                kind: StepKind::Mutation,
                resource_kind: resource_kind_for(t, &entity_base),
                policy_action: "update_fields".into(),
                input_schema: build_update_fields_input_schema(t, has_tenant, has_version),
                effects: Some(vec![
                    "Update selected fields (patch) with optimistic concurrency if supported."
                        .to_string(),
                ]),
                meta: meta.clone(),
            };
            write_action_file(&def, &name, force)?;
            written_files.push(format!("actions/{}.action.json", name));
            all_actions.push(def);
        }

        // SoftDelete (if deleted_at exists and PK exists)
        if has_deleted_at && !t.primary_key.is_empty() {
            let name = format!("SoftDelete{}", entity_base);
            let effects = vec![
                "Set deleted_at to now().".to_string(),
                if has_deleted_by {
                    "Set deleted_by.".to_string()
                } else {
                    "deleted_by not present.".to_string()
                },
                if has_delete_reason {
                    "Set delete_reason.".to_string()
                } else {
                    "delete_reason not present.".to_string()
                },
                if has_version {
                    "Increment version.".to_string()
                } else {
                    "version not present.".to_string()
                },
            ];
            let def = ActionDefinition {
                name: name.clone(),
                version: Some("0.1".into()),
                description: Some(format!("Generated action for {}", table_key)),
                kind: StepKind::Mutation,
                resource_kind: resource_kind_for(t, &entity_base),
                policy_action: "soft_delete".into(),
                input_schema: build_soft_delete_input_schema(
                    t,
                    has_tenant,
                    has_version,
                    has_deleted_by,
                    has_delete_reason,
                ),
                effects: Some(effects),
                meta: meta.clone(),
            };
            write_action_file(&def, &name, force)?;
            written_files.push(format!("actions/{}.action.json", name));
            all_actions.push(def);
        }
    }

    // Write catalog
    let catalog = ActionsCatalog {
        actions: all_actions,
    };
    fs::write(&catalog_path, serde_json::to_vec_pretty(&catalog)?)?;
    println!("Wrote actions catalog: {}", catalog_path.display());
    println!("Generated {} actions:", written_files.len());
    for f in written_files {
        println!("  - {}", f);
    }

    Ok(())
}

// -----------------------------
// actions commands
// -----------------------------

async fn run_actions(cmd: ActionsCommand) -> anyhow::Result<()> {
    match cmd {
        ActionsCommand::List => {
            let catalog = load_actions_catalog()?;
            if catalog.actions.is_empty() {
                println!("No actions in actions/catalog.json");
                return Ok(());
            }
            println!("Actions ({}):", catalog.actions.len());
            for a in &catalog.actions {
                println!(
                    "  - {:<32} kind={:<8} resource={:<16} policy_action={}",
                    a.name,
                    match a.kind {
                        StepKind::Query => "query",
                        StepKind::Mutation => "mutation",
                        StepKind::Control => "control",
                    },
                    a.resource_kind,
                    a.policy_action
                );
            }
            Ok(())
        }

        ActionsCommand::Describe { action_name } => {
            // Prefer direct file name; fallback to scan.
            let def = load_action_by_name(&action_name)?;
            println!("{}", serde_json::to_string_pretty(&def)?);
            Ok(())
        }

        ActionsCommand::Validate => {
            let catalog = load_actions_catalog()?;

            // Basic catalog checks
            let mut names = BTreeSet::new();
            for a in &catalog.actions {
                if a.name.trim().is_empty() {
                    return Err(anyhow::anyhow!("Action with empty name found in catalog."));
                }
                if !names.insert(a.name.clone()) {
                    return Err(anyhow::anyhow!(
                        "Duplicate action name in catalog: {}",
                        a.name
                    ));
                }
                if !a.input_schema.is_object() {
                    return Err(anyhow::anyhow!(
                        "input_schema must be an object for action {}",
                        a.name
                    ));
                }
            }

            // Load snapshot table keys if present (for linking validation)
            let snapshot_path = PathBuf::from("schema").join("snapshot.json");
            let snapshot_tables: Option<BTreeSet<String>> = if snapshot_path.exists() {
                let snap_v = read_json(&snapshot_path)?;
                let snap = parse_snapshot(&snap_v)?;
                let mut s = BTreeSet::new();
                for t in &snap.tables {
                    s.insert(format!("{}.{}", t.schema, t.name));
                }
                Some(s)
            } else {
                None
            };

            // Validate each action file exists + parses + matches catalog version of the action
            let mut ok = 0usize;
            for a in &catalog.actions {
                let file_path = PathBuf::from("actions").join(format!("{}.action.json", a.name));
                if !file_path.exists() {
                    return Err(anyhow::anyhow!(
                        "Missing action file for {}: {}",
                        a.name,
                        file_path.display()
                    ));
                }

                let file_v = read_json(&file_path)?;
                let file_def: ActionDefinition = serde_json::from_value(file_v)?;
                if file_def.name != a.name {
                    return Err(anyhow::anyhow!(
                        "Action file name mismatch: file {} has name '{}', expected '{}'",
                        file_path.display(),
                        file_def.name,
                        a.name
                    ));
                }

                // Optional: validate linkage to snapshot via meta.source_table
                if let Some(tables) = &snapshot_tables
                    && let Some(source_table) = file_def
                        .meta
                        .get("source_table")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                        && !tables.contains(&source_table) {
                            return Err(anyhow::anyhow!(
                                "Action {} references meta.source_table='{}' but it is not in schema snapshot.",
                                a.name,
                                source_table
                            ));
                        }

                ok += 1;
            }

            println!("✔ Actions validated successfully.");
            println!("  - catalog actions: {}", catalog.actions.len());
            println!("  - validated files: {}", ok);
            if snapshot_tables.is_none() {
                println!(
                    "  (note) schema/snapshot.json not found; skipped source_table linkage checks."
                );
            }
            Ok(())
        }
    }
}

// -----------------------------
// plan commands
// -----------------------------

async fn run_plan(cmd: PlanCommand) -> anyhow::Result<()> {
    match cmd {
        PlanCommand::Validate { file } => run_plan_validate(&file).await,
        PlanCommand::Preview { file } => run_plan_preview(&file).await,
    }
}

/// Shared plan validation used by both `plan validate` and `plan preview`.
/// Returns (normalized_plan_json, errors)
fn validate_plan_against_catalog(
    plan_file: &Path,
    action_map: &BTreeMap<String, ActionDefinition>,
) -> anyhow::Result<(serde_json::Value, Vec<String>)> {
    let plan_v = read_plan_file_as_json(plan_file)?;
    let plan = normalize_plan_value(plan_v);

    let mut errors: Vec<String> = Vec::new();

    let steps = match plan.get("steps").and_then(|v| v.as_array()) {
        Some(s) if !s.is_empty() => s,
        _ => {
            return Ok((
                plan,
                vec!["Invalid plan: missing or empty 'steps' array.".into()],
            ));
        }
    };

    // Unique step IDs
    let mut step_ids = BTreeSet::new();
    for (i, step) in steps.iter().enumerate() {
        let sid = step.get("id").and_then(|v| v.as_str()).unwrap_or("");
        if sid.is_empty() {
            errors.push(format!("steps[{}].id is required (non-empty string)", i));
        } else if !step_ids.insert(sid.to_string()) {
            errors.push(format!("Duplicate step id: '{}'", sid));
        }
    }

    // Validate steps
    for (i, step) in steps.iter().enumerate() {
        let sid = step
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("<missing>");

        // Reject MVP-unsupported constructs
        if step.get("foreach").is_some() {
            errors.push(format!(
                "steps[{}] (id={}) contains 'foreach' (not supported yet)",
                i, sid
            ));
        }
        if step.get("paginate").is_some() {
            errors.push(format!(
                "steps[{}] (id={}) contains 'paginate' (not supported yet)",
                i, sid
            ));
        }

        // kind
        let kind = step.get("kind").and_then(|v| v.as_str());
        if kind.is_none() || !matches!(kind, Some("query" | "mutation" | "control")) {
            errors.push(format!(
                "steps[{}] (id={}) has invalid kind: expected query|mutation|control",
                i, sid
            ));
        }

        // depends_on references
        if let Some(dep) = step.get("depends_on") {
            if let Some(arr) = dep.as_array() {
                for d in arr {
                    if let Some(dep_id) = d.as_str() {
                        if !step_ids.contains(dep_id) {
                            errors.push(format!(
                                "steps[{}] (id={}) depends_on '{}' which does not exist",
                                i, sid, dep_id
                            ));
                        }
                    } else {
                        errors.push(format!(
                            "steps[{}] (id={}) depends_on must contain strings",
                            i, sid
                        ));
                    }
                }
            } else {
                errors.push(format!(
                    "steps[{}] (id={}) depends_on must be an array",
                    i, sid
                ));
            }
        }

        // action
        let action_name = match step.get("action").and_then(|v| v.as_str()) {
            Some(a) if !a.trim().is_empty() => a,
            _ => {
                errors.push(format!("steps[{}] (id={}) missing action", i, sid));
                continue;
            }
        };

        // inputs
        let inputs = match step.get("inputs") {
            Some(v) => v,
            None => {
                errors.push(format!("steps[{}] (id={}) missing inputs", i, sid));
                continue;
            }
        };
        if !inputs.is_object() {
            errors.push(format!(
                "steps[{}] (id={}) inputs must be an object",
                i, sid
            ));
            continue;
        }

        // action exists
        let def = match action_map.get(action_name) {
            Some(d) => d,
            None => {
                errors.push(format!(
                    "steps[{}] (id={}) unknown action '{}' (run `cori actions list`)",
                    i, sid, action_name
                ));
                continue;
            }
        };

        // kind match
        if let Some(k) = kind {
            let expected = match k {
                "query" => StepKind::Query,
                "mutation" => StepKind::Mutation,
                "control" => StepKind::Control,
                _ => StepKind::Control, // already recorded as invalid kind above
            };
            if expected != def.kind {
                errors.push(format!(
                    "steps[{}] (id={}) kind '{}' does not match action '{}' kind '{}'",
                    i,
                    sid,
                    k,
                    action_name,
                    match def.kind {
                        StepKind::Query => "query",
                        StepKind::Mutation => "mutation",
                        StepKind::Control => "control",
                    }
                ));
            }
        }

        // input schema validation
        if let Err(e) = validate_instance_against_schema(&def.input_schema, inputs) {
            errors.push(format!(
                "steps[{}] (id={}) inputs invalid for action '{}': {}",
                i, sid, action_name, e
            ));
        }
    }

    Ok((plan, errors))
}

async fn run_plan_validate(file: &Path) -> anyhow::Result<()> {
    // Ensure inside project
    let _cfg = load_cori_config_from_cwd()?;

    let catalog = load_actions_catalog()?;
    let action_map: BTreeMap<String, ActionDefinition> = catalog
        .actions
        .into_iter()
        .map(|a| (a.name.clone(), a))
        .collect();

    let (_plan, errors) = validate_plan_against_catalog(file, &action_map)?;

    if errors.is_empty() {
        println!("✔ Plan is valid.");
        println!("  - file: {}", file.display());
        return Ok(());
    }

    println!("✖ Plan is invalid ({} error(s)):", errors.len());
    for e in errors {
        println!("  - {}", e);
    }
    Err(anyhow::anyhow!("Plan validation failed"))
}

async fn run_plan_preview(file: &Path) -> anyhow::Result<()> {
    // Ensure inside project
    let cfg = load_cori_config_from_cwd()?;
    ensure_postgres_adapter(&cfg)?;

    // Load catalog (for validation + mapping to core steps)
    let catalog = load_actions_catalog()?;
    let action_map: BTreeMap<String, ActionDefinition> = catalog
        .actions
        .into_iter()
        .map(|a| (a.name.clone(), a))
        .collect();

    // Validate plan first
    let (plan_json, errors) = validate_plan_against_catalog(file, &action_map)?;
    if !errors.is_empty() {
        println!("✖ Plan is invalid ({} error(s)):", errors.len());
        for e in errors {
            println!("  - {}", e);
        }
        return Err(anyhow::anyhow!("Plan preview aborted: validation failed"));
    }

    // Resolve DB URL & create adapter (preview mode still needs adapter)
    let db_url = resolve_database_url(&cfg)?;
    let adapter = PostgresAdapter::new(&db_url, pg_adapter_options_from_cfg(&cfg)).await?;

    // Infer tenant_id from plan inputs (best-effort), else "default"
    let tenant_id = infer_tenant_id(&plan_json).unwrap_or_else(|| "default".to_string());

    // Environment from config (default dev)
    let environment = cfg.environment.clone().unwrap_or_else(|| "dev".to_string());

    // Build cori-core Plan from plan_json steps
    let core_plan = plan_json_to_core_plan(&plan_json)?;

    // Build intent
    let intent_id = format!("preview-{}", unix_millis());
    let intent = MutationIntent {
        schema_version: "0.1.0".to_string(),
        intent_id: intent_id.clone(),
        tenant_id: tenant_id.clone(),
        environment: environment.clone(),
        preview: true,
        principal: Principal {
            id: "user:local".to_string(),
            roles: vec!["admin".to_string()],
            attrs: json!({}),
        },
        plan: core_plan,
        request: serde_json::Value::Null,
        meta: serde_json::Value::Null,
    };

    let policy = build_policy_client(&cfg).await?;
    let core_actions = action_map.clone();

    // Memory audit sink to include in report
    let audit = MemoryAuditSink::new();
    let audit_handle = audit.clone();

    let orchestrator = Orchestrator::new(policy, adapter, audit, core_actions);
    let result = orchestrator.run(&intent).await?;

    // Build preview report
    let audit_events_json: Vec<serde_json::Value> = audit_handle
        .drain()
        .into_iter()
        .map(|e| serde_json::to_value(e).expect("audit event must be serializable"))
        .collect();

    let report = json!({
        "type": "cori_plan_preview",
        "file": file.display().to_string(),
        "intent": {
            "intent_id": intent_id,
            "tenant_id": tenant_id,
            "environment": environment,
            "preview": true
        },
        "result": result,
        "audit_events": audit_events_json
    });

    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}
async fn run_apply(file: &Path, preview: bool) -> anyhow::Result<()> {
    // Must be run inside a Cori project
    let cfg = load_cori_config_from_cwd()?;
    ensure_postgres_adapter(&cfg)?;

    // Load actions catalog -> map (used for plan validation)
    let catalog = load_actions_catalog()?;
    let action_map: BTreeMap<String, ActionDefinition> = catalog
        .actions
        .into_iter()
        .map(|a| (a.name.clone(), a))
        .collect();

    // Validate plan first
    let (plan_json, errors) = validate_plan_against_catalog(file, &action_map)?;
    if !errors.is_empty() {
        println!("✖ Plan is invalid ({} error(s)):", errors.len());
        for e in errors {
            println!("  - {}", e);
        }
        return Err(anyhow::anyhow!("Apply aborted: plan validation failed"));
    }

    // Best-effort derive tenant_id from plan inputs
    let tenant_id = infer_tenant_id(&plan_json).unwrap_or_else(|| "default".to_string());

    // Environment from config (default dev)
    let environment = cfg.environment.clone().unwrap_or_else(|| "dev".to_string());

    // Convert plan into cori-core Plan (persisted in intent.json)
    let core_plan = plan_json_to_core_plan(&plan_json)?;

    // Create intent (on disk)
    let intent_id = format!("intent-{}", Uuid::new_v4());
    let intent = MutationIntent {
        schema_version: "0.1.0".to_string(),
        intent_id: intent_id.clone(),
        tenant_id: tenant_id.clone(),
        environment: environment.clone(),
        preview,
        principal: Principal {
            id: "user:local".to_string(),
            roles: vec!["admin".to_string()],
            attrs: json!({}),
        },
        plan: core_plan,
        request: serde_json::Value::Null,
        meta: serde_json::Value::Null,
    };

    // Prepare intent directory
    let intents_root = PathBuf::from("intents");
    fs::create_dir_all(&intents_root)?;
    let intent_dir = intents_root.join(&intent_id);
    if intent_dir.exists() {
        return Err(anyhow::anyhow!(
            "Intent directory already exists: {}",
            intent_dir.display()
        ));
    }
    fs::create_dir_all(&intent_dir)?;

    // Persist intent + plan + meta
    fs::write(
        intent_dir.join("intent.json"),
        serde_json::to_vec_pretty(&intent)?,
    )?;
    fs::write(
        intent_dir.join("plan.json"),
        serde_json::to_vec_pretty(&plan_json)?,
    )?;
    fs::write(
        intent_dir.join("meta.json"),
        serde_json::to_vec_pretty(&json!({
            "source_file": file.display().to_string(),
            "preview": preview
        }))?,
    )?;

    // Seed audit.json with portable evidence events (schema-aligned).
    // Note: orchestrator will emit additional events when preview/execute runs.
    {
        let mut events: Vec<AuditEvent> = Vec::new();
        events.push(new_cli_audit_event(
            &intent,
            0,
            AuditEventType::IntentReceived,
            "__intent__",
            "__intent__",
            true,
            json!({"source":"cli.apply","file":file.display().to_string()}),
            json!({}),
        ));
        events.push(new_cli_audit_event(
            &intent,
            1,
            AuditEventType::PlanValidated,
            "__intent__",
            "__intent__",
            true,
            json!({"source":"cli.apply","file":file.display().to_string()}),
            json!({}),
        ));
        fs::write(
            intent_dir.join("audit.json"),
            serde_json::to_vec_pretty(&events)?,
        )?;
    }

    // Create initial status
    let mut status = IntentStatus {
        intent_id: intent_id.clone(),
        preview,
        state: if preview {
            IntentState::Running
        } else {
            IntentState::PendingApproval
        },
        created_at_ms: now_ms(),
        updated_at_ms: now_ms(),
        message: None,
        approved_at_ms: None,
        executed_at_ms: None,
        failed_at_ms: None,
    };

    // If not preview: do NOT execute now (GitOps flow). Save status + exit.
    if !preview {
        status.message = Some(format!(
            "Pending approval. Run: cori approve {} --reason \"...\"",
            intent_id
        ));
        write_status(&status)?;
        println!("✔ Created intent (pending approval): {}", intent_id);
        println!("Path: {}", intent_dir.display());
        println!("Next:");
        println!("  cori approve {} --reason \"<why>\"", intent_id);
        println!("  cori execute {}", intent_id);
        return Ok(());
    }

    // preview=true: execute orchestrator now and save a dry-run report
    write_status(&status)?; // state=Running

    // Adapter requires DB URL for preview
    let db_url = resolve_database_url(&cfg)?;
    let adapter = PostgresAdapter::new(&db_url, pg_adapter_options_from_cfg(&cfg)).await?;

    let policy = build_policy_client(&cfg).await?;
    let core_actions = action_map.clone();

    // Capture audit events for report
    let audit = MemoryAuditSink::new();
    let audit_handle = audit.clone();

    let orchestrator = Orchestrator::new(policy, adapter, audit, core_actions);

    let run_res = orchestrator.run(&intent).await;

    match run_res {
        Ok(result) => {
            // Merge existing (CLI-seeded) audit events with runtime-emitted ones,
            // and ensure `sequence` is monotonic across the merged list.
            let mut existing_events = read_audit_events(&intent_id).unwrap_or_default();
            let offset = existing_events.len() as u64;
            let mut runtime_events = audit_handle.drain();
            for (i, e) in runtime_events.iter_mut().enumerate() {
                e.sequence = Some(offset + i as u64);
            }
            existing_events.extend(runtime_events);

            let audit_events_json: Vec<serde_json::Value> = existing_events
                .iter()
                .cloned()
                .map(|e| serde_json::to_value(e).expect("audit event must be serializable"))
                .collect();

            let report = json!({
                "type": "cori_apply_preview",
                "file": file.display().to_string(),
                "intent": {
                    "intent_id": intent_id,
                    "tenant_id": tenant_id,
                    "environment": environment,
                    "preview": true
                },
                "result": result,
                "audit_events": audit_events_json
            });

            fs::write(
                intent_dir.join("result.json"),
                serde_json::to_vec_pretty(&report)?,
            )?;
            fs::write(
                intent_dir.join("audit.json"),
                serde_json::to_vec_pretty(&existing_events)?,
            )?;

            status.state = IntentState::Previewed;
            status.updated_at_ms = now_ms();
            status.message = Some("Preview completed".to_string());
            write_status(&status)?;

            println!("✔ Previewed intent: {}", status.intent_id);
            println!("Path: {}", intent_dir.display());
            println!("{}", serde_json::to_string_pretty(&report)?);
            Ok(())
        }
        Err(err) => {
            status.state = IntentState::Failed;
            status.updated_at_ms = now_ms();
            status.failed_at_ms = Some(status.updated_at_ms);
            status.message = Some(format!("Preview failed: {}", err));
            write_status(&status)?;
            Err(err)
        }
    }
}

async fn run_status(intent_id: &str) -> anyhow::Result<()> {
    validate_intent_id(intent_id)?;

    let dir = intent_dir(intent_id);
    if !dir.exists() {
        return Err(anyhow::anyhow!("Intent not found: {}", dir.display()));
    }

    let status = read_status(intent_id)?;
    let approval = read_approval(intent_id)?;
    let has_intent = intent_path(intent_id).exists();
    let has_plan = intent_dir(intent_id).join("plan.json").exists();
    let has_meta = intent_dir(intent_id).join("meta.json").exists();
    let has_status = status_path(intent_id).exists();
    let has_approval = approval_path(intent_id).exists();
    let has_result = result_path(intent_id).exists();
    let has_audit = audit_path(intent_id).exists();

    println!("Intent: {}", intent_id);
    println!("Path:   {}", dir.display());

    if let Some(s) = status {
        println!("State:  {:?}", s.state);
        println!("Preview: {}", s.preview);
        println!("Created: {} ms", s.created_at_ms);
        println!("Updated: {} ms", s.updated_at_ms);
        if let Some(m) = s.message.as_deref() {
            println!("Message: {}", m);
        }
        if let Some(t) = s.approved_at_ms {
            println!("Approved: {} ms", t);
        }
        if let Some(t) = s.executed_at_ms {
            println!("Executed: {} ms", t);
        }
        if let Some(t) = s.failed_at_ms {
            println!("Failed:   {} ms", t);
        }
    } else {
        println!("State:  (no status.json yet)");
    }

    if let Some(a) = approval {
        println!("Approval:");
        println!("  approver: {}", a.approver);
        println!("  reason:   {}", a.reason);
        println!("  at:       {} ms", a.approved_at_ms);
    } else {
        println!("Approval: (none)");
    }

    println!("Artifacts:");
    println!("  intent.json:   {}", has_intent);
    println!("  plan.json:     {}", has_plan);
    println!("  meta.json:     {}", has_meta);
    println!("  status.json:   {}", has_status);
    println!("  approval.json: {}", has_approval);
    println!("  result.json:   {}", has_result);
    println!("  audit.json:    {}", has_audit);

    if has_result {
        println!("Tip: open {}", result_path(intent_id).display());
    }

    Ok(())
}

async fn run_approve(
    intent_id: &str,
    reason: &str,
    as_principal: Option<&str>,
) -> anyhow::Result<()> {
    validate_intent_id(intent_id)?;
    if reason.trim().is_empty() {
        return Err(anyhow::anyhow!("--reason must be non-empty"));
    }

    let dir = intent_dir(intent_id);
    if !dir.exists() {
        return Err(anyhow::anyhow!("Intent not found: {}", dir.display()));
    }

    let intent = read_intent(intent_id)?;
    if intent.preview {
        return Err(anyhow::anyhow!(
            "This intent was created as preview=true; it cannot be approved/executed."
        ));
    }

    // If already executed, block
    if let Some(s) = read_status(intent_id)? {
        match s.state {
            IntentState::Executed => {
                return Err(anyhow::anyhow!("Intent already executed."));
            }
            IntentState::Running => {
                return Err(anyhow::anyhow!("Intent is currently running."));
            }
            _ => {}
        }
    }

    let approval = IntentApproval {
        intent_id: intent_id.to_string(),
        approved_at_ms: now_ms(),
        approver: as_principal.unwrap_or("user:local").to_string(),
        reason: reason.to_string(),
    };

    fs::write(
        approval_path(intent_id),
        serde_json::to_vec_pretty(&approval)?,
    )?;

    // Update status
    let mut status = read_status(intent_id)?.unwrap_or(IntentStatus {
        intent_id: intent_id.to_string(),
        preview: false,
        state: IntentState::PendingApproval,
        created_at_ms: now_ms(),
        updated_at_ms: now_ms(),
        message: None,
        approved_at_ms: None,
        executed_at_ms: None,
        failed_at_ms: None,
    });

    status.state = IntentState::Approved;
    status.preview = false;
    status.updated_at_ms = now_ms();
    status.approved_at_ms = Some(approval.approved_at_ms);
    status.message = Some(format!(
        "Approved by {}: {}",
        approval.approver, approval.reason
    ));
    write_status(&status)?;

    // Append portable audit evidence for approval.
    {
        let next_seq = read_audit_events(intent_id).unwrap_or_default().len() as u64;
        let ev = new_cli_audit_event_for(
            &intent,
            &approval.approver,
            next_seq,
            AuditEventType::Approved,
            "__intent__",
            "__intent__",
            true,
            json!({"source":"cli.approve"}),
            json!({
                "approver": approval.approver,
                "reason": approval.reason,
                "approved_at_ms": approval.approved_at_ms
            }),
        );
        append_audit_event(intent_id, ev)?;
    }

    println!("✔ Approved intent: {}", intent_id);
    Ok(())
}

async fn run_execute(intent_id: &str) -> anyhow::Result<()> {
    validate_intent_id(intent_id)?;

    let cfg = load_cori_config_from_cwd()?;
    ensure_postgres_adapter(&cfg)?;

    let dir = intent_dir(intent_id);
    if !dir.exists() {
        return Err(anyhow::anyhow!("Intent not found: {}", dir.display()));
    }

    let mut status = read_status(intent_id)?.unwrap_or(IntentStatus {
        intent_id: intent_id.to_string(),
        preview: false,
        state: IntentState::PendingApproval,
        created_at_ms: now_ms(),
        updated_at_ms: now_ms(),
        message: None,
        approved_at_ms: None,
        executed_at_ms: None,
        failed_at_ms: None,
    });

    let intent = read_intent(intent_id)?;
    if intent.preview {
        return Err(anyhow::anyhow!(
            "This intent was created as preview=true; it cannot be executed."
        ));
    }

    // Idempotency: if a result exists, show it and exit (even if status.json is missing/out-of-date).
    if result_path(intent_id).exists() {
        println!("✔ Result already exists. Showing existing result:");
        let v = read_json(&result_path(intent_id))?;
        println!("{}", serde_json::to_string_pretty(&v)?);
        return Ok(());
    }

    // Require approval
    let approval = read_approval(intent_id)?.ok_or_else(|| {
        anyhow::anyhow!(
            "Intent is not approved yet. Run: cori approve {} --reason \"...\"",
            intent_id
        )
    })?;

    // Mark running
    status.state = IntentState::Running;
    status.updated_at_ms = now_ms();
    status.message = Some(format!("Running (approved by {})", approval.approver));
    write_status(&status)?;

    // Execute via orchestrator
    let catalog = load_actions_catalog()?;
    let action_map: BTreeMap<String, ActionDefinition> = catalog
        .actions
        .into_iter()
        .map(|a| (a.name.clone(), a))
        .collect();
    let core_actions = action_map.clone();

    let db_url = resolve_database_url(&cfg)?;
    let adapter = PostgresAdapter::new(&db_url, pg_adapter_options_from_cfg(&cfg)).await?;
    let policy = build_policy_client(&cfg).await?;

    let audit = MemoryAuditSink::new();
    let audit_handle = audit.clone();

    let orchestrator = Orchestrator::new(policy, adapter, audit, core_actions);

    let run_res = orchestrator.run(&intent).await;

    match run_res {
        Ok(result) => {
            // Merge existing audit events (seeded/appended by CLI) with runtime-emitted ones,
            // and ensure `sequence` is monotonic across the merged list.
            let mut existing_events = read_audit_events(intent_id).unwrap_or_default();
            let offset = existing_events.len() as u64;
            let mut runtime_events = audit_handle.drain();
            for (i, e) in runtime_events.iter_mut().enumerate() {
                e.sequence = Some(offset + i as u64);
            }
            existing_events.extend(runtime_events);

            let audit_events_json: Vec<serde_json::Value> = existing_events
                .iter()
                .cloned()
                .map(|e| serde_json::to_value(e).expect("audit event must be serializable"))
                .collect();

            let report = json!({
                "type": "cori_execute",
                "intent": {
                    "intent_id": intent.intent_id,
                    "tenant_id": intent.tenant_id,
                    "environment": intent.environment,
                    "preview": false
                },
                "approval": {
                    "approver": approval.approver,
                    "reason": approval.reason,
                    "approved_at_ms": approval.approved_at_ms
                },
                "result": result,
                "audit_events": audit_events_json
            });

            fs::write(result_path(intent_id), serde_json::to_vec_pretty(&report)?)?;
            fs::write(
                audit_path(intent_id),
                serde_json::to_vec_pretty(&existing_events)?,
            )?;

            status.state = IntentState::Executed;
            status.updated_at_ms = now_ms();
            status.executed_at_ms = Some(status.updated_at_ms);
            status.message = Some("Executed successfully".to_string());
            write_status(&status)?;

            println!("✔ Executed intent: {}", intent_id);
            println!("{}", serde_json::to_string_pretty(&report)?);
            Ok(())
        }
        Err(err) => {
            status.state = IntentState::Failed;
            status.updated_at_ms = now_ms();
            status.failed_at_ms = Some(status.updated_at_ms);
            status.message = Some(format!("Execution failed: {}", err));
            write_status(&status)?;

            Err(err)
        }
    }
}

// -----------------------------
// helpers for preview
// -----------------------------

fn unix_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn infer_tenant_id(plan: &serde_json::Value) -> Option<String> {
    let steps = plan.get("steps")?.as_array()?;
    for s in steps {
        let inputs = s.get("inputs")?;
        if let Some(t) = inputs.get("tenant_id").and_then(|v| v.as_str())
            && !t.trim().is_empty() {
                return Some(t.to_string());
            }
    }
    None
}

fn value_to_map(v: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
    match v {
        serde_json::Value::Object(m) => m,
        _ => serde_json::Map::new(),
    }
}

fn new_cli_audit_event(
    intent: &MutationIntent,
    sequence: u64,
    event_type: AuditEventType,
    step_id: &str,
    action: &str,
    allowed: bool,
    decision: serde_json::Value,
    outcome: serde_json::Value,
) -> AuditEvent {
    new_cli_audit_event_for(
        intent,
        &intent.principal.id,
        sequence,
        event_type,
        step_id,
        action,
        allowed,
        decision,
        outcome,
    )
}

fn new_cli_audit_event_for(
    intent: &MutationIntent,
    principal_id: &str,
    sequence: u64,
    event_type: AuditEventType,
    step_id: &str,
    action: &str,
    allowed: bool,
    decision: serde_json::Value,
    outcome: serde_json::Value,
) -> AuditEvent {
    AuditEvent {
        event_id: Uuid::new_v4(),
        occurred_at: chrono::Utc::now(),
        sequence: Some(sequence),
        tenant_id: intent.tenant_id.clone(),
        intent_id: intent.intent_id.clone(),
        principal_id: principal_id.to_string(),
        step_id: step_id.to_string(),
        event_type,
        action: action.to_string(),
        resource_kind: None,
        resource_id: None,
        allowed,
        preview: Some(intent.preview),
        decision: value_to_map(decision),
        outcome: value_to_map(outcome),
        integrity: None,
        meta: Some(json!({
            "environment": intent.environment.clone()
        })),
    }
}

fn plan_json_to_core_plan(plan: &serde_json::Value) -> anyhow::Result<Plan> {
    let steps_v = plan
        .get("steps")
        .and_then(|v| v.as_array())
        .ok_or_else(|| anyhow::anyhow!("Invalid plan: missing steps array"))?;

    let mut steps = Vec::new();
    for (i, s) in steps_v.iter().enumerate() {
        let id = s
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("steps[{}].id missing", i))?
            .to_string();

        let kind_str = s
            .get("kind")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("steps[{}].kind missing", i))?;

        let kind = match kind_str {
            "query" => StepKind::Query,
            "mutation" => StepKind::Mutation,
            "control" => StepKind::Control,
            other => return Err(anyhow::anyhow!("steps[{}].kind invalid: {}", i, other)),
        };

        let action = s
            .get("action")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("steps[{}].action missing", i))?
            .to_string();

        let inputs = s
            .get("inputs")
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("steps[{}].inputs missing", i))?;

        let depends_on = s
            .get("depends_on")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|x| x.as_str().map(|s| s.to_string()))
                    .collect::<Vec<_>>()
            })
            .filter(|v| !v.is_empty());

        let meta = s.get("meta").cloned().unwrap_or(serde_json::Value::Null);

        steps.push(Step {
            id,
            kind,
            action,
            inputs,
            depends_on,
            meta,
        });
    }

    Ok(Plan {
        plan_id: plan
            .get("plan_id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        name: plan
            .get("name")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        summary: plan
            .get("summary")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        steps,
        meta: plan.get("meta").cloned().unwrap_or(serde_json::Value::Null),
    })
}

// -----------------------------
// MemoryAuditSink (captures runtime audit events for preview output)
// -----------------------------

#[derive(Clone)]
struct MemoryAuditSink {
    inner: Arc<Mutex<Vec<RtAuditEvent>>>,
}

impl MemoryAuditSink {
    fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn drain(&self) -> Vec<RtAuditEvent> {
        let mut g = self.inner.lock().unwrap();
        std::mem::take(&mut *g)
    }
}

impl AuditSink for MemoryAuditSink {
    fn record(&self, event: RtAuditEvent) {
        let mut g = self.inner.lock().unwrap();
        g.push(event);
    }
}

fn read_plan_file_as_json(path: &Path) -> anyhow::Result<serde_json::Value> {
    let bytes = fs::read(path)?;
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();

    if ext == "json" {
        Ok(serde_json::from_slice(&bytes)?)
    } else if ext == "yaml" || ext == "yml" {
        let s = String::from_utf8(bytes)?;
        let y: serde_yaml::Value = serde_yaml::from_str(&s)?;
        Ok(serde_json::to_value(y)?)
    } else {
        Err(anyhow::anyhow!(
            "Unsupported plan extension. Use .yaml/.yml or .json (got '{}')",
            ext
        ))
    }
}

/// Convenience:
/// - if plan is a list, treat it as steps: [ ... ]
/// - if plan is already an object with steps, keep as-is
fn normalize_plan_value(v: serde_json::Value) -> serde_json::Value {
    match v {
        serde_json::Value::Array(arr) => json!({ "steps": arr }),
        serde_json::Value::Object(_) => v,
        other => json!({ "steps": [other] }), // defensive; will fail later
    }
}

fn validate_instance_against_schema(
    schema: &serde_json::Value,
    instance: &serde_json::Value,
) -> anyhow::Result<()> {
    let validator = jsonschema::draft202012::options()
        .build(schema)
        .map_err(|e| anyhow::anyhow!("Invalid action input_schema: {}", e))?;

    if validator.is_valid(instance) {
        return Ok(());
    }

    // Keep output readable: show up to 10 errors
    let mut msgs = Vec::new();
    for (idx, e) in validator.iter_errors(instance).take(10).enumerate() {
        msgs.push(format!("{}: {}", idx + 1, e));
    }
    Err(anyhow::anyhow!(msgs.join("; ")))
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

fn read_json(path: &Path) -> anyhow::Result<serde_json::Value> {
    let bytes = fs::read(path)?;
    Ok(serde_json::from_slice(&bytes)?)
}

async fn build_policy_client(_cfg: &CoriConfig) -> anyhow::Result<Arc<dyn PolicyClient>> {
    // Biscuit-native policy model: policy enforcement happens at the proxy/RLS layer
    // based on Biscuit token claims and role YAML configuration.
    // This policy client exists for audit logging and backwards compatibility.
    Ok(Arc::new(cori_policy::BiscuitPolicyClient::new()))
}

fn pg_adapter_options_from_cfg(cfg: &CoriConfig) -> cori_adapter_pg::PostgresAdapterOptions {
    cori_adapter_pg::PostgresAdapterOptions {
        max_affected_rows: cfg.max_affected_rows.unwrap_or(1000),
        preview_row_limit: cfg.preview_row_limit.unwrap_or(25),
    }
}

// -----------------------------
// actions catalog + action defs
// -----------------------------

#[derive(Debug, Serialize, Deserialize)]
struct ActionsCatalog {
    actions: Vec<ActionDefinition>,
}

fn load_actions_catalog() -> anyhow::Result<ActionsCatalog> {
    let catalog_path = PathBuf::from("actions").join("catalog.json");
    if !catalog_path.exists() {
        return Err(anyhow::anyhow!(
            "Missing {}. Run `cori generate actions` first.",
            catalog_path.display()
        ));
    }
    let v = read_json(&catalog_path)?;
    Ok(serde_json::from_value(v)?)
}

fn load_action_by_name(name: &str) -> anyhow::Result<ActionDefinition> {
    let direct = PathBuf::from("actions").join(format!("{}.action.json", name));
    if direct.exists() {
        let v = read_json(&direct)?;
        return Ok(serde_json::from_value(v)?);
    }

    // Fallback: scan actions/*.action.json
    let dir = PathBuf::from("actions");
    if !dir.exists() {
        return Err(anyhow::anyhow!(
            "actions/ directory not found. Run `cori generate actions` first."
        ));
    }

    for entry in fs::read_dir(&dir)? {
        let entry = entry?;
        let p = entry.path();
        if p.is_file()
            && let Some(fname) = p.file_name().and_then(|s| s.to_str())
                && fname.ends_with(".action.json") {
                    let v = read_json(&p)?;
                    let def: ActionDefinition = serde_json::from_value(v)?;
                    if def.name == name {
                        return Ok(def);
                    }
                }
    }

    Err(anyhow::anyhow!(
        "Action '{}' not found. Try `cori actions list`.",
        name
    ))
}

fn write_action_file(def: &ActionDefinition, action_name: &str, force: bool) -> anyhow::Result<()> {
    let path = PathBuf::from("actions").join(format!("{}.action.json", action_name));
    if path.exists() && !force {
        return Err(anyhow::anyhow!(
            "{} already exists. Use --force to overwrite.",
            path.display()
        ));
    }
    fs::write(path, serde_json::to_vec_pretty(def)?)?;
    Ok(())
}

// -----------------------------
// schema snapshot model + diff/inspect
// -----------------------------

#[derive(Debug, Deserialize, Clone)]
struct Snapshot {
    tables: Vec<TableEntry>,
}

#[derive(Debug, Deserialize, Clone)]
#[allow(dead_code)]
struct TableEntry {
    schema: String,
    name: String,
    columns: Vec<ColumnEntry>,
    #[serde(default)]
    primary_key: Vec<String>,
    #[serde(default)]
    #[allow(dead_code)]
    foreign_keys: Vec<ForeignKeyEntry>,
}

#[derive(Debug, Deserialize, Clone)]
struct ColumnEntry {
    name: String,
    data_type: String,
    nullable: bool,
    #[serde(default)]
    default: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
#[allow(dead_code)]
struct ForeignKeyEntry {
    #[allow(dead_code)]
    name: String,
    #[serde(default)]
    #[allow(dead_code)]
    mappings: Vec<ForeignKeyMapping>,
}

#[derive(Debug, Deserialize, Clone)]
#[allow(dead_code)]
struct ForeignKeyMapping {
    #[allow(dead_code)]
    column: String,
    #[allow(dead_code)]
    references: ForeignKeyRef,
}

#[derive(Debug, Deserialize, Clone)]
#[allow(dead_code)]
struct ForeignKeyRef {
    #[allow(dead_code)]
    schema: String,
    #[allow(dead_code)]
    table: String,
    #[allow(dead_code)]
    column: String,
}

fn parse_snapshot(v: &serde_json::Value) -> anyhow::Result<Snapshot> {
    Ok(serde_json::from_value(v.clone())?)
}

#[derive(Debug)]
struct SchemaDiff {
    added_tables: Vec<String>,
    removed_tables: Vec<String>,
    changed_tables: Vec<TableDiff>,
}

#[derive(Debug)]
struct TableDiff {
    table: String,
    added_columns: Vec<String>,
    removed_columns: Vec<String>,
    changed_columns: Vec<ColumnChange>,
}

#[derive(Debug)]
struct ColumnChange {
    name: String,
    from_type: String,
    to_type: String,
    from_nullable: bool,
    to_nullable: bool,
    from_default: Option<String>,
    to_default: Option<String>,
}

fn diff_snapshots(old_v: &serde_json::Value, new_v: &serde_json::Value) -> SchemaDiff {
    let old = parse_snapshot(old_v).unwrap_or(Snapshot { tables: vec![] });
    let new = parse_snapshot(new_v).unwrap_or(Snapshot { tables: vec![] });

    let old_map = tables_to_map(&old);
    let new_map = tables_to_map(&new);

    let old_keys: BTreeSet<String> = old_map.keys().cloned().collect();
    let new_keys: BTreeSet<String> = new_map.keys().cloned().collect();

    let added_tables = new_keys.difference(&old_keys).cloned().collect::<Vec<_>>();

    let removed_tables = old_keys.difference(&new_keys).cloned().collect::<Vec<_>>();

    let mut changed_tables = Vec::new();

    for key in old_keys.intersection(&new_keys) {
        let old_t = &old_map[key];
        let new_t = &new_map[key];

        let old_cols = columns_to_map(old_t);
        let new_cols = columns_to_map(new_t);

        let old_col_keys: BTreeSet<String> = old_cols.keys().cloned().collect();
        let new_col_keys: BTreeSet<String> = new_cols.keys().cloned().collect();

        let added_columns = new_col_keys
            .difference(&old_col_keys)
            .cloned()
            .collect::<Vec<_>>();

        let removed_columns = old_col_keys
            .difference(&new_col_keys)
            .cloned()
            .collect::<Vec<_>>();

        let mut changed_columns = Vec::new();
        for ck in old_col_keys.intersection(&new_col_keys) {
            let o = &old_cols[ck];
            let n = &new_cols[ck];

            let type_changed = o.data_type != n.data_type;
            let null_changed = o.nullable != n.nullable;
            let def_changed = normalize_opt(&o.default) != normalize_opt(&n.default);

            if type_changed || null_changed || def_changed {
                changed_columns.push(ColumnChange {
                    name: ck.clone(),
                    from_type: o.data_type.clone(),
                    to_type: n.data_type.clone(),
                    from_nullable: o.nullable,
                    to_nullable: n.nullable,
                    from_default: o.default.clone(),
                    to_default: n.default.clone(),
                });
            }
        }

        if !(added_columns.is_empty() && removed_columns.is_empty() && changed_columns.is_empty()) {
            changed_tables.push(TableDiff {
                table: key.clone(),
                added_columns,
                removed_columns,
                changed_columns,
            });
        }
    }

    SchemaDiff {
        added_tables,
        removed_tables,
        changed_tables,
    }
}

fn normalize_opt(s: &Option<String>) -> Option<String> {
    s.as_ref()
        .map(|x| x.trim().to_string())
        .filter(|x| !x.is_empty())
}

fn tables_to_map(snapshot: &Snapshot) -> BTreeMap<String, TableEntry> {
    let mut m = BTreeMap::new();
    for t in &snapshot.tables {
        let key = format!("{}.{}", t.schema, t.name);
        m.insert(key, t.clone());
    }
    m
}

fn columns_to_map(table: &TableEntry) -> BTreeMap<String, ColumnEntry> {
    let mut m = BTreeMap::new();
    for c in &table.columns {
        m.insert(c.name.clone(), c.clone());
    }
    m
}

fn print_schema_diff(diff: &SchemaDiff) {
    println!("Schema diff (snapshot -> live):");

    if diff.added_tables.is_empty()
        && diff.removed_tables.is_empty()
        && diff.changed_tables.is_empty()
    {
        println!("  ✔ No differences detected.");
        return;
    }

    if !diff.added_tables.is_empty() {
        println!("\nTables added ({}):", diff.added_tables.len());
        for t in &diff.added_tables {
            println!("  + {}", t);
        }
    }

    if !diff.removed_tables.is_empty() {
        println!("\nTables removed ({}):", diff.removed_tables.len());
        for t in &diff.removed_tables {
            println!("  - {}", t);
        }
    }

    if !diff.changed_tables.is_empty() {
        println!("\nTables changed ({}):", diff.changed_tables.len());
        for td in &diff.changed_tables {
            println!("  * {}", td.table);

            if !td.added_columns.is_empty() {
                println!("    Columns added:");
                for c in &td.added_columns {
                    println!("      + {}", c);
                }
            }
            if !td.removed_columns.is_empty() {
                println!("    Columns removed:");
                for c in &td.removed_columns {
                    println!("      - {}", c);
                }
            }
            if !td.changed_columns.is_empty() {
                println!("    Columns changed:");
                for ch in &td.changed_columns {
                    println!(
                        "      ~ {}: type {} -> {}, nullable {} -> {}, default {:?} -> {:?}",
                        ch.name,
                        ch.from_type,
                        ch.to_type,
                        ch.from_nullable,
                        ch.to_nullable,
                        ch.from_default,
                        ch.to_default
                    );
                }
            }
        }
    }
}

fn inspect_snapshot(snapshot_v: &serde_json::Value, entity: Option<&str>) -> anyhow::Result<()> {
    let snap = parse_snapshot(snapshot_v)?;

    if entity.is_none() {
        println!("Entities in snapshot ({}):", snap.tables.len());
        for t in snap.tables {
            println!("  - {}.{} ({} columns)", t.schema, t.name, t.columns.len());
        }
        println!("\nUse: cori schema inspect --entity <table> or <schema.table>");
        return Ok(());
    }

    let entity = entity.unwrap();
    let matches = find_tables(&snap, entity);

    if matches.is_empty() {
        return Err(anyhow::anyhow!(
            "Entity '{}' not found in snapshot. Run `cori schema inspect` to list entities.",
            entity
        ));
    }

    if matches.len() > 1 {
        println!("Entity '{}' is ambiguous. Matches:", entity);
        for key in matches {
            println!("  - {}", key);
        }
        println!("Specify schema-qualified name: schema.table");
        return Ok(());
    }

    let key = &matches[0];
    let (schema, table) = split_key(key)?;
    let t = snap
        .tables
        .iter()
        .find(|x| x.schema == schema && x.name == table)
        .ok_or_else(|| anyhow::anyhow!("Internal error: resolved entity not found"))?;

    println!("Entity: {}.{}", t.schema, t.name);
    if !t.primary_key.is_empty() {
        println!("Primary key: {}", t.primary_key.join(", "));
    } else {
        println!("Primary key: (none detected)");
    }

    println!("\nColumns ({}):", t.columns.len());
    for c in &t.columns {
        println!(
            "  - {:<24} {:<24} nullable={} default={}",
            c.name,
            c.data_type,
            c.nullable,
            c.default.as_deref().unwrap_or("null")
        );
    }

    Ok(())
}

fn find_tables(snapshot: &Snapshot, entity: &str) -> Vec<String> {
    let entity = entity.trim();
    if entity.contains('.') {
        let parts: Vec<&str> = entity.splitn(2, '.').collect();
        if parts.len() != 2 {
            return vec![];
        }
        let sch = parts[0];
        let tbl = parts[1];
        snapshot
            .tables
            .iter()
            .filter(|t| t.schema == sch && t.name == tbl)
            .map(|t| format!("{}.{}", t.schema, t.name))
            .collect()
    } else {
        snapshot
            .tables
            .iter()
            .filter(|t| t.name == entity)
            .map(|t| format!("{}.{}", t.schema, t.name))
            .collect()
    }
}

fn split_key(key: &str) -> anyhow::Result<(&str, &str)> {
    let parts: Vec<&str> = key.splitn(2, '.').collect();
    if parts.len() != 2 {
        return Err(anyhow::anyhow!("Invalid table key '{}'", key));
    }
    Ok((parts[0], parts[1]))
}

// -----------------------------
// action generation helpers
// -----------------------------

fn resource_kind_for(t: &TableEntry, entity_base: &str) -> String {
    if entity_base
        .to_lowercase()
        .contains(&t.schema.to_lowercase())
        && entity_base.to_lowercase().contains(&t.name.to_lowercase())
    {
        format!("{}_{}", t.schema, t.name)
    } else {
        t.name.clone()
    }
}

fn pascal_case(s: &str) -> String {
    s.split(['_', '-', ' '])
        .filter(|p| !p.is_empty())
        .map(|p| {
            let mut chars = p.chars();
            match chars.next() {
                None => String::new(),
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
            }
        })
        .collect::<Vec<_>>()
        .join("")
}

fn build_object_schema(
    required: Vec<String>,
    properties: BTreeMap<String, serde_json::Value>,
) -> serde_json::Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": required,
        "properties": properties
    })
}

fn json_schema_type_for_pg(data_type: &str) -> serde_json::Value {
    match data_type {
        "uuid" => json!({ "type": "string", "format": "uuid" }),
        "text" | "character varying" | "character" | "varchar" => json!({ "type": "string" }),
        "boolean" => json!({ "type": "boolean" }),
        "integer" | "bigint" | "smallint" => json!({ "type": "integer" }),
        "numeric" | "real" | "double precision" | "decimal" => json!({ "type": "number" }),
        "date" => json!({ "type": "string", "format": "date" }),
        "timestamp without time zone" | "timestamp with time zone" => {
            json!({ "type": "string", "format": "date-time" })
        }
        "json" | "jsonb" => json!({ "type": "object" }),
        _ => json!({ "type": "string" }),
    }
}

fn build_get_by_id_input_schema(t: &TableEntry, has_tenant: bool) -> serde_json::Value {
    let mut req = Vec::new();
    let mut props = BTreeMap::new();

    if has_tenant {
        req.push("tenant_id".to_string());
        props.insert("tenant_id".to_string(), json!({ "type": "string" }));
    }

    for pk in &t.primary_key {
        req.push(pk.clone());
        let col = t.columns.iter().find(|c| &c.name == pk);
        let ty = col
            .map(|c| json_schema_type_for_pg(&c.data_type))
            .unwrap_or(json!({"type":"string"}));
        props.insert(pk.clone(), ty);
    }

    build_object_schema(req, props)
}

fn build_list_input_schema(t: &TableEntry, has_tenant: bool) -> serde_json::Value {
    let mut req = Vec::new();
    let mut props = BTreeMap::new();

    if has_tenant {
        req.push("tenant_id".to_string());
        props.insert("tenant_id".to_string(), json!({ "type": "string" }));
    }

    req.push("limit".to_string());
    props.insert(
        "limit".to_string(),
        json!({ "type": "integer", "minimum": 1, "maximum": 1000 }),
    );

    if t.primary_key.len() == 1 {
        let pk = &t.primary_key[0];
        let col = t.columns.iter().find(|c| &c.name == pk);
        let ty = col
            .map(|c| json_schema_type_for_pg(&c.data_type))
            .unwrap_or(json!({"type":"string"}));
        props.insert(
            "cursor".to_string(),
            json!({ "anyOf": [ { "type": "null" }, ty ], "default": null }),
        );
    } else {
        props.insert(
            "cursor".to_string(),
            json!({
                "anyOf": [ { "type": "null" }, { "type": "string" } ],
                "default": null
            }),
        );
    }

    build_object_schema(req, props)
}

fn build_update_fields_input_schema(
    t: &TableEntry,
    has_tenant: bool,
    has_version: bool,
) -> serde_json::Value {
    let mut req = Vec::new();
    let mut props = BTreeMap::new();

    if has_tenant {
        req.push("tenant_id".to_string());
        props.insert("tenant_id".to_string(), json!({ "type": "string" }));
    }

    for pk in &t.primary_key {
        req.push(pk.clone());
        let col = t.columns.iter().find(|c| &c.name == pk);
        let ty = col
            .map(|c| json_schema_type_for_pg(&c.data_type))
            .unwrap_or(json!({"type":"string"}));
        props.insert(pk.clone(), ty);
    }

    if has_version {
        props.insert(
            "expected_version".to_string(),
            json!({ "anyOf": [ { "type": "null" }, { "type": "integer" } ], "default": null }),
        );
    }

    props.insert(
        "reason".to_string(),
        json!({ "anyOf": [ { "type": "null" }, { "type": "string" } ], "default": null }),
    );

    let mut patch_props = BTreeMap::new();
    for c in &t.columns {
        if t.primary_key.contains(&c.name) {
            continue;
        }
        if c.name == "tenant_id"
            || c.name == "version"
            || c.name == "deleted_at"
            || c.name == "deleted_by"
            || c.name == "delete_reason"
        {
            continue;
        }
        let base = json_schema_type_for_pg(&c.data_type);
        let ty = if c.nullable {
            json!({ "anyOf": [ { "type": "null" }, base ] })
        } else {
            base
        };
        patch_props.insert(c.name.clone(), ty);
    }

    props.insert(
        "patch".to_string(),
        json!({
            "type": "object",
            "additionalProperties": false,
            "properties": patch_props
        }),
    );
    req.push("patch".to_string());

    build_object_schema(req, props)
}

fn build_soft_delete_input_schema(
    t: &TableEntry,
    has_tenant: bool,
    has_version: bool,
    has_deleted_by: bool,
    has_delete_reason: bool,
) -> serde_json::Value {
    let mut req = Vec::new();
    let mut props = BTreeMap::new();

    if has_tenant {
        req.push("tenant_id".to_string());
        props.insert("tenant_id".to_string(), json!({ "type": "string" }));
    }

    for pk in &t.primary_key {
        req.push(pk.clone());
        let col = t.columns.iter().find(|c| &c.name == pk);
        let ty = col
            .map(|c| json_schema_type_for_pg(&c.data_type))
            .unwrap_or(json!({"type":"string"}));
        props.insert(pk.clone(), ty);
    }

    if has_version {
        props.insert(
            "expected_version".to_string(),
            json!({ "anyOf": [ { "type": "null" }, { "type": "integer" } ], "default": null }),
        );
    }

    if has_deleted_by {
        req.push("deleted_by".to_string());
        props.insert(
            "deleted_by".to_string(),
            json!({ "type": "string", "minLength": 1 }),
        );
    } else {
        props.insert(
            "deleted_by".to_string(),
            json!({ "anyOf": [ { "type": "null" }, { "type": "string" } ], "default": null }),
        );
    }

    if has_delete_reason {
        props.insert(
            "reason".to_string(),
            json!({ "anyOf": [ { "type": "null" }, { "type": "string" } ], "default": null }),
        );
    } else {
        props.insert(
            "reason".to_string(),
            json!({ "anyOf": [ { "type": "null" }, { "type": "string" } ], "default": null }),
        );
    }

    build_object_schema(req, props)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum IntentState {
    PendingApproval,
    Approved,
    Running,
    Previewed,
    Executed,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct IntentStatus {
    intent_id: String,
    preview: bool,
    state: IntentState,
    created_at_ms: u128,
    updated_at_ms: u128,
    message: Option<String>,
    approved_at_ms: Option<u128>,
    executed_at_ms: Option<u128>,
    failed_at_ms: Option<u128>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct IntentApproval {
    intent_id: String,
    approved_at_ms: u128,
    approver: String,
    reason: String,
}

fn intents_dir() -> PathBuf {
    PathBuf::from("intents")
}

fn intent_dir(intent_id: &str) -> PathBuf {
    intents_dir().join(intent_id)
}

fn status_path(intent_id: &str) -> PathBuf {
    intent_dir(intent_id).join("status.json")
}

fn approval_path(intent_id: &str) -> PathBuf {
    intent_dir(intent_id).join("approval.json")
}

fn intent_path(intent_id: &str) -> PathBuf {
    intent_dir(intent_id).join("intent.json")
}

fn result_path(intent_id: &str) -> PathBuf {
    intent_dir(intent_id).join("result.json")
}

fn audit_path(intent_id: &str) -> PathBuf {
    intent_dir(intent_id).join("audit.json")
}

/// Prevent path traversal / weird IDs.
/// Allow only [A-Za-z0-9_-] and require non-empty.
fn validate_intent_id(intent_id: &str) -> anyhow::Result<()> {
    if intent_id.is_empty() {
        return Err(anyhow::anyhow!("intent_id must be non-empty"));
    }
    if !intent_id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(anyhow::anyhow!(
            "Invalid intent_id '{}'. Use only letters, digits, '_' or '-'.",
            intent_id
        ));
    }
    Ok(())
}

fn now_ms() -> u128 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn read_status(intent_id: &str) -> anyhow::Result<Option<IntentStatus>> {
    let p = status_path(intent_id);
    if !p.exists() {
        return Ok(None);
    }
    let v = read_json(&p)?;
    Ok(Some(serde_json::from_value(v)?))
}

fn write_status(status: &IntentStatus) -> anyhow::Result<()> {
    let p = status_path(&status.intent_id);
    fs::write(p, serde_json::to_vec_pretty(status)?)?;
    Ok(())
}

fn read_intent(intent_id: &str) -> anyhow::Result<MutationIntent> {
    let p = intent_path(intent_id);
    if !p.exists() {
        return Err(anyhow::anyhow!(
            "Missing {} (intent not found).",
            p.display()
        ));
    }
    let v = read_json(&p)?;
    Ok(serde_json::from_value(v)?)
}

fn read_approval(intent_id: &str) -> anyhow::Result<Option<IntentApproval>> {
    let p = approval_path(intent_id);
    if !p.exists() {
        return Ok(None);
    }
    let v = read_json(&p)?;
    Ok(Some(serde_json::from_value(v)?))
}

fn read_audit_events(intent_id: &str) -> anyhow::Result<Vec<AuditEvent>> {
    let p = audit_path(intent_id);
    if !p.exists() {
        return Ok(Vec::new());
    }
    let bytes = fs::read(&p)?;
    // audit.json is a JSON array of AuditEvent objects.
    Ok(serde_json::from_slice::<Vec<AuditEvent>>(&bytes)?)
}

fn write_audit_events(intent_id: &str, events: &[AuditEvent]) -> anyhow::Result<()> {
    fs::write(audit_path(intent_id), serde_json::to_vec_pretty(events)?)?;
    Ok(())
}

fn append_audit_event(intent_id: &str, event: AuditEvent) -> anyhow::Result<()> {
    let mut events = read_audit_events(intent_id)?;
    events.push(event);
    write_audit_events(intent_id, &events)?;
    Ok(())
}
