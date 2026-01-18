//! `cori check` command implementation.
//!
//! Validates configuration files for consistency and correctness:
//! - JSON Schema validation against schemas in `schemas/`
//! - Cross-file consistency checks (tables, columns, groups, etc.)
//! - Warning detection for best practices

use anyhow::{Context, Result};
use serde_json::Value as JsonValue;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use cori_core::config::{
    ApprovalConfig, ApprovalRequirement, CoriConfig, CreatableColumns, DeletablePermission,
    GroupDefinition, ReadableConfig, UpdatableColumns,
};

// ============================================================================
// Embedded JSON Schemas
// ============================================================================

/// Embedded JSON schemas for configuration validation.
/// These are compiled into the binary so validation works without external files.
mod embedded_schemas {
    pub const CORI_DEFINITION: &str =
        include_str!("../../../../schemas/CoriDefinition.schema.json");
    pub const SCHEMA_DEFINITION: &str =
        include_str!("../../../../schemas/SchemaDefinition.schema.json");
    pub const RULES_DEFINITION: &str =
        include_str!("../../../../schemas/RulesDefinition.schema.json");
    pub const TYPES_DEFINITION: &str =
        include_str!("../../../../schemas/TypesDefinition.schema.json");
    pub const ROLE_DEFINITION: &str =
        include_str!("../../../../schemas/RoleDefinition.schema.json");
    pub const GROUP_DEFINITION: &str =
        include_str!("../../../../schemas/GroupDefinition.schema.json");
}

// ============================================================================
// Check Result Types
// ============================================================================

/// Severity level for check results.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// Informational message.
    Info,
    /// Warning - may indicate a potential issue.
    Warning,
    /// Error - configuration is invalid.
    Error,
}

impl std::fmt::Display for Severity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Severity::Info => write!(f, "INFO"),
            Severity::Warning => write!(f, "WARN"),
            Severity::Error => write!(f, "ERROR"),
        }
    }
}

/// A single check finding.
#[derive(Debug, Clone)]
pub struct CheckFinding {
    /// Severity of the finding.
    pub severity: Severity,
    /// Category of the check that produced this finding.
    pub category: String,
    /// Human-readable message describing the finding.
    pub message: String,
    /// Optional file path where the issue was found.
    pub file: Option<PathBuf>,
    /// Optional location within the file (e.g., "tables.customers.readable").
    pub location: Option<String>,
}

impl CheckFinding {
    fn error(category: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Error,
            category: category.into(),
            message: message.into(),
            file: None,
            location: None,
        }
    }

    fn warning(category: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Warning,
            category: category.into(),
            message: message.into(),
            file: None,
            location: None,
        }
    }

    #[allow(dead_code)]
    fn info(category: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Info,
            category: category.into(),
            message: message.into(),
            file: None,
            location: None,
        }
    }

    fn with_file(mut self, file: impl Into<PathBuf>) -> Self {
        self.file = Some(file.into());
        self
    }

    fn with_location(mut self, location: impl Into<String>) -> Self {
        self.location = Some(location.into());
        self
    }
}

/// Results from running all checks.
#[derive(Debug, Default)]
pub struct CheckResults {
    pub findings: Vec<CheckFinding>,
}

impl CheckResults {
    fn new() -> Self {
        Self {
            findings: Vec::new(),
        }
    }

    #[allow(dead_code)]
    fn add(&mut self, finding: CheckFinding) {
        self.findings.push(finding);
    }

    fn extend(&mut self, findings: impl IntoIterator<Item = CheckFinding>) {
        self.findings.extend(findings);
    }

    /// Returns true if there are any errors.
    pub fn has_errors(&self) -> bool {
        self.findings.iter().any(|f| f.severity == Severity::Error)
    }

    /// Returns true if there are any warnings.
    #[allow(dead_code)]
    pub fn has_warnings(&self) -> bool {
        self.findings
            .iter()
            .any(|f| f.severity == Severity::Warning)
    }

    /// Count of errors.
    pub fn error_count(&self) -> usize {
        self.findings
            .iter()
            .filter(|f| f.severity == Severity::Error)
            .count()
    }

    /// Count of warnings.
    #[allow(dead_code)]
    pub fn warning_count(&self) -> usize {
        self.findings
            .iter()
            .filter(|f| f.severity == Severity::Warning)
            .count()
    }

    /// Print human-readable summary.
    pub fn print_summary(&self) {
        // Group findings by severity
        let mut errors: Vec<_> = self
            .findings
            .iter()
            .filter(|f| f.severity == Severity::Error)
            .collect();
        let mut warnings: Vec<_> = self
            .findings
            .iter()
            .filter(|f| f.severity == Severity::Warning)
            .collect();
        let mut infos: Vec<_> = self
            .findings
            .iter()
            .filter(|f| f.severity == Severity::Info)
            .collect();

        // Sort by category within each group
        errors.sort_by(|a, b| a.category.cmp(&b.category));
        warnings.sort_by(|a, b| a.category.cmp(&b.category));
        infos.sort_by(|a, b| a.category.cmp(&b.category));

        // Print errors
        if !errors.is_empty() {
            println!("\n❌ Errors ({}):", errors.len());
            println!("{}", "─".repeat(60));
            for finding in &errors {
                print_finding(finding);
            }
        }

        // Print warnings
        if !warnings.is_empty() {
            println!("\n⚠️  Warnings ({}):", warnings.len());
            println!("{}", "─".repeat(60));
            for finding in &warnings {
                print_finding(finding);
            }
        }

        // Print info (only if no errors/warnings or verbose)
        if !infos.is_empty() && errors.is_empty() && warnings.is_empty() {
            println!("\nℹ️  Info ({}):", infos.len());
            println!("{}", "─".repeat(60));
            for finding in &infos {
                print_finding(finding);
            }
        }

        // Print summary
        println!();
        println!("{}", "═".repeat(60));
        if errors.is_empty() && warnings.is_empty() {
            println!("✅ All checks passed!");
        } else {
            println!(
                "Summary: {} error(s), {} warning(s)",
                errors.len(),
                warnings.len()
            );
            if !errors.is_empty() {
                println!("\n❌ Configuration has errors that must be fixed.");
            }
        }
    }
}

fn print_finding(finding: &CheckFinding) {
    let icon = match finding.severity {
        Severity::Error => "✗",
        Severity::Warning => "⚠",
        Severity::Info => "ℹ",
    };

    let location = match (&finding.file, &finding.location) {
        (Some(f), Some(l)) => format!(" [{}:{}]", f.display(), l),
        (Some(f), None) => format!(" [{}]", f.display()),
        (None, Some(l)) => format!(" [{}]", l),
        (None, None) => String::new(),
    };

    println!(
        "  {} [{}]{}: {}",
        icon, finding.category, location, finding.message
    );
}

// ============================================================================
// Main Check Runner
// ============================================================================

/// Run all configuration checks quietly (no output), returns the results.
/// Useful for pre-hook validation.
pub async fn run_quiet(config_path: &Path) -> Result<CheckResults> {
    let base_dir = config_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));

    let mut results = CheckResults::new();

    // 1. JSON Schema validation
    results.extend(validate_json_schemas(&base_dir, config_path)?);

    // 2. Load configuration for cross-file checks
    let config =
        CoriConfig::load_with_context(config_path).context("Failed to load configuration")?;

    // 3. Table name consistency (roles/rules vs schema)
    results.extend(check_table_names(&config, &base_dir)?);

    // 4. Column name validation
    results.extend(check_column_names(&config, &base_dir)?);

    // 5. Approval group validation
    results.extend(check_approval_groups(&config, &base_dir)?);

    // 6. Soft delete consistency
    results.extend(check_soft_delete(&config, &base_dir)?);

    // 7. Rules vs types validation
    results.extend(check_type_references(&config, &base_dir)?);

    // 8. Non-null column constraints
    results.extend(check_nonnull_columns(&config, &base_dir)?);

    // 9. Primary key access for update/delete operations
    results.extend(check_primary_key_access(&config, &base_dir)?);

    Ok(results)
}

/// Run configuration check as a pre-hook before other commands.
/// Returns Ok(()) if no errors found, otherwise prints errors and returns Err.
pub async fn run_pre_hook(config_path: &Path) -> Result<()> {
    // Only run if config file exists
    if !config_path.exists() {
        return Ok(());
    }

    let results = run_quiet(config_path).await?;

    if results.has_errors() {
        eprintln!("\n❌ Configuration check failed. Run `cori check` for details.\n");

        // Print errors only (no warnings in pre-hook)
        let errors: Vec<_> = results
            .findings
            .iter()
            .filter(|f| f.severity == Severity::Error)
            .collect();

        for finding in &errors {
            let location = match (&finding.file, &finding.location) {
                (Some(f), Some(l)) => format!(" [{}:{}]", f.display(), l),
                (Some(f), None) => format!(" [{}]", f.display()),
                (None, Some(l)) => format!(" [{}]", l),
                (None, None) => String::new(),
            };
            eprintln!(
                "  ✗ [{}]{}: {}",
                finding.category, location, finding.message
            );
        }

        eprintln!();
        anyhow::bail!(
            "Configuration has {} error(s). Fix them before running this command.",
            results.error_count()
        );
    }

    Ok(())
}

/// Run all configuration checks.
pub async fn run(config_path: &Path) -> Result<()> {
    println!("🔍 Checking Cori configuration...");
    println!();

    let base_dir = config_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));

    let mut results = CheckResults::new();

    // 1. JSON Schema validation (using embedded schemas)
    println!("  📋 Validating JSON schemas...");
    results.extend(validate_json_schemas(&base_dir, config_path)?);

    // 2. Load configuration for cross-file checks
    let config =
        CoriConfig::load_with_context(config_path).context("Failed to load configuration")?;

    // 3. Table name consistency (roles/rules vs schema)
    println!("  📊 Checking table name consistency...");
    results.extend(check_table_names(&config, &base_dir)?);

    // 4. Column name validation
    println!("  📝 Checking column name consistency...");
    results.extend(check_column_names(&config, &base_dir)?);

    // 5. Approval group validation
    println!("  👥 Checking approval group references...");
    results.extend(check_approval_groups(&config, &base_dir)?);

    // 6. Soft delete consistency
    println!("  🗑️  Checking soft delete configuration...");
    results.extend(check_soft_delete(&config, &base_dir)?);

    // 7. Rules vs types validation
    println!("  🔤 Checking type references in rules...");
    results.extend(check_type_references(&config, &base_dir)?);

    // 8. Non-null column constraints
    println!("  ⚡ Checking non-null column constraints...");
    results.extend(check_nonnull_columns(&config, &base_dir)?);

    // 9. Primary key access for update/delete operations
    println!("  🔑 Checking primary key access for update/delete...");
    results.extend(check_primary_key_access(&config, &base_dir)?);

    // Print results
    results.print_summary();

    // Exit with error if there are errors
    if results.has_errors() {
        anyhow::bail!(
            "Configuration check failed with {} error(s)",
            results.error_count()
        );
    }

    Ok(())
}

// ============================================================================
// Check 1: JSON Schema Validation
// ============================================================================

fn validate_json_schemas(base_dir: &Path, config_path: &Path) -> Result<Vec<CheckFinding>> {
    let mut findings = Vec::new();

    // Load embedded JSON schemas
    let schemas = load_embedded_schemas()?;

    // Validate cori.yaml
    if let Some(schema) = schemas.get("CoriDefinition") {
        findings.extend(validate_yaml_against_schema(
            config_path,
            schema,
            "cori.yaml",
        )?);
    }

    // Validate schema/schema.yaml
    let schema_yaml = base_dir.join("schema").join("schema.yaml");
    if schema_yaml.exists()
        && let Some(schema) = schemas.get("SchemaDefinition") {
            findings.extend(validate_yaml_against_schema(
                &schema_yaml,
                schema,
                "schema/schema.yaml",
            )?);
        }

    // Validate schema/rules.yaml
    let rules_yaml = base_dir.join("schema").join("rules.yaml");
    if rules_yaml.exists()
        && let Some(schema) = schemas.get("RulesDefinition") {
            findings.extend(validate_yaml_against_schema(
                &rules_yaml,
                schema,
                "schema/rules.yaml",
            )?);
        }

    // Validate schema/types.yaml
    let types_yaml = base_dir.join("schema").join("types.yaml");
    if types_yaml.exists()
        && let Some(schema) = schemas.get("TypesDefinition") {
            findings.extend(validate_yaml_against_schema(
                &types_yaml,
                schema,
                "schema/types.yaml",
            )?);
        }

    // Validate roles/*.yaml
    if let Some(schema) = schemas.get("RoleDefinition") {
        let roles_dir = base_dir.join("roles");
        if roles_dir.exists() && roles_dir.is_dir() {
            for entry in fs::read_dir(&roles_dir)? {
                let entry = entry?;
                let path = entry.path();
                if path
                    .extension()
                    .map(|e| e == "yaml" || e == "yml")
                    .unwrap_or(false)
                {
                    let rel_path = format!("roles/{}", path.file_name().unwrap().to_string_lossy());
                    findings.extend(validate_yaml_against_schema(&path, schema, &rel_path)?);
                }
            }
        }
    }

    // Validate groups/*.yaml
    if let Some(schema) = schemas.get("GroupDefinition") {
        let groups_dir = base_dir.join("groups");
        if groups_dir.exists() && groups_dir.is_dir() {
            for entry in fs::read_dir(&groups_dir)? {
                let entry = entry?;
                let path = entry.path();
                if path
                    .extension()
                    .map(|e| e == "yaml" || e == "yml")
                    .unwrap_or(false)
                {
                    let rel_path =
                        format!("groups/{}", path.file_name().unwrap().to_string_lossy());
                    findings.extend(validate_yaml_against_schema(&path, schema, &rel_path)?);
                }
            }
        }
    }

    Ok(findings)
}

/// Load JSON schemas from embedded strings.
fn load_embedded_schemas() -> Result<HashMap<String, JsonValue>> {
    let mut schemas = HashMap::new();

    let embedded = [
        ("CoriDefinition", embedded_schemas::CORI_DEFINITION),
        ("SchemaDefinition", embedded_schemas::SCHEMA_DEFINITION),
        ("RulesDefinition", embedded_schemas::RULES_DEFINITION),
        ("TypesDefinition", embedded_schemas::TYPES_DEFINITION),
        ("RoleDefinition", embedded_schemas::ROLE_DEFINITION),
        ("GroupDefinition", embedded_schemas::GROUP_DEFINITION),
    ];

    for (name, content) in embedded {
        let schema: JsonValue = serde_json::from_str(content)
            .with_context(|| format!("Failed to parse embedded schema: {}", name))?;
        schemas.insert(name.to_string(), schema);
    }

    Ok(schemas)
}

fn validate_yaml_against_schema(
    yaml_path: &Path,
    schema: &JsonValue,
    display_name: &str,
) -> Result<Vec<CheckFinding>> {
    let mut findings = Vec::new();

    // Read and parse YAML
    let content = match fs::read_to_string(yaml_path) {
        Ok(c) => c,
        Err(e) => {
            findings.push(
                CheckFinding::error("json-schema", format!("Failed to read file: {}", e))
                    .with_file(yaml_path.to_path_buf()),
            );
            return Ok(findings);
        }
    };

    let yaml_value: JsonValue = match serde_yaml::from_str(&content) {
        Ok(v) => v,
        Err(e) => {
            findings.push(
                CheckFinding::error("json-schema", format!("Failed to parse YAML: {}", e))
                    .with_file(yaml_path.to_path_buf()),
            );
            return Ok(findings);
        }
    };

    // Validate against JSON schema
    let compiled = match jsonschema::validator_for(schema) {
        Ok(c) => c,
        Err(e) => {
            findings.push(CheckFinding::error(
                "json-schema",
                format!("Failed to compile JSON schema: {}", e),
            ));
            return Ok(findings);
        }
    };

    // Validate against JSON schema using iter_errors to collect all issues
    for error in compiled.iter_errors(&yaml_value) {
        let path_str = error.instance_path().to_string();
        let location = if path_str.is_empty() {
            "(root)".to_string()
        } else {
            path_str
        };

        findings.push(
            CheckFinding::error("json-schema", format!("{}", error))
                .with_file(display_name)
                .with_location(location),
        );
    }

    Ok(findings)
}

// ============================================================================
// Check 2: Table Name Consistency
// ============================================================================

fn check_table_names(config: &CoriConfig, _base_dir: &Path) -> Result<Vec<CheckFinding>> {
    let mut findings = Vec::new();

    // Get all table names from schema
    let schema_tables: HashSet<String> = config
        .schema
        .as_ref()
        .map(|s| s.tables.iter().map(|t| t.name.clone()).collect())
        .unwrap_or_default();

    if schema_tables.is_empty() {
        findings.push(CheckFinding::warning(
            "table-names",
            "No schema definition found (schema/schema.yaml). Cannot validate table names.",
        ));
        return Ok(findings);
    }

    // Check tables in roles
    for (role_name, role) in &config.roles {
        for table_name in role.tables.keys() {
            if !schema_tables.contains(table_name) {
                findings.push(
                    CheckFinding::error(
                        "table-names",
                        format!(
                            "Table '{}' in role '{}' does not exist in schema",
                            table_name, role_name
                        ),
                    )
                    .with_file(format!("roles/{}.yaml", role_name))
                    .with_location(format!("tables.{}", table_name)),
                );
            }
        }
    }

    // Check tables in rules.yaml
    if let Some(rules) = &config.rules {
        for table_name in rules.tables.keys() {
            if !schema_tables.contains(table_name) {
                findings.push(
                    CheckFinding::error(
                        "table-names",
                        format!(
                            "Table '{}' in rules.yaml does not exist in schema",
                            table_name
                        ),
                    )
                    .with_file("schema/rules.yaml")
                    .with_location(format!("tables.{}", table_name)),
                );
            }
        }
    }

    Ok(findings)
}

// ============================================================================
// Check 3: Blocked Tables Conflicts
// ============================================================================
// Check 3: Column Name Validation
// ============================================================================

fn check_column_names(config: &CoriConfig, _base_dir: &Path) -> Result<Vec<CheckFinding>> {
    let mut findings = Vec::new();

    let schema = match &config.schema {
        Some(s) => s,
        None => return Ok(findings), // Already warned in table check
    };

    // Build table -> column names map
    let table_columns: HashMap<&str, HashSet<&str>> = schema
        .tables
        .iter()
        .map(|t| {
            (
                t.name.as_str(),
                t.columns.iter().map(|c| c.name.as_str()).collect(),
            )
        })
        .collect();

    // Check columns in roles
    for (role_name, role) in &config.roles {
        for (table_name, perms) in &role.tables {
            if let Some(schema_columns) = table_columns.get(table_name.as_str()) {
                // Check readable columns
                check_readable_config(
                    &perms.readable,
                    schema_columns,
                    table_name,
                    role_name,
                    "readable",
                    &mut findings,
                );

                // Check creatable columns
                check_creatable_columns(
                    &perms.creatable,
                    schema_columns,
                    table_name,
                    role_name,
                    &mut findings,
                );

                // Check updatable columns
                check_updatable_columns(
                    &perms.updatable,
                    schema_columns,
                    table_name,
                    role_name,
                    &mut findings,
                );
            }
        }
    }

    // Check columns in rules.yaml
    if let Some(rules) = &config.rules {
        for (table_name, table_rules) in &rules.tables {
            if let Some(schema_columns) = table_columns.get(table_name.as_str()) {
                for col_name in table_rules.columns.keys() {
                    if !schema_columns.contains(col_name.as_str()) {
                        findings.push(
                            CheckFinding::error(
                                "column-names",
                                format!(
                                    "Column '{}' in rules for table '{}' does not exist in schema",
                                    col_name, table_name
                                ),
                            )
                            .with_file("schema/rules.yaml")
                            .with_location(format!("tables.{}.columns.{}", table_name, col_name)),
                        );
                    }
                }

                // Check tenant column exists
                if let Some(tenant_config) = &table_rules.tenant {
                    match tenant_config {
                        cori_core::config::TenantConfig::Direct(col) => {
                            if !schema_columns.contains(col.as_str()) {
                                findings.push(
                                    CheckFinding::error(
                                        "column-names",
                                        format!(
                                            "Tenant column '{}' for table '{}' does not exist in schema",
                                            col, table_name
                                        ),
                                    )
                                    .with_file("schema/rules.yaml")
                                    .with_location(format!("tables.{}.tenant", table_name)),
                                );
                            }
                        }
                        cori_core::config::TenantConfig::Inherited(inherited) => {
                            if !schema_columns.contains(inherited.via.as_str()) {
                                findings.push(
                                    CheckFinding::error(
                                        "column-names",
                                        format!(
                                            "FK column '{}' for inherited tenant in table '{}' does not exist",
                                            inherited.via, table_name
                                        ),
                                    )
                                    .with_file("schema/rules.yaml")
                                    .with_location(format!("tables.{}.tenant.via", table_name)),
                                );
                            }
                        }
                    }
                }

                // Check soft_delete column exists
                if let Some(soft_delete) = &table_rules.soft_delete
                    && !schema_columns.contains(soft_delete.column.as_str()) {
                        findings.push(
                            CheckFinding::error(
                                "column-names",
                                format!(
                                    "Soft delete column '{}' for table '{}' does not exist in schema",
                                    soft_delete.column, table_name
                                ),
                            )
                            .with_file("schema/rules.yaml")
                            .with_location(format!("tables.{}.soft_delete.column", table_name)),
                        );
                    }
            }
        }
    }

    Ok(findings)
}

fn check_readable_config(
    readable_config: &cori_core::config::ReadableConfig,
    schema_columns: &HashSet<&str>,
    table_name: &str,
    role_name: &str,
    field_name: &str,
    findings: &mut Vec<CheckFinding>,
) {
    // Extract columns list from ReadableConfig
    let cols = match readable_config {
        cori_core::config::ReadableConfig::All(_) => return, // All columns, no check needed
        cori_core::config::ReadableConfig::List(cols) => cols,
        cori_core::config::ReadableConfig::Config(cfg) => match &cfg.columns {
            cori_core::config::ColumnList::All(_) => return,
            cori_core::config::ColumnList::List(cols) => cols,
        },
    };

    for col in cols {
        if !schema_columns.contains(col.as_str()) {
            findings.push(
                CheckFinding::error(
                    "column-names",
                    format!(
                        "Column '{}' in {}.{} for role '{}' does not exist in schema",
                        col, table_name, field_name, role_name
                    ),
                )
                .with_file(format!("roles/{}.yaml", role_name))
                .with_location(format!("tables.{}.{}", table_name, field_name)),
            );
        }
    }
}

fn check_creatable_columns(
    creatable: &CreatableColumns,
    schema_columns: &HashSet<&str>,
    table_name: &str,
    role_name: &str,
    findings: &mut Vec<CheckFinding>,
) {
    if let Some(map) = creatable.as_map() {
        for col in map.keys() {
            if !schema_columns.contains(col.as_str()) {
                findings.push(
                    CheckFinding::error(
                        "column-names",
                        format!(
                            "Column '{}' in {}.creatable for role '{}' does not exist in schema",
                            col, table_name, role_name
                        ),
                    )
                    .with_file(format!("roles/{}.yaml", role_name))
                    .with_location(format!("tables.{}.creatable.{}", table_name, col)),
                );
            }
        }
    }
}

fn check_updatable_columns(
    updatable: &UpdatableColumns,
    schema_columns: &HashSet<&str>,
    table_name: &str,
    role_name: &str,
    findings: &mut Vec<CheckFinding>,
) {
    if let Some(map) = updatable.as_map() {
        for col in map.keys() {
            if !schema_columns.contains(col.as_str()) {
                findings.push(
                    CheckFinding::error(
                        "column-names",
                        format!(
                            "Column '{}' in {}.updatable for role '{}' does not exist in schema",
                            col, table_name, role_name
                        ),
                    )
                    .with_file(format!("roles/{}.yaml", role_name))
                    .with_location(format!("tables.{}.updatable.{}", table_name, col)),
                );
            }
        }
    }
}

// ============================================================================
// Check 5: Approval Group Validation
// ============================================================================

fn check_approval_groups(config: &CoriConfig, _base_dir: &Path) -> Result<Vec<CheckFinding>> {
    let mut findings = Vec::new();

    // Get all defined groups
    let defined_groups: HashSet<&str> = config.groups.keys().map(|s| s.as_str()).collect();

    // Check groups referenced in roles
    for (role_name, role) in &config.roles {
        // Check role-level approval group
        if let Some(approvals) = &role.approvals {
            check_approval_config(
                approvals,
                &defined_groups,
                &config.groups,
                role_name,
                "approvals",
                &mut findings,
            );
        }

        // Check approval requirements in table permissions
        for (table_name, perms) in &role.tables {
            // Check creatable columns
            if let Some(map) = perms.creatable.as_map() {
                for (col_name, constraints) in map {
                    if let Some(req) = &constraints.requires_approval {
                        check_approval_requirement(
                            req,
                            &defined_groups,
                            &config.groups,
                            &role.approvals,
                            role_name,
                            &format!(
                                "tables.{}.creatable.{}.requires_approval",
                                table_name, col_name
                            ),
                            &mut findings,
                        );
                    }
                }
            }

            // Check updatable columns
            if let Some(map) = perms.updatable.as_map() {
                for (col_name, constraints) in map {
                    if let Some(req) = &constraints.requires_approval {
                        check_approval_requirement(
                            req,
                            &defined_groups,
                            &config.groups,
                            &role.approvals,
                            role_name,
                            &format!(
                                "tables.{}.updatable.{}.requires_approval",
                                table_name, col_name
                            ),
                            &mut findings,
                        );
                    }
                }
            }

            // Check deletable
            if let DeletablePermission::WithConstraints(opts) = &perms.deletable
                && let Some(req) = &opts.requires_approval {
                    check_approval_requirement(
                        req,
                        &defined_groups,
                        &config.groups,
                        &role.approvals,
                        role_name,
                        &format!("tables.{}.deletable.requires_approval", table_name),
                        &mut findings,
                    );
                }
        }
    }

    Ok(findings)
}

fn check_approval_config(
    config: &ApprovalConfig,
    defined_groups: &HashSet<&str>,
    groups: &HashMap<String, GroupDefinition>,
    role_name: &str,
    location: &str,
    findings: &mut Vec<CheckFinding>,
) {
    if !defined_groups.contains(config.group.as_str()) {
        findings.push(
            CheckFinding::error(
                "approval-groups",
                format!(
                    "Approval group '{}' referenced in role '{}' is not defined in groups/",
                    config.group, role_name
                ),
            )
            .with_file(format!("roles/{}.yaml", role_name))
            .with_location(location.to_string()),
        );
    } else if let Some(group) = groups.get(&config.group) {
        // Check that group has valid email members
        if group.members.is_empty() {
            findings.push(
                CheckFinding::error(
                    "approval-groups",
                    format!(
                        "Approval group '{}' referenced in role '{}' has no members",
                        config.group, role_name
                    ),
                )
                .with_file(format!("groups/{}.yaml", config.group)),
            );
        } else {
            // Validate email format for members
            for member in &group.members {
                if !is_valid_email(member) {
                    findings.push(
                        CheckFinding::warning(
                            "approval-groups",
                            format!(
                                "Member '{}' in group '{}' does not appear to be a valid email",
                                member, config.group
                            ),
                        )
                        .with_file(format!("groups/{}.yaml", config.group))
                        .with_location("members"),
                    );
                }
            }
        }
    }
}

fn check_approval_requirement(
    req: &ApprovalRequirement,
    defined_groups: &HashSet<&str>,
    groups: &HashMap<String, GroupDefinition>,
    role_approvals: &Option<ApprovalConfig>,
    role_name: &str,
    location: &str,
    findings: &mut Vec<CheckFinding>,
) {
    match req {
        ApprovalRequirement::Simple(true) => {
            // requires_approval: true — needs a default group at role level
            if role_approvals.is_none() {
                findings.push(
                    CheckFinding::error(
                        "approval-groups",
                        format!(
                            "requires_approval: true at {} in role '{}' but no default approval group defined",
                            location, role_name
                        ),
                    )
                    .with_file(format!("roles/{}.yaml", role_name))
                    .with_location(location.to_string()),
                );
            }
        }
        ApprovalRequirement::Simple(false) => {
            // No approval needed
        }
        ApprovalRequirement::Detailed(config) => {
            check_approval_config(
                config,
                defined_groups,
                groups,
                role_name,
                location,
                findings,
            );
        }
    }
}

fn is_valid_email(s: &str) -> bool {
    // Simple email validation
    let at_pos = s.find('@');
    let dot_pos = s.rfind('.');

    match (at_pos, dot_pos) {
        (Some(at), Some(dot)) => at > 0 && dot > at + 1 && dot < s.len() - 1,
        _ => false,
    }
}

// ============================================================================
// Check 6: Soft Delete Consistency
// ============================================================================

fn check_soft_delete(config: &CoriConfig, _base_dir: &Path) -> Result<Vec<CheckFinding>> {
    let mut findings = Vec::new();

    let rules = match &config.rules {
        Some(r) => r,
        None => return Ok(findings),
    };

    // Build map of tables with soft_delete configured in rules
    let tables_with_soft_delete: HashMap<&str, &str> = rules
        .tables
        .iter()
        .filter_map(|(table, rule)| {
            rule.soft_delete
                .as_ref()
                .map(|sd| (table.as_str(), sd.column.as_str()))
        })
        .collect();

    // Check roles
    for (role_name, role) in &config.roles {
        for (table_name, perms) in &role.tables {
            if let DeletablePermission::WithConstraints(opts) = &perms.deletable
                && opts.soft_delete {
                    // Role specifies soft_delete: true
                    if !tables_with_soft_delete.contains_key(table_name.as_str()) {
                        findings.push(
                            CheckFinding::error(
                                "soft-delete",
                                format!(
                                    "Role '{}' specifies soft_delete for table '{}' but no soft_delete column is defined in rules.yaml",
                                    role_name, table_name
                                ),
                            )
                            .with_file(format!("roles/{}.yaml", role_name))
                            .with_location(format!("tables.{}.deletable.soft_delete", table_name)),
                        );
                    }
                }

            // Check if deletion is allowed but soft_delete is configured in rules
            let deletion_allowed = match &perms.deletable {
                DeletablePermission::Allowed(true) => true,
                DeletablePermission::WithConstraints(opts) => !opts.soft_delete,
                _ => false,
            };

            if deletion_allowed && tables_with_soft_delete.contains_key(table_name.as_str()) {
                let sd_col = tables_with_soft_delete.get(table_name.as_str()).unwrap();
                findings.push(
                    CheckFinding::warning(
                        "soft-delete",
                        format!(
                            "Table '{}' has soft_delete configured in rules.yaml (column: '{}') but role '{}' allows hard delete. Consider using soft_delete: true in the role.",
                            table_name, sd_col, role_name
                        ),
                    )
                    .with_file(format!("roles/{}.yaml", role_name))
                    .with_location(format!("tables.{}.deletable", table_name)),
                );
            }
        }
    }

    Ok(findings)
}

// ============================================================================
// Check 7: Type References in Rules
// ============================================================================

fn check_type_references(config: &CoriConfig, _base_dir: &Path) -> Result<Vec<CheckFinding>> {
    let mut findings = Vec::new();

    let rules = match &config.rules {
        Some(r) => r,
        None => return Ok(findings),
    };

    // Get defined types
    let defined_types: HashSet<&str> = config
        .types
        .as_ref()
        .map(|t| t.types.keys().map(|k| k.as_str()).collect())
        .unwrap_or_default();

    // Check type references in rules
    for (table_name, table_rules) in &rules.tables {
        for (col_name, col_rules) in &table_rules.columns {
            if let Some(type_ref) = &col_rules.type_ref
                && !defined_types.contains(type_ref.as_str()) {
                    findings.push(
                        CheckFinding::error(
                            "type-references",
                            format!(
                                "Column '{}.{}' references type '{}' which is not defined in types.yaml",
                                table_name, col_name, type_ref
                            ),
                        )
                        .with_file("schema/rules.yaml")
                        .with_location(format!("tables.{}.columns.{}.type", table_name, col_name)),
                    );
                }
        }
    }

    Ok(findings)
}

// ============================================================================
// Check 8: Non-Null Column Constraints
// ============================================================================

fn check_nonnull_columns(config: &CoriConfig, _base_dir: &Path) -> Result<Vec<CheckFinding>> {
    let mut findings = Vec::new();

    let schema = match &config.schema {
        Some(s) => s,
        None => return Ok(findings),
    };

    // Build map of table -> non-null columns without defaults
    let mut required_columns: HashMap<&str, HashSet<&str>> = HashMap::new();

    for table in &schema.tables {
        let mut cols = HashSet::new();
        for col in &table.columns {
            // Column is "required" if it's NOT NULL and has no default
            if !col.nullable && col.default.is_none() {
                // Exclude primary key columns (usually auto-generated)
                if !table.primary_key.contains(&col.name) {
                    cols.insert(col.name.as_str());
                }
            }
        }
        if !cols.is_empty() {
            required_columns.insert(table.name.as_str(), cols);
        }
    }

    // Check each role
    for (role_name, role) in &config.roles {
        for (table_name, perms) in &role.tables {
            // Only check if role can create records
            if perms.creatable.is_empty() {
                continue;
            }

            let Some(req_cols) = required_columns.get(table_name.as_str()) else {
                continue;
            };

            // Check if creatable covers all required columns (or provides defaults)
            match &perms.creatable {
                CreatableColumns::All(_) => {
                    // All columns are creatable, but agent still needs to provide values
                    // This is informational only
                }
                CreatableColumns::Map(map) => {
                    for req_col in req_cols {
                        if !map.contains_key(*req_col) {
                            findings.push(
                                CheckFinding::error(
                                    "non-null-columns",
                                    format!(
                                        "Column '{}' in table '{}' is NOT NULL without default, but role '{}' doesn't allow creating it",
                                        req_col, table_name, role_name
                                    ),
                                )
                                .with_file(format!("roles/{}.yaml", role_name))
                                .with_location(format!("tables.{}.creatable", table_name)),
                            );
                        } else {
                            // Column is creatable - check if required or has default
                            let constraints = map.get(*req_col).unwrap();
                            if !constraints.required && constraints.default.is_none() {
                                findings.push(
                                    CheckFinding::warning(
                                        "non-null-columns",
                                        format!(
                                            "Column '{}' in table '{}' is NOT NULL without DB default. Role '{}' should mark it as required or provide a default.",
                                            req_col, table_name, role_name
                                        ),
                                    )
                                    .with_file(format!("roles/{}.yaml", role_name))
                                    .with_location(format!("tables.{}.creatable.{}", table_name, req_col)),
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(findings)
}

// ============================================================================
// Check 9: Primary Key Access for Update/Delete Operations
// ============================================================================

fn check_primary_key_access(config: &CoriConfig, _base_dir: &Path) -> Result<Vec<CheckFinding>> {
    let mut findings = Vec::new();

    let schema = match &config.schema {
        Some(s) => s,
        None => return Ok(findings), // Already warned in table check
    };

    // Build table -> primary key columns map
    let table_primary_keys: HashMap<&str, Vec<&str>> = schema
        .tables
        .iter()
        .map(|t| {
            (
                t.name.as_str(),
                t.primary_key.iter().map(|c| c.as_str()).collect(),
            )
        })
        .collect();

    // Check each role
    for (role_name, role) in &config.roles {
        for (table_name, perms) in &role.tables {
            // Get primary key columns for this table
            let pk_columns = match table_primary_keys.get(table_name.as_str()) {
                Some(pks) if !pks.is_empty() => pks,
                _ => continue, // Table has no PK or table not in schema - skip
            };

            // Get readable columns for this role
            let readable_cols: HashSet<&str> = match &perms.readable {
                ReadableConfig::All(_) => {
                    // All columns are readable, so PKs are included
                    continue;
                }
                ReadableConfig::List(cols) => cols.iter().map(|c| c.as_str()).collect(),
                ReadableConfig::Config(cfg) => match &cfg.columns {
                    cori_core::config::ColumnList::All(_) => continue,
                    cori_core::config::ColumnList::List(cols) => {
                        cols.iter().map(|c| c.as_str()).collect()
                    }
                },
            };

            // Check if role allows update
            let allows_update = !perms.updatable.is_empty();

            // Check if role allows delete
            let allows_delete = perms.deletable.is_allowed();

            // If update or delete is allowed, all PK columns must be readable
            if allows_update || allows_delete {
                let operation = if allows_update && allows_delete {
                    "update and delete"
                } else if allows_update {
                    "update"
                } else {
                    "delete"
                };

                for pk_col in pk_columns {
                    if !readable_cols.contains(pk_col) {
                        findings.push(
                            CheckFinding::error(
                                "primary-key-access",
                                format!(
                                    "Role '{}' allows {} on table '{}' but primary key column '{}' is not readable. \
                                     Add '{}' to readable columns to enable row identification.",
                                    role_name, operation, table_name, pk_col, pk_col
                                ),
                            )
                            .with_file(format!("roles/{}.yaml", role_name))
                            .with_location(format!("tables.{}.readable", table_name)),
                        );
                    }
                }
            }
        }
    }

    Ok(findings)
}
