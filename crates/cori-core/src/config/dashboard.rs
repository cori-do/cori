//! Dashboard configuration.

use serde::{Deserialize, Serialize};

/// Configuration for the admin dashboard.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DashboardConfig {
    /// Whether the dashboard is enabled.
    #[serde(default = "default_enabled")]
    pub enabled: bool,

    /// Port to listen on.
    #[serde(default = "default_listen_port")]
    pub listen_port: u16,

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
}

impl Default for DashboardConfig {
    fn default() -> Self {
        Self {
            enabled: default_enabled(),
            listen_port: default_listen_port(),
            auth: AuthConfig::default(),
        }
    }
}

fn default_enabled() -> bool {
    true
}

fn default_listen_port() -> u16 {
    8080
}
