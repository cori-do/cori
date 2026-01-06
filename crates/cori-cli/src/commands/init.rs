//! `cori init` command implementation.
//!
//! Scaffolds a new Cori MCP server project by introspecting an existing
//! PostgreSQL database schema. Creates the project structure, configuration files,
//! Biscuit keypair, and sample role definitions.

use anyhow::{Context, Result};
use serde_json::Value as JsonValue;
use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

/// Well-known tenant column names (checked in priority order)
const TENANT_COLUMN_CANDIDATES: &[&str] = &[
    "tenant_id",
    "organization_id",
    "org_id",
    "customer_id",
    "account_id",
    "client_id",
];

/// Sensitive tables that should be blocked by default
const DEFAULT_BLOCKED_TABLES: &[&str] = &[
    "users",
    "api_keys",
    "billing",
    "audit_logs",
    "sessions",
    "tokens",
    "secrets",
    "credentials",
    "password_resets",
];

/// Run the `cori init` command.
///
/// # Arguments
/// * `database_url` - PostgreSQL connection URL
/// * `project` - Project name (used as output directory)
/// * `force` - Overwrite existing directory if true
pub async fn run(database_url: &str, project: &str, force: bool) -> Result<()> {
    let project_dir = PathBuf::from(project);

    // Check if project directory exists
    if project_dir.exists() {
        if !force {
            anyhow::bail!(
                "Project directory '{}' already exists. Use --force to overwrite.",
                project
            );
        }
        println!("⚠️  Overwriting existing project directory...");
    }

    println!("🚀 Initializing Cori project: {}", project);
    println!();

    // Create directory structure
    println!("📁 Creating project structure...");
    create_project_structure(&project_dir)?;

    // Generate Biscuit keypair
    println!("🔑 Generating Biscuit keypair...");
    let keys_dir = project_dir.join("keys");
    generate_keypair(&keys_dir)?;

    // Introspect database schema
    println!("🔍 Introspecting database schema...");
    let schema = cori_adapter_pg::introspect::introspect_schema_json(database_url)
        .await
        .context("Failed to introspect database schema")?;

    // Write schema snapshot
    let schema_path = project_dir.join("schema").join("snapshot.json");
    fs::write(&schema_path, serde_json::to_vec_pretty(&schema)?)?;
    println!("   ✓ Schema snapshot written to schema/snapshot.json");

    // Analyze schema for tenant columns and FK relationships
    let tables = extract_tables(&schema);
    let tenant_column = detect_tenant_column(&tables);
    println!("   ✓ Detected {} tables", tables.len());

    // Count FK relationships for reporting
    let total_fks: usize = tables.iter().map(|t| t.foreign_keys.len()).sum();
    if total_fks > 0 {
        println!("   ✓ Detected {} foreign key relationships", total_fks);
    }

    if let Some(ref col) = tenant_column {
        println!("   ✓ Auto-detected tenant column: {}", col);
    } else {
        println!("   ⚠ No tenant column auto-detected (configure manually in tenancy.yaml)");
    }

    // Analyze tenancy including FK chain resolution
    println!("🔗 Analyzing tenancy and FK relationships...");
    let tenancy_analysis = analyze_tenancy(&tables, &tenant_column);

    // Report tenancy findings
    let direct_count = tenancy_analysis
        .table_tenancy
        .values()
        .filter(|s| matches!(s, TenancySource::Direct { .. }))
        .count();
    let fk_count = tenancy_analysis
        .table_tenancy
        .values()
        .filter(|s| matches!(s, TenancySource::ViaForeignKey { .. }))
        .count();
    let global_count = tenancy_analysis
        .table_tenancy
        .values()
        .filter(|s| matches!(s, TenancySource::Global))
        .count();
    let unknown_count = tenancy_analysis
        .table_tenancy
        .values()
        .filter(|s| matches!(s, TenancySource::Unknown))
        .count();

    println!("   ✓ {} tables with direct tenant column", direct_count);
    if fk_count > 0 {
        println!(
            "   ✓ {} tables with tenant via FK chain (will use JOINs)",
            fk_count
        );
    }
    if global_count > 0 {
        println!("   ✓ {} global tables (no tenant isolation)", global_count);
    }
    if unknown_count > 0 {
        println!(
            "   ⚠ {} tables need manual tenancy configuration",
            unknown_count
        );
    }

    // Print FK chain warnings
    for warning in &tenancy_analysis.warnings {
        println!("   {}", warning);
    }

    // Generate tenancy.yaml
    let tenancy_yaml = generate_tenancy_yaml(&tenancy_analysis);
    let tenancy_path = project_dir.join("tenancy.yaml");
    fs::write(&tenancy_path, &tenancy_yaml)?;
    println!("   ✓ Tenancy configuration written to tenancy.yaml");

    // Identify sensitive tables
    let sensitive_tables = identify_sensitive_tables(&tables);
    if !sensitive_tables.is_empty() {
        println!(
            "   ✓ Identified {} potentially sensitive tables",
            sensitive_tables.len()
        );
    }

    // Generate configuration file
    println!("⚙️  Generating configuration...");
    let config = generate_config(project, &tenant_column, &sensitive_tables);
    let config_path = project_dir.join("cori.yaml");
    fs::write(&config_path, config)?;
    println!("   ✓ Configuration written to cori.yaml");

    // Generate sample roles
    println!("👥 Generating sample roles...");
    let roles_dir = project_dir.join("roles");
    generate_sample_roles(&roles_dir, &tables, &tenant_column, &sensitive_tables)?;
    println!("   ✓ Sample roles written to roles/");

    // Generate README
    let readme_path = project_dir.join("README.md");
    let readme = generate_readme(project, &tenant_column, &tenancy_analysis);
    fs::write(&readme_path, readme)?;
    println!("   ✓ README.md generated");

    // Print summary
    println!();
    println!("✅ Cori project initialized successfully!");
    println!();
    println!("📂 Project structure:");
    println!("   {}/", project);
    println!("   ├── cori.yaml           # Main configuration");
    println!("   ├── tenancy.yaml        # Tenant column mapping & FK chains");
    println!("   ├── keys/               # Biscuit keypair (keep private.key secret!)");
    println!("   │   ├── private.key");
    println!("   │   └── public.key");
    println!("   ├── roles/              # Role definitions");
    println!("   │   ├── admin_agent.yaml");
    println!("   │   ├── readonly_agent.yaml");
    println!("   │   └── support_agent.yaml");
    println!("   ├── schema/             # Schema snapshots");
    println!("   │   └── snapshot.json");
    println!("   └── tokens/             # Generated tokens (gitignore these)");
    println!();
    println!("🚦 Next steps:");
    println!("   1. cd {}", project);
    println!("   2. Review tenancy.yaml - especially tables with FK-inherited tenancy");
    println!("   3. Review and customize roles in roles/*.yaml");
    println!("   4. Set DATABASE_URL environment variable");
    println!("   5. Start the server: cori serve --config cori.yaml");
    println!("   6. Mint a token: cori token mint --role support_agent --tenant <tenant_id> --expires 24h");
    println!();
    println!("📚 See README.md for detailed instructions.");

    Ok(())
}

/// Create the project directory structure.
fn create_project_structure(project_dir: &PathBuf) -> Result<()> {
    let dirs = ["schema", "roles", "keys", "tokens"];

    for dir in dirs {
        fs::create_dir_all(project_dir.join(dir))?;
    }

    // Create .gitignore for sensitive files
    let gitignore = r#"# Cori project gitignore

# Private keys - NEVER commit these!
keys/private.key

# Generated tokens
tokens/*.token
tokens/*.biscuit

# Environment files with secrets
.env
.env.*

# Local development
*.local.yaml
"#;
    fs::write(project_dir.join(".gitignore"), gitignore)?;

    Ok(())
}

/// Generate Biscuit Ed25519 keypair.
fn generate_keypair(keys_dir: &PathBuf) -> Result<()> {
    use cori_biscuit::KeyPair;

    let keypair = KeyPair::generate()?;

    let private_key = keypair.private_key_hex();
    let public_key = keypair.public_key_hex();

    fs::write(keys_dir.join("private.key"), &private_key)?;
    fs::write(keys_dir.join("public.key"), &public_key)?;

    Ok(())
}

/// Extract table information from schema snapshot.
fn extract_tables(schema: &JsonValue) -> Vec<TableInfo> {
    let mut tables = Vec::new();

    if let Some(table_array) = schema.get("tables").and_then(|t| t.as_array()) {
        for table in table_array {
            let name = table
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or_default()
                .to_string();
            let schema_name = table
                .get("schema")
                .and_then(|s| s.as_str())
                .unwrap_or("public")
                .to_string();

            let columns: Vec<ColumnInfo> = table
                .get("columns")
                .and_then(|c| c.as_array())
                .map(|cols| {
                    cols.iter()
                        .filter_map(|c| {
                            let col_name = c.get("name")?.as_str()?.to_string();
                            let data_type = c.get("data_type")?.as_str()?.to_string();
                            let nullable = c
                                .get("nullable")
                                .and_then(|n| n.as_bool())
                                .unwrap_or(true);
                            Some(ColumnInfo {
                                name: col_name,
                                data_type,
                                nullable,
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();

            let primary_key: Vec<String> = table
                .get("primary_key")
                .and_then(|pk| pk.as_array())
                .map(|pks| {
                    pks.iter()
                        .filter_map(|p| p.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();

            // Extract foreign key information
            let foreign_keys: Vec<ForeignKeyInfo> = table
                .get("foreign_keys")
                .and_then(|fks| fks.as_array())
                .map(|fk_list| {
                    fk_list
                        .iter()
                        .flat_map(|fk| {
                            let fk_name = fk
                                .get("name")
                                .and_then(|n| n.as_str())
                                .unwrap_or_default()
                                .to_string();
                            fk.get("mappings")
                                .and_then(|m| m.as_array())
                                .map(|mappings| {
                                    mappings
                                        .iter()
                                        .filter_map(|mapping| {
                                            let column =
                                                mapping.get("column")?.as_str()?.to_string();
                                            let refs = mapping.get("references")?;
                                            let ref_table =
                                                refs.get("table")?.as_str()?.to_string();
                                            let ref_column =
                                                refs.get("column")?.as_str()?.to_string();
                                            Some(ForeignKeyInfo {
                                                name: fk_name.clone(),
                                                column,
                                                references_table: ref_table,
                                                references_column: ref_column,
                                            })
                                        })
                                        .collect::<Vec<_>>()
                                })
                                .unwrap_or_default()
                        })
                        .collect()
                })
                .unwrap_or_default();

            tables.push(TableInfo {
                schema: schema_name,
                name,
                columns,
                primary_key,
                foreign_keys,
            });
        }
    }

    tables
}

/// Detect the most likely tenant column across all tables.
fn detect_tenant_column(tables: &[TableInfo]) -> Option<String> {
    for candidate in TENANT_COLUMN_CANDIDATES {
        let count = tables
            .iter()
            .filter(|t| t.columns.iter().any(|c| c.name == *candidate))
            .count();

        // If at least half the tables have this column, it's likely the tenant column
        if count > 0 && count >= tables.len() / 2 {
            return Some(candidate.to_string());
        }
    }

    // Also check for columns containing "tenant", "org", etc.
    let mut column_counts: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    for table in tables {
        for col in &table.columns {
            let lower = col.name.to_lowercase();
            if lower.contains("tenant")
                || lower.contains("org")
                || (lower.contains("account") && lower.contains("id"))
            {
                *column_counts.entry(col.name.clone()).or_insert(0) += 1;
            }
        }
    }

    column_counts
        .into_iter()
        .filter(|(_, count)| *count >= tables.len() / 2)
        .max_by_key(|(_, count)| *count)
        .map(|(name, _)| name)
}

/// Analyze tenancy for all tables, detecting both direct tenant columns and FK chains.
///
/// This function:
/// 1. Identifies tables with direct tenant columns
/// 2. For tables without direct tenant columns, traces FK relationships to find tenant context
/// 3. Highlights tables that need manual configuration or review
fn analyze_tenancy(tables: &[TableInfo], tenant_column: &Option<String>) -> TenancyAnalysis {
    use std::collections::HashMap;

    let tenant_col = tenant_column
        .clone()
        .unwrap_or_else(|| "organization_id".to_string());

    // Build a lookup map for tables
    let table_map: HashMap<&str, &TableInfo> = tables.iter().map(|t| (t.name.as_str(), t)).collect();

    let mut table_tenancy: HashMap<String, TenancySource> = HashMap::new();
    let mut warnings: Vec<String> = Vec::new();

    // Detect tenant type from the first table with the tenant column
    let tenant_type = tables
        .iter()
        .find_map(|t| {
            t.columns
                .iter()
                .find(|c| c.name == tenant_col)
                .map(|c| normalize_tenant_type(&c.data_type))
        })
        .unwrap_or_else(|| "uuid".to_string());

    // First pass: identify tables with direct tenant columns
    for table in tables {
        if let Some(col) = table.columns.iter().find(|c| c.name == tenant_col) {
            table_tenancy.insert(
                table.name.clone(),
                TenancySource::Direct {
                    column: tenant_col.clone(),
                    data_type: normalize_tenant_type(&col.data_type),
                },
            );
        }
    }

    // Second pass: for tables without direct tenant columns, trace FK chains
    for table in tables {
        if table_tenancy.contains_key(&table.name) {
            continue; // Already has direct tenant column
        }

        // Try to find tenant context via FK chain
        match find_tenant_via_fk(table, &table_map, &tenant_col, &table_tenancy) {
            Some(source) => {
                match &source {
                    TenancySource::ViaForeignKey { chain, .. } => {
                        let chain_desc: Vec<String> =
                            chain.iter().map(|(t, c)| format!("{}.{}", t, c)).collect();
                        warnings.push(format!(
                            "Table '{}' has tenant context via FK chain: {} → {}",
                            table.name,
                            table.name,
                            chain_desc.join(" → ")
                        ));
                    }
                    _ => {}
                }
                table_tenancy.insert(table.name.clone(), source);
            }
            None => {
                // Check if this looks like a global/reference table
                if is_likely_global_table(&table.name, table) {
                    table_tenancy.insert(table.name.clone(), TenancySource::Global);
                } else {
                    warnings.push(format!(
                        "⚠️  Table '{}' has no tenant column '{}' and no FK path to tenant. \
                         Configure as 'global: true' or add tenant_column manually.",
                        table.name, tenant_col
                    ));
                    table_tenancy.insert(table.name.clone(), TenancySource::Unknown);
                }
            }
        }
    }

    TenancyAnalysis {
        tenant_type,
        tenant_column: Some(tenant_col),
        table_tenancy,
        warnings,
    }
}

/// Normalize SQL data type to tenancy type (uuid, integer, string)
fn normalize_tenant_type(data_type: &str) -> String {
    let lower = data_type.to_lowercase();
    if lower.contains("uuid") {
        "uuid".to_string()
    } else if lower.contains("int") {
        "integer".to_string()
    } else {
        "string".to_string()
    }
}

/// Try to find tenant context for a table via foreign key chains.
/// Uses BFS to find the shortest path to a table with a direct tenant column.
fn find_tenant_via_fk(
    table: &TableInfo,
    table_map: &std::collections::HashMap<&str, &TableInfo>,
    _tenant_col: &str,
    direct_tenant_tables: &std::collections::HashMap<String, TenancySource>,
) -> Option<TenancySource> {
    use std::collections::{HashSet, VecDeque};

    // BFS to find shortest FK path to a tenant-aware table
    // Queue items: (current_table_name, fk_column_in_original_table, path_so_far)
    let mut queue: VecDeque<(String, String, Vec<(String, String)>)> = VecDeque::new();
    let mut visited: HashSet<String> = HashSet::new();

    // Initialize with direct FKs from this table
    for fk in &table.foreign_keys {
        queue.push_back((
            fk.references_table.clone(),
            fk.column.clone(),
            vec![(fk.references_table.clone(), fk.references_column.clone())],
        ));
    }
    visited.insert(table.name.clone());

    // Limit chain length to prevent infinite loops and overly complex chains
    const MAX_CHAIN_LENGTH: usize = 3;

    while let Some((current_table, fk_column, path)) = queue.pop_front() {
        if path.len() > MAX_CHAIN_LENGTH {
            continue;
        }

        if visited.contains(&current_table) {
            continue;
        }
        visited.insert(current_table.clone());

        // Check if this table has a direct tenant column
        if let Some(TenancySource::Direct { column, .. }) = direct_tenant_tables.get(&current_table)
        {
            return Some(TenancySource::ViaForeignKey {
                fk_column,
                chain: path,
                tenant_column: column.clone(),
            });
        }

        // Continue traversing FKs from this table
        if let Some(ref_table) = table_map.get(current_table.as_str()) {
            for fk in &ref_table.foreign_keys {
                if !visited.contains(&fk.references_table) {
                    let mut new_path = path.clone();
                    new_path
                        .push((fk.references_table.clone(), fk.references_column.clone()));
                    queue.push_back((fk.references_table.clone(), fk_column.clone(), new_path));
                }
            }
        }
    }

    None
}

/// Heuristic to detect likely global/reference tables that don't need tenant isolation.
fn is_likely_global_table(name: &str, table: &TableInfo) -> bool {
    let lower = name.to_lowercase();

    // Common global/reference table patterns
    let global_patterns = [
        "country",
        "countries",
        "currency",
        "currencies",
        "language",
        "languages",
        "timezone",
        "timezones",
        "region",
        "regions",
        "status",
        "statuses",
        "type",
        "types",
        "category",
        "categories",
        "permission",
        "permissions",
        "role",
        "roles",
        "setting",
        "settings",
        "config",
        "configuration",
        "enum",
        "lookup",
        "reference",
        "ref_",
        "_ref",
        "master_",
        "_master",
    ];

    // Check name patterns
    if global_patterns.iter().any(|p| lower.contains(p)) {
        return true;
    }

    // Small tables with no FKs are often lookup tables
    if table.foreign_keys.is_empty() && table.columns.len() <= 5 {
        return true;
    }

    false
}

/// Generate tenancy.yaml content from analysis.
fn generate_tenancy_yaml(analysis: &TenancyAnalysis) -> String {
    let tenant_col = analysis
        .tenant_column
        .clone()
        .unwrap_or_else(|| "organization_id".to_string());

    let mut direct_tables = Vec::new();
    let mut fk_tables = Vec::new();
    let mut global_tables = Vec::new();
    let mut unknown_tables = Vec::new();

    // Sort tables into categories
    let mut sorted_tables: Vec<_> = analysis.table_tenancy.iter().collect();
    sorted_tables.sort_by_key(|(name, _)| name.as_str());

    for (table_name, source) in sorted_tables {
        match source {
            TenancySource::Direct { column, data_type } => {
                direct_tables.push(format!(
                    r#"  {}:
    tenant_column: {}
    tenant_type: {}"#,
                    table_name, column, data_type
                ));
            }
            TenancySource::ViaForeignKey {
                fk_column, chain, ..
            } => {
                let via_path = format!("{}.{}", chain[0].0, chain[0].1);
                fk_tables.push(format!(
                    r#"  {}:
    # No direct tenant column - tenant context inherited via FK
    tenant_via: {}
    fk_column: {}
    # NOTE: Queries on this table will need JOIN to enforce tenant isolation"#,
                    table_name, via_path, fk_column
                ));
            }
            TenancySource::Global => {
                global_tables.push(format!("  {}:\n    global: true", table_name));
            }
            TenancySource::Unknown => {
                unknown_tables.push(format!(
                    r#"  # {}:
    # ⚠️  NEEDS CONFIGURATION - no tenant column or FK path detected
    # Options:
    #   - Add 'tenant_column: <column_name>' if it has tenant data
    #   - Add 'global: true' if this is shared reference data
    #   - Add 'tenant_via: <parent_table>.<fk_column>' for FK inheritance"#,
                    table_name
                ));
            }
        }
    }

    let mut sections = Vec::new();

    // Direct tenant tables section
    if !direct_tables.is_empty() {
        sections.push(format!(
            r#"  # ---------------------------------------------------------------------------
  # Tables with direct tenant column
  # ---------------------------------------------------------------------------
{}"#,
            direct_tables.join("\n\n")
        ));
    }

    // FK-inherited tables section
    if !fk_tables.is_empty() {
        sections.push(format!(
            r#"
  # ---------------------------------------------------------------------------
  # Tables with tenant context via foreign key (requires JOIN for RLS)
  # ---------------------------------------------------------------------------
{}"#,
            fk_tables.join("\n\n")
        ));
    }

    // Global tables section
    if !global_tables.is_empty() {
        sections.push(format!(
            r#"
  # ---------------------------------------------------------------------------
  # Global tables (shared across all tenants)
  # ---------------------------------------------------------------------------
{}"#,
            global_tables.join("\n\n")
        ));
    }

    // Unknown tables section
    if !unknown_tables.is_empty() {
        sections.push(format!(
            r#"
  # ---------------------------------------------------------------------------
  # Tables requiring manual configuration
  # ---------------------------------------------------------------------------
{}"#,
            unknown_tables.join("\n\n")
        ));
    }

    format!(
        r#"# =============================================================================
# Tenancy Configuration
# =============================================================================
# This file declares HOW multi-tenancy is structured for this database.
# Generated by: cori init
#
# It defines:
#   - The tenant identifier type
#   - Which column holds tenant IDs in each table
#   - Tables that inherit tenant via FK relationships
#   - Which tables are global (shared across tenants)
#
# This is separate from roles (access control) — tenancy is about database
# structure, roles are about who can do what.
# =============================================================================

# =============================================================================
# TENANT IDENTIFIER
# =============================================================================
# The canonical tenant identifier used in Biscuit tokens and RLS predicates.

tenant_id:
  type: {}
  description: "Tenant identifier"

# =============================================================================
# TABLE CONFIGURATION
# =============================================================================

tables:
{}

# =============================================================================
# AUTO-DETECTION (Fallback)
# =============================================================================
# For tables not explicitly configured, Cori will look for these columns.

auto_detect:
  enabled: true
  columns:
    - {}
    - tenant_id
    - org_id
    - customer_id
    - account_id
"#,
        analysis.tenant_type,
        sections.join("\n"),
        tenant_col
    )
}

/// Identify potentially sensitive tables that should be blocked by default.
fn identify_sensitive_tables(tables: &[TableInfo]) -> Vec<String> {
    let mut sensitive = Vec::new();
    let table_names: HashSet<String> = tables.iter().map(|t| t.name.clone()).collect();

    // Check against known sensitive table names
    for name in DEFAULT_BLOCKED_TABLES {
        if table_names.contains(*name) {
            sensitive.push(name.to_string());
        }
    }

    // Also look for tables with sensitive-sounding names
    for table in tables {
        let lower = table.name.to_lowercase();
        if (lower.contains("password")
            || lower.contains("secret")
            || lower.contains("credential")
            || lower.contains("auth")
            || lower.contains("token")
            || lower.contains("key")
            || lower.contains("private"))
            && !sensitive.contains(&table.name)
        {
            sensitive.push(table.name.clone());
        }

        // Check for tables with sensitive columns
        let has_sensitive_columns = table.columns.iter().any(|c| {
            let lower = c.name.to_lowercase();
            lower.contains("password")
                || lower.contains("secret")
                || lower.contains("hash")
                || lower.contains("salt")
                || lower == "ssn"
                || lower.contains("credit_card")
                || lower.contains("card_number")
        });
        if has_sensitive_columns && !sensitive.contains(&table.name) {
            // Don't auto-block, but it's worth noting
        }
    }

    sensitive
}

/// Generate the cori.yaml configuration file content.
fn generate_config(
    project: &str,
    tenant_column: &Option<String>,
    sensitive_tables: &[String],
) -> String {
    let tenant_col = tenant_column
        .clone()
        .unwrap_or_else(|| "organization_id".to_string());

    let blocked_tables_yaml = if sensitive_tables.is_empty() {
        "  # No sensitive tables detected - add any that should be globally blocked\n  # - users\n  # - api_keys".to_string()
    } else {
        sensitive_tables
            .iter()
            .map(|t| format!("    - {}", t))
            .collect::<Vec<_>>()
            .join("\n")
    };

    format!(
        r#"# =============================================================================
# Cori MCP Server Configuration
# =============================================================================
# Project: {project}
# Generated by: cori init
#
# This configuration defines how Cori protects your database:
#   - Multi-tenant isolation via cryptographic Biscuit tokens
#   - Role-based access control with fine-grained permissions
#   - MCP protocol for AI agent integration
#   - Audit logging
# =============================================================================

project: {project}
version: "1.0"

# =============================================================================
# UPSTREAM DATABASE
# =============================================================================
# The PostgreSQL database that Cori protects.

upstream:
  # Recommended: use environment variable (don't commit secrets!)
  credentials_env: DATABASE_URL
  
  # Or explicit configuration (for local development only)
  # host: localhost
  # port: 5432
  # database: mydb
  # username: postgres
  # password: postgres

# =============================================================================
# BISCUIT TOKEN CONFIGURATION
# =============================================================================
# Authentication via cryptographic Biscuit tokens.
# Keys generated in keys/ directory during init.

biscuit:
  public_key_file: keys/public.key
  private_key_file: keys/private.key
  # Or via environment variables:
  # public_key_env: BISCUIT_PUBLIC_KEY
  # private_key_env: BISCUIT_PRIVATE_KEY
  require_expiration: true
  max_token_lifetime: "30d"

# =============================================================================
# TENANT ISOLATION
# =============================================================================
# Multi-tenant isolation configuration.
# Detailed table-by-table configuration is in tenancy.yaml

tenancy_file: tenancy.yaml

# Quick reference (see tenancy.yaml for full configuration):
#   - Default tenant column: {tenant_col}
#   - Tables with FK-inherited tenancy will use JOINs
#   - Global tables (no tenant isolation) are marked in tenancy.yaml

# =============================================================================
# VIRTUAL SCHEMA
# =============================================================================
# Control what AI agents see in schema introspection.

virtual_schema:
  enabled: true
  
  # Tables to hide from schema introspection
  always_hidden:
{blocked_tables_yaml}
  
  expose_row_counts: false
  expose_indexes: false

# =============================================================================
# ROLES
# =============================================================================
# Load role definitions from the roles/ directory.
# Each .yaml file defines one role with table/column permissions.

roles_dir: roles

# Or list specific role files:
# role_files:
#   - roles/support_agent.yaml
#   - roles/readonly_agent.yaml

# =============================================================================
# MCP SERVER (Model Context Protocol)
# =============================================================================
# Expose database actions as typed tools for AI agents.

mcp:
  enabled: true
  transport: stdio
  http_port: 8989  # if transport: http
  auto_generate_tools: true
  dry_run_enabled: true

# =============================================================================
# ADMIN DASHBOARD
# =============================================================================

dashboard:
  enabled: true
  listen_port: 8080
  auth:
    type: basic
    users:
      - username: admin
        password_env: DASHBOARD_ADMIN_PASSWORD

# =============================================================================
# GUARDRAILS
# =============================================================================
# Global safety limits for all queries.

guardrails:
  max_rows_per_query: 10000
  max_affected_rows: 1000
  blocked_operations:
    - TRUNCATE
    - DROP
    - ALTER
    - CREATE
    - GRANT
    - REVOKE

# =============================================================================
# AUDIT LOGGING
# =============================================================================

audit:
  enabled: true
  log_queries: true
  log_results: false  # Don't log actual data
  log_errors: true
  retention_days: 90
  output:
    - type: file
      path: logs/audit.jsonl
"#,
        project = project,
        tenant_col = tenant_col,
        blocked_tables_yaml = blocked_tables_yaml
    )
}

/// Generate sample role definition files.
fn generate_sample_roles(
    roles_dir: &PathBuf,
    tables: &[TableInfo],
    tenant_column: &Option<String>,
    sensitive_tables: &[String],
) -> Result<()> {
    // Filter out sensitive tables for non-admin roles
    let accessible_tables: Vec<&TableInfo> = tables
        .iter()
        .filter(|t| !sensitive_tables.contains(&t.name))
        .collect();

    // Generate admin role (full access)
    let admin_role = generate_admin_role(tables, tenant_column);
    fs::write(roles_dir.join("admin_agent.yaml"), admin_role)?;

    // Generate readonly role (read all non-sensitive tables)
    let readonly_role = generate_readonly_role(&accessible_tables, sensitive_tables);
    fs::write(roles_dir.join("readonly_agent.yaml"), readonly_role)?;

    // Generate support role (common customer-facing tables)
    let support_role = generate_support_role(&accessible_tables, sensitive_tables);
    fs::write(roles_dir.join("support_agent.yaml"), support_role)?;

    Ok(())
}

/// Generate admin role with full access.
fn generate_admin_role(tables: &[TableInfo], _tenant_column: &Option<String>) -> String {
    let table_entries: Vec<String> = tables
        .iter()
        .map(|t| {
            format!(
                r#"  {}:
    readable: "*"
    editable: "*""#,
                t.name
            )
        })
        .collect();

    format!(
        r#"# =============================================================================
# ADMIN AGENT ROLE
# =============================================================================
# Full administrative access to all tables and columns.
# Use with extreme caution - typically for internal tools only.

name: admin_agent
description: "Administrative agent with full database access"

tables:
{}

blocked_tables: []

max_rows_per_query: 10000
max_affected_rows: 1000

blocked_operations: []
"#,
        table_entries.join("\n\n")
    )
}

/// Generate readonly role.
fn generate_readonly_role(tables: &[&TableInfo], sensitive_tables: &[String]) -> String {
    let table_entries: Vec<String> = tables
        .iter()
        .take(20) // Limit to first 20 tables for readability
        .map(|t| {
            let columns: Vec<String> = t.columns.iter().map(|c| c.name.clone()).collect();
            format!(
                r#"  {}:
    readable: [{}]
    editable: {{}}"#,
                t.name,
                columns.join(", ")
            )
        })
        .collect();

    let blocked_yaml = if sensitive_tables.is_empty() {
        "blocked_tables: []".to_string()
    } else {
        format!(
            "blocked_tables:\n{}",
            sensitive_tables
                .iter()
                .map(|t| format!("  - {}", t))
                .collect::<Vec<_>>()
                .join("\n")
        )
    };

    format!(
        r#"# =============================================================================
# READONLY AGENT ROLE
# =============================================================================
# Read-only access to non-sensitive tables.
# Suitable for analytics and reporting agents.

name: readonly_agent
description: "Read-only agent for analytics and reporting"

tables:
{}

{}

max_rows_per_query: 5000
max_affected_rows: 0

blocked_operations:
  - INSERT
  - UPDATE
  - DELETE
  - TRUNCATE
"#,
        table_entries.join("\n\n"),
        blocked_yaml
    )
}

/// Generate support agent role.
fn generate_support_role(tables: &[&TableInfo], sensitive_tables: &[String]) -> String {
    // Common customer-facing table patterns
    let customer_tables = ["customers", "customer", "contacts", "contact", "clients", "client"];
    let ticket_tables = ["tickets", "ticket", "issues", "issue", "support_tickets", "cases", "case"];
    let order_tables = ["orders", "order", "purchases", "transactions"];
    let common_tables = ["products", "product", "services", "service", "notes", "comments"];

    let all_patterns: Vec<&str> = customer_tables
        .iter()
        .chain(ticket_tables.iter())
        .chain(order_tables.iter())
        .chain(common_tables.iter())
        .copied()
        .collect();

    let support_tables: Vec<&&TableInfo> = tables
        .iter()
        .filter(|t| {
            let lower = t.name.to_lowercase();
            all_patterns.iter().any(|p| lower.contains(p))
        })
        .collect();

    let table_entries: Vec<String> = if support_tables.is_empty() {
        // If no common tables found, use first few tables
        tables
            .iter()
            .take(5)
            .map(|t| {
                let columns: Vec<String> = t.columns.iter().map(|c| c.name.clone()).collect();
                format!(
                    r#"  {}:
    readable: [{}]
    editable: {{}}"#,
                    t.name,
                    columns.join(", ")
                )
            })
            .collect()
    } else {
        support_tables
            .iter()
            .map(|t| {
                let columns: Vec<String> = t.columns.iter().map(|c| c.name.clone()).collect();
                // Check if this looks like a tickets table (allow status edits)
                let is_ticket = ticket_tables
                    .iter()
                    .any(|p| t.name.to_lowercase().contains(p));

                if is_ticket && t.columns.iter().any(|c| c.name == "status") {
                    format!(
                        r#"  {}:
    readable: [{}]
    editable:
      status:
        allowed_values: [open, in_progress, pending, resolved, closed]"#,
                        t.name,
                        columns.join(", ")
                    )
                } else {
                    format!(
                        r#"  {}:
    readable: [{}]
    editable: {{}}"#,
                        t.name,
                        columns.join(", ")
                    )
                }
            })
            .collect()
    };

    let blocked_yaml = if sensitive_tables.is_empty() {
        "blocked_tables: []".to_string()
    } else {
        format!(
            "blocked_tables:\n{}",
            sensitive_tables
                .iter()
                .map(|t| format!("  - {}", t))
                .collect::<Vec<_>>()
                .join("\n")
        )
    };

    format!(
        r#"# =============================================================================
# SUPPORT AGENT ROLE
# =============================================================================
# Customer support agent with access to customer-facing data.
# Can view most data, limited write access to tickets/issues.

name: support_agent
description: "AI agent for customer support operations"

tables:
{}

{}

max_rows_per_query: 100
max_affected_rows: 10

blocked_operations:
  - DELETE
  - TRUNCATE
"#,
        table_entries.join("\n\n"),
        blocked_yaml
    )
}

/// Generate README content.
fn generate_readme(
    project: &str,
    tenant_column: &Option<String>,
    tenancy_analysis: &TenancyAnalysis,
) -> String {
    let tenant_col = tenant_column
        .clone()
        .unwrap_or_else(|| "organization_id".to_string());

    // Generate FK chain documentation if there are any
    let fk_tables: Vec<_> = tenancy_analysis
        .table_tenancy
        .iter()
        .filter_map(|(name, source)| {
            if let TenancySource::ViaForeignKey { fk_column, chain, .. } = source {
                let chain_desc: Vec<String> =
                    chain.iter().map(|(t, c)| format!("{}.{}", t, c)).collect();
                Some((name.clone(), fk_column.clone(), chain_desc.join(" → ")))
            } else {
                None
            }
        })
        .collect();

    let fk_section = if fk_tables.is_empty() {
        String::new()
    } else {
        let mut rows = String::from("### FK-Inherited Tenant Isolation\n\n");
        rows.push_str("Some tables don't have a direct tenant column but inherit tenant context via foreign key relationships.\n");
        rows.push_str("Cori will use JOINs to enforce tenant isolation on these tables:\n\n");
        rows.push_str("| Table | FK Column | Path to Tenant |\n");
        rows.push_str("|-------|-----------|----------------|\n");
        for (table, fk_col, chain) in &fk_tables {
            rows.push_str(&format!("| `{}` | `{}` | {} |\n", table, fk_col, chain));
        }
        rows.push_str("\n**Example:** When querying a table with FK-inherited tenancy:\n");
        rows.push_str("```sql\n");
        rows.push_str("-- Original query\n");
        rows.push_str(&format!("SELECT * FROM {} WHERE status = 'active'\n", fk_tables.first().map(|(t, _, _)| t.as_str()).unwrap_or("child_table")));
        rows.push_str("\n-- Cori rewrites to (using JOIN for tenant isolation)\n");
        if let Some((table, fk_col, _)) = fk_tables.first() {
            let parent = tenancy_analysis
                .table_tenancy
                .get(table)
                .and_then(|s| {
                    if let TenancySource::ViaForeignKey { chain, .. } = s {
                        chain.first().map(|(t, c)| (t.clone(), c.clone()))
                    } else {
                        None
                    }
                })
                .unwrap_or_else(|| ("parent".to_string(), "id".to_string()));
            rows.push_str(&format!(
                "SELECT t.* FROM {} t\n  JOIN {} p ON t.{} = p.{}\n  WHERE t.status = 'active' AND p.{} = 'acme_corp'\n",
                table, parent.0, fk_col, parent.1, tenant_col
            ));
        }
        rows.push_str("```\n");
        rows
    };

    format!(
        r#"# {project}

A Cori MCP server project for secure, tenant-isolated database access.

## Overview

This project was initialized from an existing database schema using `cori init`.
Cori acts as a security layer between AI agents and your PostgreSQL database,
enforcing tenant isolation and role-based access control via the Model Context Protocol (MCP).

## Project Structure

```
{project}/
├── cori.yaml           # Main configuration
├── tenancy.yaml        # Tenant column mapping & FK chains
├── keys/               # Biscuit keypair
│   ├── private.key     # ⚠️ Keep secret! Don't commit!
│   └── public.key
├── roles/              # Role definitions
│   ├── admin_agent.yaml
│   ├── readonly_agent.yaml
│   └── support_agent.yaml
├── schema/             # Schema snapshots
│   └── snapshot.json
└── tokens/             # Generated tokens (gitignored)
```

## Quick Start

### 1. Configure Database Connection

Set the database URL via environment variable:

```bash
export DATABASE_URL="postgres://user:password@host:5432/database"
```

### 2. Review Role Definitions

Edit the role files in `roles/` to match your access control requirements:

- `admin_agent.yaml` - Full access (use sparingly)
- `readonly_agent.yaml` - Read-only analytics access
- `support_agent.yaml` - Customer support operations

### 3. Start the Server

```bash
cori serve --config cori.yaml
```

This starts:
- MCP server (HTTP on port **8989** or stdio mode)
- Admin dashboard on port **8080**

### 4. Mint Tokens

Create a role token (base token without tenant restriction):

```bash
cori token mint --role support_agent --output tokens/support_role.token
```

Attenuate to a specific tenant:

```bash
cori token attenuate \
    --base tokens/support_role.token \
    --tenant acme_corp \
    --expires 24h \
    --output tokens/acme_support.token
```

Or mint a tenant-specific token directly:

```bash
cori token mint \
    --role support_agent \
    --tenant acme_corp \
    --expires 24h \
    --output tokens/acme_support.token
```

## Tenant Isolation

All queries are automatically scoped to the tenant specified in the token.

**Configured tenant column:** `{tenant_col}`

When an agent executes a tool:
```json
{{"tool": "listCustomers", "arguments": {{"status": "active"}}}}
```

Cori automatically adds tenant filtering:
```sql
SELECT * FROM customers WHERE status = 'active' AND {tenant_col} = 'acme_corp'
```

{fk_section}

## MCP (Model Context Protocol)

Cori exposes typed database actions as MCP tools for AI agents.

### Claude Desktop Configuration

Add to `claude_desktop_config.json`:

```json
{{
  "mcpServers": {{
    "cori": {{
      "command": "cori",
      "args": ["mcp", "serve", "--config", "cori.yaml"],
      "env": {{"CORI_TOKEN": "<base64 token>"}}
    }}
  }}
}}
```

### HTTP Transport

For agents that prefer HTTP, start with `--http`:

```bash
cori serve --config cori.yaml
# MCP HTTP server on port 8989
```

## Commands Reference

| Command | Description |
|---------|-------------|
| `cori serve` | Start the MCP server and dashboard |
| `cori mcp serve` | Start MCP server (stdio mode) |
| `cori keys generate` | Generate new Biscuit keypair |
| `cori token mint` | Mint a new token |
| `cori token attenuate` | Attenuate a token to a tenant |
| `cori token inspect` | Inspect token claims |
| `cori token verify` | Verify token validity |
| `cori schema snapshot` | Capture current schema |
| `cori schema diff` | Compare schema to snapshot |

## Security Notes

1. **Never commit `keys/private.key`** - It's gitignored by default
2. **Use short token lifetimes** - Default is 24h, adjust as needed
3. **Review role permissions** - Generated roles are starting points
4. **Review `tenancy.yaml`** - Especially tables with FK-inherited tenancy
5. **Enable TLS in production** - Use a reverse proxy like nginx or Caddy

## Documentation

- [Cori Documentation](https://github.com/your-org/cori)
- [Biscuit Tokens](https://www.biscuitsec.org/)
- [MCP Specification](https://modelcontextprotocol.io/)
"#,
        project = project,
        tenant_col = tenant_col,
        fk_section = fk_section
    )
}

// -----------------------------------------------------------------------------
// Helper types
// -----------------------------------------------------------------------------

#[derive(Debug, Clone)]
#[allow(dead_code)]
struct TableInfo {
    schema: String,
    name: String,
    columns: Vec<ColumnInfo>,
    primary_key: Vec<String>,
    foreign_keys: Vec<ForeignKeyInfo>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
struct ColumnInfo {
    name: String,
    data_type: String,
    nullable: bool,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
struct ForeignKeyInfo {
    name: String,
    column: String,
    references_table: String,
    references_column: String,
}

/// Describes how a table gets its tenant context
#[derive(Debug, Clone)]
#[allow(dead_code)]
enum TenancySource {
    /// Table has a direct tenant column
    Direct { column: String, data_type: String },
    /// Table inherits tenant via FK chain
    ViaForeignKey {
        /// FK column in this table
        fk_column: String,
        /// Chain of (table, column) pairs to reach tenant
        chain: Vec<(String, String)>,
        /// Final tenant column at the end of the chain
        tenant_column: String,
    },
    /// Table is global (no tenant isolation)
    Global,
    /// Cannot determine tenancy (needs manual config)
    Unknown,
}

/// Analysis result for tenancy configuration
#[derive(Debug)]
struct TenancyAnalysis {
    /// Detected tenant column type (uuid, integer, string)
    tenant_type: String,
    /// Detected tenant column name
    tenant_column: Option<String>,
    /// Per-table tenancy source
    table_tenancy: std::collections::HashMap<String, TenancySource>,
    /// Tables requiring attention (indirect FK, needs manual review)
    warnings: Vec<String>,
}
