//! Virtual schema configuration.

use serde::{Deserialize, Serialize};

/// Configuration for virtual schema filtering.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VirtualSchemaConfig {
    /// Whether virtual schema filtering is enabled.
    #[serde(default = "default_enabled")]
    pub enabled: bool,

    /// Tables visible to all tokens (e.g., reference data like countries, currencies).
    #[serde(default)]
    pub always_visible: Vec<String>,

    /// Whether to expose row counts in schema queries.
    /// Default: false (for security)
    #[serde(default)]
    pub expose_row_counts: bool,

    /// Whether to expose index information.
    /// Default: false (for security)
    #[serde(default)]
    pub expose_indexes: bool,

    /// The default schema to filter (typically "public").
    #[serde(default = "default_schema")]
    pub default_schema: String,
}

impl Default for VirtualSchemaConfig {
    fn default() -> Self {
        Self {
            enabled: default_enabled(),
            always_visible: Vec::new(),
            expose_row_counts: false,
            expose_indexes: false,
            default_schema: default_schema(),
        }
    }
}

impl VirtualSchemaConfig {
    /// Create a new configuration with virtual schema enabled.
    pub fn new() -> Self {
        Self::default()
    }

    /// Disable virtual schema filtering.
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            ..Self::default()
        }
    }

    /// Add a table that should always be visible.
    pub fn with_always_visible(mut self, table: impl Into<String>) -> Self {
        self.always_visible.push(table.into());
        self
    }

    /// Check if a table should always be visible.
    pub fn is_always_visible(&self, table: &str) -> bool {
        self.always_visible.iter().any(|t| t == table)
    }
}

fn default_enabled() -> bool {
    true
}

fn default_schema() -> String {
    "public".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = VirtualSchemaConfig::default();
        assert!(config.enabled);
        assert!(config.always_visible.is_empty());
        assert!(!config.expose_row_counts);
        assert!(!config.expose_indexes);
        assert_eq!(config.default_schema, "public");
    }

    #[test]
    fn test_always_visible() {
        let config = VirtualSchemaConfig::new()
            .with_always_visible("countries")
            .with_always_visible("currencies");

        assert!(config.is_always_visible("countries"));
        assert!(config.is_always_visible("currencies"));
        assert!(!config.is_always_visible("users"));
    }

    #[test]
    fn test_disabled() {
        let config = VirtualSchemaConfig::disabled();
        assert!(!config.enabled);
    }
}

