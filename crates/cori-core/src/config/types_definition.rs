//! Types definition for reusable semantic types.
//!
//! This module defines types for the semantic type definitions used for
//! input validation. Types are referenced by column rules in rules.yaml.
//!
//! # Types Location
//!
//! By convention, types are stored at `schema/types.yaml` relative to the
//! project root.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

use super::ConfigError;

/// Reusable semantic types for input validation.
///
/// This structure defines named types with validation patterns that can
/// be referenced by column rules in rules.yaml.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypesDefinition {
    /// Types version (semver format).
    pub version: String,

    /// Map of type name to definition.
    #[serde(default)]
    pub types: HashMap<String, TypeDef>,
}

impl Default for TypesDefinition {
    fn default() -> Self {
        Self {
            version: "1.0.0".to_string(),
            types: HashMap::new(),
        }
    }
}

impl TypesDefinition {
    /// Load types definition from a YAML file.
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let content = fs::read_to_string(path.as_ref())?;
        Self::from_yaml(&content)
    }

    /// Parse types definition from YAML content.
    pub fn from_yaml(content: &str) -> Result<Self, ConfigError> {
        serde_yaml::from_str(content).map_err(ConfigError::from)
    }

    /// Get a type definition by name.
    pub fn get_type(&self, name: &str) -> Option<&TypeDef> {
        self.types.get(name)
    }

    /// Check if a type exists.
    pub fn has_type(&self, name: &str) -> bool {
        self.types.contains_key(name)
    }
}

/// Definition of a semantic type.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TypeDef {
    /// Human-readable description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Regex pattern for validation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,

    /// Categorization tags (e.g., "pii", "sensitive").
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
}

impl TypeDef {
    /// Check if this type has a specific tag.
    pub fn has_tag(&self, tag: &str) -> bool {
        self.tags.iter().any(|t| t == tag)
    }

    /// Check if this type is marked as PII.
    pub fn is_pii(&self) -> bool {
        self.has_tag("pii")
    }

    /// Check if this type is marked as sensitive.
    pub fn is_sensitive(&self) -> bool {
        self.has_tag("sensitive")
    }

    /// Get the pattern for validation (if defined).
    pub fn get_pattern(&self) -> Option<&str> {
        self.pattern.as_deref()
    }

    /// Check if this type has a pattern defined.
    pub fn has_pattern(&self) -> bool {
        self.pattern.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_types_definition() {
        let yaml = r#"
version: "1.0.0"
types:
  email:
    description: "Email address"
    pattern: "^[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\\.[a-zA-Z]{2,}$"
    tags: [pii]
  phone:
    description: "Phone number (E.164 format)"
    pattern: "^\\+[1-9]\\d{1,14}$"
    tags: [pii]
  sku:
    description: "Product SKU"
    pattern: "^[A-Z]{3}-[0-9]{4}$"
"#;

        let types = TypesDefinition::from_yaml(yaml).unwrap();
        assert_eq!(types.version, "1.0.0");
        assert_eq!(types.types.len(), 3);

        let email = types.get_type("email").unwrap();
        assert!(email.is_pii());
        assert!(email.description.is_some());

        let sku = types.get_type("sku").unwrap();
        assert!(!sku.is_pii());
    }

    #[test]
    fn test_type_has_pattern() {
        let type_def = TypeDef {
            description: Some("Email".to_string()),
            pattern: Some(r"^[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}$".to_string()),
            tags: vec![],
        };

        assert!(type_def.has_pattern());
        assert!(type_def.get_pattern().is_some());
    }
}
