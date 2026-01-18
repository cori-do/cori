//! Dashboard configuration.
//!
//! This module defines configuration for the admin dashboard web UI.

use serde::{Deserialize, Serialize};

/// Configuration for the admin dashboard.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DashboardConfig {
    /// Whether the dashboard is enabled.
    #[serde(default = "default_enabled")]
    pub enabled: bool,

    /// Host to bind the dashboard to.
    #[serde(default = "default_host")]
    pub host: String,

    /// Port to listen on.
    #[serde(default = "default_port")]
    pub port: u16,

    /// Authentication configuration.
    #[serde(default)]
    pub auth: AuthConfig,
}

/// Authentication configuration for the dashboard.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AuthConfig {
    /// Authentication type: "basic" or "oidc".
    #[serde(default, rename = "type")]
    pub auth_type: AuthType,

    /// Users for basic auth.
    #[serde(default)]
    pub users: Vec<BasicAuthUser>,

    /// OIDC configuration (when auth_type is "oidc").
    #[serde(default)]
    pub oidc: Option<OidcConfig>,
}

/// Authentication type.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AuthType {
    /// Basic username/password authentication.
    #[default]
    Basic,
    /// OpenID Connect authentication.
    Oidc,
}

/// Basic auth user.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BasicAuthUser {
    /// Username.
    pub username: String,
    /// Password (or environment variable reference).
    #[serde(default)]
    pub password: Option<String>,
    /// Environment variable containing the password.
    #[serde(default)]
    pub password_env: Option<String>,
}

impl BasicAuthUser {
    /// Get the password, checking password_env first.
    pub fn get_password(&self) -> Option<String> {
        // Try password_env first
        if let Some(env_var) = &self.password_env
            && let Ok(password) = std::env::var(env_var) {
                return Some(password);
            }
        // Fall back to direct password
        self.password.clone()
    }
}

/// OIDC configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OidcConfig {
    /// OIDC issuer URL.
    pub issuer: String,
    /// Client ID.
    pub client_id: String,
    /// Client secret (or environment variable reference).
    #[serde(default)]
    pub client_secret: Option<String>,
    /// Environment variable containing the client secret.
    #[serde(default)]
    pub client_secret_env: Option<String>,
    /// OAuth scopes to request.
    #[serde(default = "default_scopes")]
    pub scopes: Vec<String>,
    /// OAuth redirect URI.
    #[serde(default)]
    pub redirect_uri: Option<String>,
}

impl OidcConfig {
    /// Get the client secret, checking client_secret_env first.
    pub fn get_client_secret(&self) -> Option<String> {
        // Try client_secret_env first
        if let Some(env_var) = &self.client_secret_env
            && let Ok(secret) = std::env::var(env_var) {
                return Some(secret);
            }
        // Fall back to direct secret
        self.client_secret.clone()
    }
}

impl Default for DashboardConfig {
    fn default() -> Self {
        Self {
            enabled: default_enabled(),
            host: default_host(),
            port: default_port(),
            auth: AuthConfig::default(),
        }
    }
}

impl DashboardConfig {
    /// Get the port.
    pub fn get_port(&self) -> u16 {
        self.port
    }
}

fn default_enabled() -> bool {
    true
}

fn default_host() -> String {
    "127.0.0.1".to_string()
}

fn default_port() -> u16 {
    8080
}

fn default_scopes() -> Vec<String> {
    vec![
        "openid".to_string(),
        "profile".to_string(),
        "email".to_string(),
    ]
}
