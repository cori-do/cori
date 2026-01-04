//! Proxy configuration types.

use serde::{Deserialize, Serialize};

/// Configuration for the Postgres wire protocol proxy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyConfig {
    /// Address to listen on.
    #[serde(default = "default_listen_addr")]
    pub listen_addr: String,

    /// Port to listen on for incoming Postgres connections.
    #[serde(default = "default_listen_port")]
    pub listen_port: u16,

    /// Maximum number of concurrent connections.
    #[serde(default = "default_max_connections")]
    pub max_connections: u32,

    /// Connection timeout in seconds.
    #[serde(default = "default_connection_timeout")]
    pub connection_timeout: u32,

    /// TLS configuration.
    #[serde(default)]
    pub tls: TlsConfig,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            listen_addr: default_listen_addr(),
            listen_port: default_listen_port(),
            max_connections: default_max_connections(),
            connection_timeout: default_connection_timeout(),
            tls: TlsConfig::default(),
        }
    }
}

/// TLS configuration for the proxy.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TlsConfig {
    /// Whether TLS is enabled.
    #[serde(default)]
    pub enabled: bool,

    /// Path to certificate file.
    #[serde(default)]
    pub cert_file: Option<String>,

    /// Path to private key file.
    #[serde(default)]
    pub key_file: Option<String>,
}

/// Configuration for the upstream Postgres connection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpstreamConfig {
    /// Hostname of the upstream Postgres server.
    #[serde(default = "default_host")]
    pub host: String,

    /// Port of the upstream Postgres server.
    #[serde(default = "default_upstream_port")]
    pub port: u16,

    /// Database name to connect to.
    #[serde(default = "default_database")]
    pub database: String,

    /// Username for upstream connection.
    #[serde(default = "default_username")]
    pub username: String,

    /// Password for upstream connection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,

    /// Environment variable containing the full DATABASE_URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credentials_env: Option<String>,
}

impl Default for UpstreamConfig {
    fn default() -> Self {
        Self {
            host: default_host(),
            port: default_upstream_port(),
            database: default_database(),
            username: default_username(),
            password: None,
            credentials_env: None,
        }
    }
}

impl UpstreamConfig {
    /// Build a PostgreSQL connection string from this configuration.
    pub fn connection_string(&self) -> String {
        // If credentials_env is set, try to read from environment
        if let Some(env_var) = &self.credentials_env {
            if let Ok(url) = std::env::var(env_var) {
                return url;
            }
        }

        match &self.password {
            Some(password) => format!(
                "postgresql://{}:{}@{}:{}/{}",
                self.username, password, self.host, self.port, self.database
            ),
            None => format!(
                "postgresql://{}@{}:{}/{}",
                self.username, self.host, self.port, self.database
            ),
        }
    }

    /// Check if this configuration uses environment variable for credentials.
    pub fn uses_env_credentials(&self) -> bool {
        self.credentials_env.is_some()
    }
}

// Default value functions
fn default_listen_addr() -> String {
    "0.0.0.0".to_string()
}

fn default_listen_port() -> u16 {
    5433
}

fn default_max_connections() -> u32 {
    100
}

fn default_connection_timeout() -> u32 {
    30
}

fn default_host() -> String {
    "localhost".to_string()
}

fn default_upstream_port() -> u16 {
    5432
}

fn default_database() -> String {
    "postgres".to_string()
}

fn default_username() -> String {
    "postgres".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_connection_string_with_password() {
        let config = UpstreamConfig {
            host: "localhost".to_string(),
            port: 5432,
            database: "mydb".to_string(),
            username: "user".to_string(),
            password: Some("pass".to_string()),
            credentials_env: None,
        };
        assert_eq!(
            config.connection_string(),
            "postgresql://user:pass@localhost:5432/mydb"
        );
    }

    #[test]
    fn test_connection_string_without_password() {
        let config = UpstreamConfig {
            host: "localhost".to_string(),
            port: 5432,
            database: "mydb".to_string(),
            username: "user".to_string(),
            password: None,
            credentials_env: None,
        };
        assert_eq!(
            config.connection_string(),
            "postgresql://user@localhost:5432/mydb"
        );
    }
}
