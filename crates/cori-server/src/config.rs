use serde::{Deserialize, Serialize};
use std::{env, fs, path::PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default)]
    pub auth: AuthConfig,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            server: ServerConfig::default(),
            auth: AuthConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    /// Bind address, e.g. "0.0.0.0:8080"
    #[serde(default = "default_bind")]
    pub bind: String,

    /// Path to the local SQLite file used by the embedded auth components.
    #[serde(default = "default_auth_db_path")]
    pub auth_sqlite_path: String,
}

fn default_bind() -> String {
    "0.0.0.0:8080".to_string()
}

fn default_auth_db_path() -> String {
    "data/cori-auth.sqlite".to_string()
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind: default_bind(),
            auth_sqlite_path: default_auth_db_path(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AuthMode {
    Embedded,
    External,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthConfig {
    #[serde(default = "default_auth_mode")]
    pub mode: AuthMode,

    #[serde(default)]
    pub embedded: EmbeddedAuthConfig,

    #[serde(default)]
    pub external: ExternalAuthConfig,
}

fn default_auth_mode() -> AuthMode {
    AuthMode::Embedded
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            mode: default_auth_mode(),
            embedded: EmbeddedAuthConfig::default(),
            external: ExternalAuthConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddedAuthConfig {
    /// Default admin password. For security: prefer setting env var
    /// `CORI_AUTH_EMBEDDED_ADMIN_PASSWORD`.
    #[serde(default = "default_admin_password")]
    pub admin_password: String,
}

fn default_admin_password() -> String {
    "changeme".to_string()
}

impl Default for EmbeddedAuthConfig {
    fn default() -> Self {
        Self {
            admin_password: default_admin_password(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalAuthConfig {
    #[serde(default)]
    pub issuer_url: String,
    #[serde(default)]
    pub client_id: String,
}

impl Default for ExternalAuthConfig {
    fn default() -> Self {
        Self {
            issuer_url: "https://accounts.google.com".to_string(),
            client_id: String::new(),
        }
    }
}

pub fn load_config() -> anyhow::Result<AppConfig> {
    let path = config_path();
    let raw = fs::read_to_string(&path)?;
    let cfg: AppConfig = toml::from_str(&raw)?;
    Ok(cfg)
}

fn config_path() -> PathBuf {
    if let Ok(p) = env::var("CORI_SERVER_CONFIG") {
        return PathBuf::from(p);
    }
    PathBuf::from("config.toml")
}


