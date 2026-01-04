//! Virtual schema handler.
//!
//! This module provides the main handler for intercepting and processing
//! schema queries with filtered responses.

use super::config::VirtualSchemaConfig;
use super::detector::{SchemaQueryDetector, SchemaQueryInfo, SchemaQueryType};
use super::responses::{ResponseGenerator, SchemaPermissions, VirtualSchemaResponse};
use std::collections::HashMap;

/// Handler for virtual schema queries.
///
/// This handler intercepts queries to `information_schema` and `pg_catalog`
/// views and returns filtered responses based on the token's permissions.
#[derive(Clone)]
pub struct VirtualSchemaHandler {
    config: VirtualSchemaConfig,
    detector: SchemaQueryDetector,
}

impl Default for VirtualSchemaHandler {
    fn default() -> Self {
        Self::new(VirtualSchemaConfig::default())
    }
}

impl VirtualSchemaHandler {
    /// Create a new virtual schema handler with the given configuration.
    pub fn new(config: VirtualSchemaConfig) -> Self {
        Self {
            config,
            detector: SchemaQueryDetector::new(),
        }
    }

    /// Check if virtual schema filtering is enabled.
    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    /// Get the configuration.
    pub fn config(&self) -> &VirtualSchemaConfig {
        &self.config
    }

    /// Detect if a query is a schema introspection query.
    pub fn detect(&self, sql: &str) -> SchemaQueryInfo {
        self.detector.detect(sql)
    }

    /// Check if a query should be intercepted by the virtual schema handler.
    pub fn should_intercept(&self, sql: &str) -> bool {
        if !self.config.enabled {
            return false;
        }

        let info = self.detector.detect(sql);
        info.should_intercept()
    }

    /// Handle a schema query and return a filtered response.
    ///
    /// # Arguments
    /// * `sql` - The SQL query to handle
    /// * `accessible_tables` - Tables the token has access to
    /// * `readable_columns` - Readable columns per table
    ///
    /// # Returns
    /// `Some(VirtualSchemaResponse)` if the query was intercepted, `None` otherwise.
    pub fn handle(
        &self,
        sql: &str,
        accessible_tables: Vec<String>,
        readable_columns: HashMap<String, Vec<String>>,
    ) -> Option<VirtualSchemaResponse> {
        if !self.config.enabled {
            return None;
        }

        let query_info = self.detector.detect(sql);

        if !query_info.should_intercept() {
            return None;
        }

        // Build permissions from provided data
        let permissions = SchemaPermissions {
            accessible_tables,
            readable_columns,
            always_visible: self.config.always_visible.clone(),
            schema_name: self.config.default_schema.clone(),
        };

        tracing::debug!(
            query_type = ?query_info.query_type,
            primary_view = ?query_info.primary_view,
            tables = ?permissions.all_visible_tables(),
            "Intercepting schema query"
        );

        Some(ResponseGenerator::generate(&query_info, &permissions))
    }

    /// Handle a schema query with pre-built permissions.
    pub fn handle_with_permissions(
        &self,
        sql: &str,
        permissions: SchemaPermissions,
    ) -> Option<VirtualSchemaResponse> {
        if !self.config.enabled {
            return None;
        }

        let query_info = self.detector.detect(sql);

        if !query_info.should_intercept() {
            return None;
        }

        // Merge always_visible from config
        let permissions = SchemaPermissions {
            always_visible: {
                let mut visible = permissions.always_visible;
                for t in &self.config.always_visible {
                    if !visible.contains(t) {
                        visible.push(t.clone());
                    }
                }
                visible
            },
            schema_name: if permissions.schema_name.is_empty() {
                self.config.default_schema.clone()
            } else {
                permissions.schema_name
            },
            ..permissions
        };

        tracing::debug!(
            query_type = ?query_info.query_type,
            primary_view = ?query_info.primary_view,
            tables = ?permissions.all_visible_tables(),
            "Intercepting schema query"
        );

        Some(ResponseGenerator::generate(&query_info, &permissions))
    }

    /// Create a result that indicates the query type.
    pub fn analyze(&self, sql: &str) -> VirtualSchemaAnalysis {
        let query_info = self.detector.detect(sql);

        VirtualSchemaAnalysis {
            is_schema_query: query_info.should_intercept(),
            query_type: query_info.query_type,
            would_intercept: self.config.enabled && query_info.should_intercept(),
        }
    }
}

/// Result of analyzing a query for virtual schema interception.
#[derive(Debug, Clone)]
pub struct VirtualSchemaAnalysis {
    /// Whether this is a schema introspection query.
    pub is_schema_query: bool,
    /// The type of schema query.
    pub query_type: SchemaQueryType,
    /// Whether the query would be intercepted (based on config).
    pub would_intercept: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_handler() -> VirtualSchemaHandler {
        VirtualSchemaHandler::new(
            VirtualSchemaConfig::new().with_always_visible("countries"),
        )
    }

    fn create_test_permissions() -> (Vec<String>, HashMap<String, Vec<String>>) {
        let tables = vec!["customers".to_string(), "orders".to_string()];
        let mut columns = HashMap::new();
        columns.insert(
            "customers".to_string(),
            vec!["id".to_string(), "name".to_string(), "email".to_string()],
        );
        columns.insert(
            "orders".to_string(),
            vec![
                "id".to_string(),
                "customer_id".to_string(),
                "total".to_string(),
            ],
        );
        (tables, columns)
    }

    #[test]
    fn test_should_intercept_information_schema() {
        let handler = create_test_handler();
        assert!(handler.should_intercept("SELECT * FROM information_schema.tables"));
        assert!(handler.should_intercept("SELECT column_name FROM information_schema.columns"));
    }

    #[test]
    fn test_should_intercept_pg_catalog() {
        let handler = create_test_handler();
        assert!(handler.should_intercept("SELECT * FROM pg_catalog.pg_tables"));
        assert!(handler.should_intercept("SELECT * FROM pg_catalog.pg_class"));
    }

    #[test]
    fn test_should_not_intercept_regular_query() {
        let handler = create_test_handler();
        assert!(!handler.should_intercept("SELECT * FROM users"));
        assert!(!handler.should_intercept("SELECT * FROM orders WHERE id = 1"));
    }

    #[test]
    fn test_should_not_intercept_when_disabled() {
        let handler = VirtualSchemaHandler::new(VirtualSchemaConfig::disabled());
        assert!(!handler.should_intercept("SELECT * FROM information_schema.tables"));
    }

    #[test]
    fn test_handle_returns_response() {
        let handler = create_test_handler();
        let (tables, columns) = create_test_permissions();

        let response = handler.handle(
            "SELECT * FROM information_schema.tables WHERE table_schema = 'public'",
            tables,
            columns,
        );

        assert!(response.is_some());
        let response = response.unwrap();
        // Should have customers, orders, and countries (always_visible)
        assert_eq!(response.row_count, 3);
    }

    #[test]
    fn test_handle_returns_none_for_regular_query() {
        let handler = create_test_handler();
        let (tables, columns) = create_test_permissions();

        let response = handler.handle("SELECT * FROM users", tables, columns);

        assert!(response.is_none());
    }

    #[test]
    fn test_handle_filters_by_table_name() {
        let handler = create_test_handler();
        let (tables, columns) = create_test_permissions();

        let response = handler.handle(
            "SELECT * FROM information_schema.columns WHERE table_name = 'customers'",
            tables,
            columns,
        );

        assert!(response.is_some());
        let response = response.unwrap();
        // Should only have columns for customers table (3 columns)
        assert_eq!(response.row_count, 3);
    }

    #[test]
    fn test_analyze() {
        let handler = create_test_handler();

        let analysis = handler.analyze("SELECT * FROM information_schema.tables");
        assert!(analysis.is_schema_query);
        assert!(analysis.would_intercept);

        let analysis = handler.analyze("SELECT * FROM users");
        assert!(!analysis.is_schema_query);
        assert!(!analysis.would_intercept);
    }
}

