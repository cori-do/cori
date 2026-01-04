//! Token claims for role and agent tokens.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Claims contained in a role token (base token).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoleClaims {
    /// Role name (e.g., "support_agent", "analytics_agent").
    pub role: String,

    /// Accessible tables and their permissions.
    pub tables: HashMap<String, TablePermissions>,

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

    /// Optional description of the role.
    #[serde(default)]
    pub description: Option<String>,

    /// When the role token was minted.
    #[serde(default)]
    pub minted_at: Option<DateTime<Utc>>,
}

/// Permissions for a specific table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TablePermissions {
    /// Columns that can be read.
    pub readable: Vec<String>,

    /// Columns that can be edited, with optional constraints.
    #[serde(default)]
    pub editable: HashMap<String, ColumnConstraints>,

    /// Tenant column for this table (overrides default).
    #[serde(default)]
    pub tenant_column: Option<String>,
}

/// Constraints on an editable column.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ColumnConstraints {
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

/// Claims added when a role token is attenuated to an agent token.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentClaims {
    /// The role claims (inherited from base token).
    #[serde(flatten)]
    pub role: RoleClaims,

    /// Tenant ID this token is scoped to.
    pub tenant: String,

    /// When the token expires.
    #[serde(default)]
    pub expires_at: Option<DateTime<Utc>>,

    /// Source of attenuation (e.g., "dashboard", "cli").
    #[serde(default)]
    pub source: Option<String>,

    /// When the attenuation was performed.
    #[serde(default)]
    pub attenuated_at: Option<DateTime<Utc>>,
}

impl RoleClaims {
    /// Create new role claims.
    pub fn new(role: impl Into<String>) -> Self {
        Self {
            role: role.into(),
            tables: HashMap::new(),
            blocked_tables: Vec::new(),
            max_rows_per_query: None,
            max_affected_rows: None,
            blocked_operations: Vec::new(),
            description: None,
            minted_at: Some(Utc::now()),
        }
    }

    /// Add a table with read-only access.
    pub fn add_readable_table(mut self, table: impl Into<String>, columns: Vec<String>) -> Self {
        self.tables.insert(
            table.into(),
            TablePermissions {
                readable: columns,
                editable: HashMap::new(),
                tenant_column: None,
            },
        );
        self
    }

    /// Check if a table is accessible.
    pub fn can_access_table(&self, table: &str) -> bool {
        !self.blocked_tables.contains(&table.to_string()) && self.tables.contains_key(table)
    }

    /// Check if a column is readable.
    pub fn can_read_column(&self, table: &str, column: &str) -> bool {
        if let Some(perms) = self.tables.get(table) {
            perms.readable.iter().any(|c| c == column || c == "*")
        } else {
            false
        }
    }

    /// Check if a column is editable.
    pub fn can_edit_column(&self, table: &str, column: &str) -> bool {
        if let Some(perms) = self.tables.get(table) {
            perms.editable.contains_key(column)
        } else {
            false
        }
    }
}

impl AgentClaims {
    /// Check if the token has expired.
    pub fn is_expired(&self) -> bool {
        if let Some(expires_at) = self.expires_at {
            Utc::now() > expires_at
        } else {
            false
        }
    }

    /// Get time until expiration.
    pub fn time_until_expiration(&self) -> Option<chrono::Duration> {
        self.expires_at.map(|exp| exp - Utc::now())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_role_claims_creation() {
        let claims = RoleClaims::new("support_agent")
            .add_readable_table("customers", vec!["id".into(), "name".into(), "email".into()]);

        assert_eq!(claims.role, "support_agent");
        assert!(claims.can_access_table("customers"));
        assert!(claims.can_read_column("customers", "id"));
        assert!(!claims.can_read_column("customers", "password"));
    }

    #[test]
    fn test_blocked_tables() {
        let mut claims = RoleClaims::new("agent");
        claims.blocked_tables.push("users".to_string());

        assert!(!claims.can_access_table("users"));
    }
}

