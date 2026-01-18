//! Group definition for approval groups.
//!
//! This module defines types for approval groups. Groups contain members
//! identified by email addresses and are used for human-in-the-loop actions.
//!
//! # Groups Location
//!
//! By convention, groups are stored at `groups/*.yaml` relative to the
//! project root (one file per group).

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

use super::ConfigError;

/// Approval group definition.
///
/// Groups define who can approve human-in-the-loop actions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupDefinition {
    /// Unique identifier for this group.
    ///
    /// Must be lowercase alphanumeric with underscores (e.g., "support_managers").
    pub name: String,

    /// Human-readable description of the group's purpose.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// List of group members identified by email addresses.
    pub members: Vec<String>,
}

impl GroupDefinition {
    /// Load group definition from a YAML file.
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let content = fs::read_to_string(path.as_ref())?;
        Self::from_yaml(&content)
    }

    /// Parse group definition from YAML content.
    pub fn from_yaml(content: &str) -> Result<Self, ConfigError> {
        serde_yaml::from_str(content).map_err(ConfigError::from)
    }

    /// Check if an email is a member of this group.
    pub fn has_member(&self, email: &str) -> bool {
        self.members.iter().any(|m| m.eq_ignore_ascii_case(email))
    }

    /// Get the number of members in this group.
    pub fn member_count(&self) -> usize {
        self.members.len()
    }

    /// Validate the group definition.
    pub fn validate(&self) -> Result<(), String> {
        // Name must be non-empty and lowercase alphanumeric with underscores
        if self.name.is_empty() {
            return Err("Group name cannot be empty".to_string());
        }

        if !self
            .name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        {
            return Err(format!(
                "Group name '{}' must be lowercase alphanumeric with underscores",
                self.name
            ));
        }

        if !self
            .name
            .chars()
            .next()
            .map(|c| c.is_ascii_lowercase())
            .unwrap_or(false)
        {
            return Err(format!(
                "Group name '{}' must start with a lowercase letter",
                self.name
            ));
        }

        // Must have at least one member
        if self.members.is_empty() {
            return Err("Group must have at least one member".to_string());
        }

        // All members should look like email addresses (basic check)
        for member in &self.members {
            if !member.contains('@') || !member.contains('.') {
                return Err(format!(
                    "Invalid email address '{}' in group '{}'",
                    member, self.name
                ));
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_group_definition() {
        let yaml = r#"
name: support_managers
description: "Managers who can approve support ticket priority changes"
members:
  - alice.manager@example.com
  - bob.lead@example.com
"#;

        let group = GroupDefinition::from_yaml(yaml).unwrap();
        assert_eq!(group.name, "support_managers");
        assert_eq!(group.members.len(), 2);
        assert!(group.has_member("alice.manager@example.com"));
        assert!(group.has_member("ALICE.MANAGER@example.com")); // Case-insensitive
        assert!(!group.has_member("unknown@example.com"));
    }

    #[test]
    fn test_group_validation() {
        let valid_group = GroupDefinition {
            name: "support_managers".to_string(),
            description: Some("Support managers".to_string()),
            members: vec!["alice@example.com".to_string()],
        };
        assert!(valid_group.validate().is_ok());

        let empty_name = GroupDefinition {
            name: "".to_string(),
            description: None,
            members: vec!["alice@example.com".to_string()],
        };
        assert!(empty_name.validate().is_err());

        let invalid_name = GroupDefinition {
            name: "Support-Managers".to_string(),
            description: None,
            members: vec!["alice@example.com".to_string()],
        };
        assert!(invalid_name.validate().is_err());

        let no_members = GroupDefinition {
            name: "empty_group".to_string(),
            description: None,
            members: vec![],
        };
        assert!(no_members.validate().is_err());

        let invalid_email = GroupDefinition {
            name: "test_group".to_string(),
            description: None,
            members: vec!["not-an-email".to_string()],
        };
        assert!(invalid_email.validate().is_err());
    }
}
