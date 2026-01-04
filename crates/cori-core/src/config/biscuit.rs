//! Biscuit token configuration.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Configuration for Biscuit token handling.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BiscuitConfig {
    /// Environment variable containing the public key (hex-encoded).
    #[serde(default)]
    pub public_key_env: Option<String>,

    /// Path to the public key file.
    #[serde(default)]
    pub public_key_file: Option<PathBuf>,

    /// Environment variable containing the private key (hex-encoded).
    #[serde(default)]
    pub private_key_env: Option<String>,

    /// Path to the private key file.
    #[serde(default)]
    pub private_key_file: Option<PathBuf>,

    /// Whether to require tokens to have an expiration claim.
    #[serde(default = "default_true")]
    pub require_expiration: bool,

    /// Maximum token lifetime for newly minted tokens (e.g., "30d", "24h").
    #[serde(default)]
    pub max_token_lifetime: Option<String>,
}

impl BiscuitConfig {
    /// Resolve the public key from environment or file.
    pub fn resolve_public_key(&self) -> Result<Option<String>, std::io::Error> {
        // Try environment variable first
        if let Some(env_var) = &self.public_key_env {
            if let Ok(key) = std::env::var(env_var) {
                return Ok(Some(key));
            }
        }

        // Try file path
        if let Some(path) = &self.public_key_file {
            if path.exists() {
                let key = std::fs::read_to_string(path)?;
                return Ok(Some(key.trim().to_string()));
            }
        }

        Ok(None)
    }

    /// Resolve the private key from environment or file.
    pub fn resolve_private_key(&self) -> Result<Option<String>, std::io::Error> {
        // Try environment variable first
        if let Some(env_var) = &self.private_key_env {
            if let Ok(key) = std::env::var(env_var) {
                return Ok(Some(key));
            }
        }

        // Try file path
        if let Some(path) = &self.private_key_file {
            if path.exists() {
                let key = std::fs::read_to_string(path)?;
                return Ok(Some(key.trim().to_string()));
            }
        }

        Ok(None)
    }
}

fn default_true() -> bool {
    true
}
