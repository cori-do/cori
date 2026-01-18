//! Upstream database configuration types.
//!
//! This module defines configuration for the upstream PostgreSQL connection.
//! Three configuration methods are supported:
//! 1. `database_url_env` - reference an environment variable
//! 2. `database_url` - provide the URL directly
//! 3. Individual fields (host, port, database, username, password)

use serde::{Deserialize, Serialize};

/// Configuration for the upstream Postgres connection.
///
/// Supports three configuration methods (in order of precedence):
/// 1. Environment variable containing the full connection URL
/// 2. Direct connection URL
/// 3. Individual connection parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpstreamConfig {
    /// Environment variable name containing the PostgreSQL connection URL.
    /// Highest precedence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub database_url_env: Option<String>,

    /// Full PostgreSQL connection URL.
    /// Second precedence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub database_url: Option<String>,

    /// Hostname of the upstream Postgres server.
    #[serde(default = "default_host")]
    pub host: String,

    /// Port of the upstream Postgres server.
    #[serde(default = "default_port")]
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

    /// Environment variable containing the password.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password_env: Option<String>,

    /// SSL mode for the connection.
    #[serde(default)]
    pub ssl_mode: SslMode,

    /// Connection pool configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pool: Option<ConnectionPoolConfig>,
}

impl Default for UpstreamConfig {
    fn default() -> Self {
        Self {
            database_url_env: None,
            database_url: None,
            host: default_host(),
            port: default_port(),
            database: default_database(),
            username: default_username(),
            password: None,
            password_env: None,
            ssl_mode: SslMode::default(),
            pool: None,
        }
    }
}

/// SSL mode for database connections.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SslMode {
    /// Disable SSL.
    Disable,
    /// Allow SSL but don't require it.
    Allow,
    /// Prefer SSL (default).
    #[default]
    Prefer,
    /// Require SSL.
    Require,
    /// Require SSL with CA verification.
    #[serde(rename = "verify-ca")]
    VerifyCa,
    /// Require SSL with full verification.
    #[serde(rename = "verify-full")]
    VerifyFull,
}

/// Connection pool configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionPoolConfig {
    /// Minimum number of connections to maintain.
    #[serde(default = "default_min_connections")]
    pub min_connections: u32,

    /// Maximum number of connections in the pool.
    #[serde(default = "default_max_connections")]
    pub max_connections: u32,

    /// Timeout in seconds when acquiring a connection.
    #[serde(default = "default_acquire_timeout")]
    pub acquire_timeout_seconds: u32,

    /// How long a connection can remain idle before being closed.
    #[serde(default = "default_idle_timeout")]
    pub idle_timeout_seconds: u32,
}

impl Default for ConnectionPoolConfig {
    fn default() -> Self {
        Self {
            min_connections: default_min_connections(),
            max_connections: default_max_connections(),
            acquire_timeout_seconds: default_acquire_timeout(),
            idle_timeout_seconds: default_idle_timeout(),
        }
    }
}

fn default_min_connections() -> u32 {
    1
}

fn default_max_connections() -> u32 {
    10
}

fn default_acquire_timeout() -> u32 {
    30
}

fn default_idle_timeout() -> u32 {
    600
}

impl UpstreamConfig {
    /// Build a PostgreSQL connection string from this configuration.
    ///
    /// Precedence:
    /// 1. database_url_env (environment variable)
    /// 2. credentials_env (legacy, for backwards compatibility)
    /// 3. database_url (direct URL)
    /// 4. Individual fields
    pub fn connection_string(&self) -> String {
        // Method 1: Environment variable with connection URL
        if let Some(env_var) = &self.database_url_env
            && let Ok(url) = std::env::var(env_var) {
                return url;
            }

        // Method 2: Direct URL
        if let Some(url) = &self.database_url {
            return url.clone();
        }

        // Method 3: Individual fields
        let password = self.get_password();
        match password {
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

    /// Get the password, checking password_env first.
    fn get_password(&self) -> Option<String> {
        // Try password_env first
        if let Some(env_var) = &self.password_env
            && let Ok(password) = std::env::var(env_var) {
                return Some(password);
            }
        // Fall back to direct password
        self.password.clone()
    }

    /// Check if this configuration uses environment variables.
    pub fn uses_env_credentials(&self) -> bool {
        self.database_url_env.is_some() || self.password_env.is_some()
    }

    /// Get the configured SSL mode.
    pub fn get_ssl_mode(&self) -> &SslMode {
        &self.ssl_mode
    }
}

// Default value functions
fn default_host() -> String {
    "localhost".to_string()
}

fn default_port() -> u16 {
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
            ..Default::default()
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
            ..Default::default()
        };
        assert_eq!(
            config.connection_string(),
            "postgresql://user@localhost:5432/mydb"
        );
    }

    #[test]
    fn test_connection_string_direct_url() {
        let config = UpstreamConfig {
            database_url: Some(
                "postgresql://admin:secret@db.example.com:5432/production".to_string(),
            ),
            ..Default::default()
        };
        assert_eq!(
            config.connection_string(),
            "postgresql://admin:secret@db.example.com:5432/production"
        );
    }

    #[test]
    fn test_ssl_mode_serialization() {
        let config = UpstreamConfig {
            ssl_mode: SslMode::VerifyFull,
            ..Default::default()
        };
        let yaml = serde_yaml::to_string(&config).unwrap();
        assert!(yaml.contains("verify-full"));
    }
}
