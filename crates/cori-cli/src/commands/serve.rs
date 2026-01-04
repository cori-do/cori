//! Serve command for starting the Cori proxy.
//!
//! `cori serve` - Start the Postgres proxy server.

use cori_audit::AuditLogger;
use cori_biscuit::TokenVerifier;
use cori_core::config::{AuditConfig, ProxyConfig, TenancyConfig, UpstreamConfig};
use cori_proxy::{CoriProxy, RolePermissions};
use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Configuration file structure for `cori serve`.
#[derive(Debug, Deserialize)]
pub struct ServeConfig {
    /// Upstream Postgres connection.
    pub upstream: UpstreamConfigFile,

    /// Proxy settings.
    #[serde(default)]
    pub proxy: ProxySettings,

    /// Biscuit configuration.
    pub biscuit: BiscuitConfig,

    /// Path to external tenancy configuration file.
    /// If specified, this takes precedence over inline tenancy config.
    #[serde(default)]
    pub tenancy_file: Option<PathBuf>,

    /// Inline tenancy configuration.
    /// Used if tenancy_file is not specified.
    #[serde(default)]
    pub tenancy: TenancyConfigFile,

    /// Audit configuration.
    #[serde(default)]
    pub audit: AuditConfigFile,

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
    /// Hostname (optional if credentials_env is set)
    pub host: Option<String>,
    #[serde(default = "default_upstream_port")]
    pub port: u16,
    /// Database name (optional if credentials_env is set)
    pub database: Option<String>,
    /// Username (optional if credentials_env is set)
    pub username: Option<String>,
    pub password: Option<String>,
    /// Environment variable containing DATABASE_URL
    pub credentials_env: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ProxySettings {
    #[serde(default = "default_listen_addr")]
    pub listen_addr: String,
    #[serde(default = "default_listen_port")]
    pub listen_port: u16,
    #[serde(default = "default_max_connections")]
    pub max_connections: u32,
}

impl Default for ProxySettings {
    fn default() -> Self {
        Self {
            listen_addr: default_listen_addr(),
            listen_port: default_listen_port(),
            max_connections: default_max_connections(),
        }
    }
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

#[derive(Debug, Deserialize)]
pub struct TenancyConfigFile {
    #[serde(default = "default_tenant_column")]
    pub default_column: String,
    #[serde(default)]
    pub global_tables: Vec<String>,
}

impl Default for TenancyConfigFile {
    fn default() -> Self {
        Self {
            default_column: default_tenant_column(),
            global_tables: Vec::new(),
        }
    }
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

fn default_listen_addr() -> String {
    "0.0.0.0".to_string()
}

fn default_listen_port() -> u16 {
    5433
}

fn default_max_connections() -> u32 {
    100
}

fn default_tenant_column() -> String {
    "tenant_id".to_string()
}

fn default_enabled() -> bool {
    true
}

/// Start the Cori proxy server.
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

    // Convert roles to role permissions for virtual schema filtering
    let role_permissions = build_role_permissions(&roles);

    // Resolve Biscuit public key
    let public_key_hex = resolve_public_key(&serve_config.biscuit)?;

    // Create token verifier
    let public_key = cori_biscuit::keys::load_public_key_hex(&public_key_hex)?;
    let verifier = TokenVerifier::new(public_key);

    // Build proxy config
    // When credentials_env is set, we can use placeholder values since connection_string()
    // will read from the environment variable instead
    let (upstream_host, upstream_database, upstream_username) = 
        if serve_config.upstream.credentials_env.is_some() {
            // Placeholders - connection_string() will use credentials_env
            (
                serve_config.upstream.host.clone().unwrap_or_else(|| "localhost".to_string()),
                serve_config.upstream.database.clone().unwrap_or_else(|| "postgres".to_string()),
                serve_config.upstream.username.clone().unwrap_or_else(|| "postgres".to_string()),
            )
        } else {
            // Explicit config required
            let host = serve_config.upstream.host.clone().ok_or_else(|| {
                anyhow::anyhow!("upstream.host is required when credentials_env is not set")
            })?;
            let database = serve_config.upstream.database.clone().ok_or_else(|| {
                anyhow::anyhow!("upstream.database is required when credentials_env is not set")
            })?;
            let username = serve_config.upstream.username.clone().unwrap_or_else(|| "postgres".to_string());
            (host, database, username)
        };

    let upstream_config = UpstreamConfig {
        host: upstream_host.clone(),
        port: serve_config.upstream.port,
        database: upstream_database,
        username: upstream_username,
        password: serve_config.upstream.password.clone(),
        credentials_env: serve_config.upstream.credentials_env.clone(),
    };

    // Build tenancy config - load from file if specified, otherwise use inline config
    let tenancy_config = load_tenancy_config(&serve_config, &config_dir)?;

    let proxy_config = ProxyConfig {
        listen_addr: serve_config.proxy.listen_addr,
        listen_port: serve_config.proxy.listen_port,
        max_connections: serve_config.proxy.max_connections,
        ..Default::default()
    };

    // Build audit logger if enabled
    let audit_logger = if serve_config.audit.enabled {
        let audit_config = AuditConfig {
            enabled: serve_config.audit.enabled,
            log_queries: serve_config.audit.log_queries,
            log_results: serve_config.audit.log_results,
            ..Default::default()
        };
        Some(AuditLogger::new(audit_config)?)
    } else {
        None
    };

    tracing::info!(
        listen_port = proxy_config.listen_port,
        upstream_host = upstream_config.host,
        roles_loaded = roles.len(),
        "Starting Cori proxy"
    );

    // Create and run proxy with role permissions
    let proxy = CoriProxy::with_role_permissions(
        proxy_config,
        upstream_config,
        verifier,
        tenancy_config,
        audit_logger,
        role_permissions,
    );

    proxy.run().await?;

    Ok(())
}

/// Load tenancy configuration from file or inline config.
///
/// If `tenancy_file` is specified in the config, load from that file using cori-core.
/// Otherwise, use the inline `tenancy` configuration.
fn load_tenancy_config(config: &ServeConfig, config_dir: &Path) -> anyhow::Result<TenancyConfig> {
    if let Some(tenancy_file) = &config.tenancy_file {
        tracing::info!(path = %tenancy_file.display(), "Loading tenancy configuration from file");
        match TenancyConfig::load_from_path(tenancy_file, config_dir) {
            Ok(tenancy) => {
                tracing::debug!(
                    default_column = %tenancy.default_column,
                    tables = tenancy.tables.len(),
                    global_tables = tenancy.global_tables.len(),
                    "Tenancy configuration loaded"
                );
                Ok(tenancy)
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "Failed to load tenancy file, using inline configuration"
                );
                Ok(TenancyConfig {
                    default_column: config.tenancy.default_column.clone(),
                    global_tables: config.tenancy.global_tables.clone(),
                    ..Default::default()
                })
            }
        }
    } else {
        // Use inline configuration
        Ok(TenancyConfig {
            default_column: config.tenancy.default_column.clone(),
            global_tables: config.tenancy.global_tables.clone(),
            ..Default::default()
        })
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
///
/// Roles can be loaded from:
/// 1. A directory (roles_dir) - each .yaml file is a role
/// 2. Individual files (role_files) - explicit list of role files
/// 3. Inline definitions (roles) - defined directly in the config
///
/// All sources are merged, with later definitions overriding earlier ones.
pub fn load_roles(config: &ServeConfig, config_dir: &Path) -> anyhow::Result<Vec<LoadedRole>> {
    let mut roles: HashMap<String, RoleConfigFile> = HashMap::new();

    // 1. Load from roles_dir if specified
    if let Some(roles_dir) = &config.roles_dir {
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
            tracing::warn!(
                dir = %resolved_dir.display(),
                "Roles directory does not exist or is not a directory"
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

/// Build role permissions map for virtual schema filtering.
///
/// Extracts table and column access information from role configurations
/// and formats it for use by the virtual schema handler.
fn build_role_permissions(roles: &[LoadedRole]) -> HashMap<String, RolePermissions> {
    let mut permissions_map = HashMap::new();

    for role in roles {
        let accessible_tables: Vec<String> = role.config.tables.keys().cloned().collect();
        
        let mut readable_columns = HashMap::new();
        for (table_name, table_config) in &role.config.tables {
            let columns = match &table_config.readable {
                ReadableColumnsConfig::All => {
                    // For "*", we can't know all columns without schema introspection
                    // So we pass an empty vec and the virtual schema handler will need to handle this
                    // For now, just put a marker that we'll handle specially
                    vec!["*".to_string()]
                }
                ReadableColumnsConfig::List(cols) => cols.clone(),
                ReadableColumnsConfig::None => vec![],
            };
            readable_columns.insert(table_name.clone(), columns);
        }

        permissions_map.insert(
            role.name.clone(),
            RolePermissions {
                accessible_tables,
                readable_columns,
            },
        );
    }

    permissions_map
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

proxy:
  listen_port: 5433
  max_connections: 100

biscuit:
  public_key_env: BISCUIT_PUBLIC_KEY

tenancy:
  default_column: organization_id
  global_tables:
    - products
    - categories

audit:
  enabled: true
  log_queries: true
  log_results: false
"#;

        let serve_config: ServeConfig = serde_yaml::from_str(config).unwrap();
        assert_eq!(serve_config.upstream.host.as_deref(), Some("localhost"));
        assert_eq!(serve_config.proxy.listen_port, 5433);
        assert_eq!(serve_config.tenancy.default_column, "organization_id");
        assert_eq!(serve_config.tenancy.global_tables.len(), 2);
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
        assert_eq!(serve_config.upstream.username, None); // No default username in UpstreamConfigFile
        assert_eq!(serve_config.proxy.listen_port, 5433);
        assert_eq!(serve_config.tenancy.default_column, "tenant_id");
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
    fn test_parse_role_files() {
        let config = r#"
upstream:
  host: localhost
  database: mydb

biscuit:
  public_key_file: /path/to/key

role_files:
  - roles/support_agent.yaml
  - roles/sales_agent.yaml
"#;

        let serve_config: ServeConfig = serde_yaml::from_str(config).unwrap();
        assert_eq!(serve_config.role_files.len(), 2);
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

        // Test readable columns parsing
        let customers = support.tables.get("customers").unwrap();
        match &customers.readable {
            ReadableColumnsConfig::List(cols) => {
                assert_eq!(cols.len(), 3);
                assert!(cols.contains(&"id".to_string()));
            }
            _ => panic!("Expected List for customers.readable"),
        }

        let tickets = support.tables.get("tickets").unwrap();
        match &tickets.readable {
            ReadableColumnsConfig::All => {}
            _ => panic!("Expected All for tickets.readable"),
        }

        // Test editable columns parsing
        match &customers.editable {
            EditableColumnsConfig::None => {}
            _ => panic!("Expected None for customers.editable"),
        }

        match &tickets.editable {
            EditableColumnsConfig::Map(map) => {
                assert!(map.contains_key("status"));
                let status = map.get("status").unwrap();
                assert_eq!(
                    status.allowed_values,
                    Some(vec!["open".to_string(), "closed".to_string()])
                );
            }
            _ => panic!("Expected Map for tickets.editable"),
        }
    }

    #[test]
    fn test_parse_role_file() {
        let role_yaml = r#"
name: analytics_agent
description: "Analytics agent for reporting"

tables:
  orders:
    readable: [order_id, total, created_at]
    editable: []

blocked_tables:
  - users
  - billing

max_rows_per_query: 10000
max_affected_rows: 0

blocked_operations:
  - INSERT
  - UPDATE
  - DELETE
"#;

        let config: RoleConfigFile = serde_yaml::from_str(role_yaml).unwrap();
        assert_eq!(config.name, Some("analytics_agent".to_string()));
        assert_eq!(config.tables.len(), 1);
        assert_eq!(config.blocked_tables.len(), 2);
        assert_eq!(config.max_rows_per_query, Some(10000));
        assert_eq!(config.max_affected_rows, Some(0));
        assert_eq!(config.blocked_operations.len(), 3);
    }
}
