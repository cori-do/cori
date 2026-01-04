//! Schema query detection.
//!
//! This module detects when a SQL query targets `information_schema` or `pg_catalog`
//! views, and extracts information about what schema view is being queried.

use crate::parser::SqlAnalyzer;
use sqlparser::ast::{Expr, Query, Select, SelectItem, SetExpr, Statement, TableFactor};

/// Types of schema views that can be queried.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SchemaView {
    /// `information_schema.tables`
    InformationSchemaTables,
    /// `information_schema.columns`
    InformationSchemaColumns,
    /// `information_schema.table_constraints`
    InformationSchemaTableConstraints,
    /// `information_schema.key_column_usage`
    InformationSchemaKeyColumnUsage,
    /// `information_schema.schemata`
    InformationSchemaSchemata,
    /// `pg_catalog.pg_tables`
    PgCatalogPgTables,
    /// `pg_catalog.pg_attribute`
    PgCatalogPgAttribute,
    /// `pg_catalog.pg_class`
    PgCatalogPgClass,
    /// `pg_catalog.pg_namespace`
    PgCatalogPgNamespace,
    /// Other information_schema view (unknown)
    OtherInformationSchema,
    /// Other pg_catalog view (unknown)
    OtherPgCatalog,
}

impl SchemaView {
    /// Check if this is an information_schema view.
    pub fn is_information_schema(&self) -> bool {
        matches!(
            self,
            SchemaView::InformationSchemaTables
                | SchemaView::InformationSchemaColumns
                | SchemaView::InformationSchemaTableConstraints
                | SchemaView::InformationSchemaKeyColumnUsage
                | SchemaView::InformationSchemaSchemata
                | SchemaView::OtherInformationSchema
        )
    }

    /// Check if this is a pg_catalog view.
    pub fn is_pg_catalog(&self) -> bool {
        matches!(
            self,
            SchemaView::PgCatalogPgTables
                | SchemaView::PgCatalogPgAttribute
                | SchemaView::PgCatalogPgClass
                | SchemaView::PgCatalogPgNamespace
                | SchemaView::OtherPgCatalog
        )
    }

    /// Parse a schema view from a fully qualified table name.
    pub fn from_table_name(schema: &str, table: &str) -> Option<SchemaView> {
        let schema_lower = schema.to_lowercase();
        let table_lower = table.to_lowercase();

        match schema_lower.as_str() {
            "information_schema" => match table_lower.as_str() {
                "tables" => Some(SchemaView::InformationSchemaTables),
                "columns" => Some(SchemaView::InformationSchemaColumns),
                "table_constraints" => Some(SchemaView::InformationSchemaTableConstraints),
                "key_column_usage" => Some(SchemaView::InformationSchemaKeyColumnUsage),
                "schemata" => Some(SchemaView::InformationSchemaSchemata),
                _ => Some(SchemaView::OtherInformationSchema),
            },
            "pg_catalog" => match table_lower.as_str() {
                "pg_tables" => Some(SchemaView::PgCatalogPgTables),
                "pg_attribute" => Some(SchemaView::PgCatalogPgAttribute),
                "pg_class" => Some(SchemaView::PgCatalogPgClass),
                "pg_namespace" => Some(SchemaView::PgCatalogPgNamespace),
                _ => Some(SchemaView::OtherPgCatalog),
            },
            _ => None,
        }
    }
}

/// The type of schema query being performed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaQueryType {
    /// Query targeting information_schema views.
    InformationSchema(SchemaView),
    /// Query targeting pg_catalog views.
    PgCatalog(SchemaView),
    /// Not a schema query.
    NotSchemaQuery,
}

/// Information extracted from a schema query.
#[derive(Debug, Clone)]
pub struct SchemaQueryInfo {
    /// The type of schema query.
    pub query_type: SchemaQueryType,
    /// The primary schema view being queried.
    pub primary_view: SchemaView,
    /// Columns being selected (if extractable).
    pub selected_columns: Vec<String>,
    /// Filter on table_schema (if present).
    pub schema_filter: Option<String>,
    /// Filter on table_name (if present).
    pub table_name_filter: Option<String>,
    /// The original SQL query.
    pub original_sql: String,
}

impl SchemaQueryInfo {
    /// Check if this is a schema introspection query that should be intercepted.
    pub fn should_intercept(&self) -> bool {
        !matches!(self.query_type, SchemaQueryType::NotSchemaQuery)
    }
}

/// Detector for schema queries.
#[derive(Clone)]
pub struct SchemaQueryDetector {
    analyzer: SqlAnalyzer,
}

impl Default for SchemaQueryDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl SchemaQueryDetector {
    /// Create a new schema query detector.
    pub fn new() -> Self {
        Self {
            analyzer: SqlAnalyzer::new(),
        }
    }

    /// Detect if a SQL query is targeting schema views.
    pub fn detect(&self, sql: &str) -> SchemaQueryInfo {
        let statements = match self.analyzer.parse(sql) {
            Ok(stmts) => stmts,
            Err(_) => {
                return SchemaQueryInfo {
                    query_type: SchemaQueryType::NotSchemaQuery,
                    primary_view: SchemaView::OtherInformationSchema,
                    selected_columns: Vec::new(),
                    schema_filter: None,
                    table_name_filter: None,
                    original_sql: sql.to_string(),
                };
            }
        };

        if statements.is_empty() {
            return SchemaQueryInfo {
                query_type: SchemaQueryType::NotSchemaQuery,
                primary_view: SchemaView::OtherInformationSchema,
                selected_columns: Vec::new(),
                schema_filter: None,
                table_name_filter: None,
                original_sql: sql.to_string(),
            };
        }

        // Only process SELECT statements
        let stmt = &statements[0];
        if let Statement::Query(query) = stmt {
            self.analyze_query(query, sql)
        } else {
            SchemaQueryInfo {
                query_type: SchemaQueryType::NotSchemaQuery,
                primary_view: SchemaView::OtherInformationSchema,
                selected_columns: Vec::new(),
                schema_filter: None,
                table_name_filter: None,
                original_sql: sql.to_string(),
            }
        }
    }

    fn analyze_query(&self, query: &Query, original_sql: &str) -> SchemaQueryInfo {
        // Check if this is a SELECT query
        if let SetExpr::Select(select) = query.body.as_ref() {
            self.analyze_select(select, original_sql)
        } else {
            SchemaQueryInfo {
                query_type: SchemaQueryType::NotSchemaQuery,
                primary_view: SchemaView::OtherInformationSchema,
                selected_columns: Vec::new(),
                schema_filter: None,
                table_name_filter: None,
                original_sql: original_sql.to_string(),
            }
        }
    }

    fn analyze_select(&self, select: &Select, original_sql: &str) -> SchemaQueryInfo {
        // Extract table references from FROM clause
        let mut schema_view: Option<SchemaView> = None;
        let mut query_type = SchemaQueryType::NotSchemaQuery;

        for table_with_joins in &select.from {
            if let Some(view) = self.extract_schema_view(&table_with_joins.relation) {
                schema_view = Some(view);
                if view.is_information_schema() {
                    query_type = SchemaQueryType::InformationSchema(view);
                } else if view.is_pg_catalog() {
                    query_type = SchemaQueryType::PgCatalog(view);
                }
                break;
            }
        }

        let primary_view = schema_view.unwrap_or(SchemaView::OtherInformationSchema);

        // Extract selected columns
        let selected_columns = self.extract_selected_columns(&select.projection);

        // Extract filters from WHERE clause
        let (schema_filter, table_name_filter) = select
            .selection
            .as_ref()
            .map(|expr| self.extract_filters(expr))
            .unwrap_or((None, None));

        SchemaQueryInfo {
            query_type,
            primary_view,
            selected_columns,
            schema_filter,
            table_name_filter,
            original_sql: original_sql.to_string(),
        }
    }

    fn extract_schema_view(&self, table_factor: &TableFactor) -> Option<SchemaView> {
        match table_factor {
            TableFactor::Table { name, .. } => {
                // Check if this is a schema view reference
                // Name can be like "information_schema.tables" or just "tables"
                // Use to_string() and split by '.' to handle both quoted and unquoted names
                let full_name = name.to_string();
                let parts: Vec<&str> = full_name.split('.').collect();

                match parts.as_slice() {
                    [schema, table] => SchemaView::from_table_name(schema, table),
                    [table] => {
                        // Check if this is a pg_catalog table without explicit schema prefix
                        // PostgreSQL includes pg_catalog in the default search path
                        let table_lower = table.to_lowercase();
                        if table_lower.starts_with("pg_") {
                            // Treat unqualified pg_* tables as pg_catalog tables
                            SchemaView::from_table_name("pg_catalog", table)
                        } else {
                            None
                        }
                    }
                    _ => None,
                }
            }
            _ => None,
        }
    }

    fn extract_selected_columns(&self, projection: &[SelectItem]) -> Vec<String> {
        projection
            .iter()
            .filter_map(|item| match item {
                SelectItem::UnnamedExpr(Expr::Identifier(ident)) => Some(ident.value.clone()),
                SelectItem::ExprWithAlias { alias, .. } => Some(alias.value.clone()),
                SelectItem::Wildcard(_) => Some("*".to_string()),
                _ => None,
            })
            .collect()
    }

    fn extract_filters(&self, expr: &Expr) -> (Option<String>, Option<String>) {
        let mut schema_filter = None;
        let mut table_name_filter = None;

        self.visit_expr_for_filters(expr, &mut schema_filter, &mut table_name_filter);

        (schema_filter, table_name_filter)
    }

    fn visit_expr_for_filters(
        &self,
        expr: &Expr,
        schema_filter: &mut Option<String>,
        table_name_filter: &mut Option<String>,
    ) {
        match expr {
            Expr::BinaryOp { left, op, right } => {
                // Check for table_schema = 'xxx' or table_name = 'xxx'
                if matches!(op, sqlparser::ast::BinaryOperator::Eq) {
                    // Handle simple identifier (e.g., table_name = 'xxx')
                    if let (Expr::Identifier(ident), Expr::Value(value_with_span)) =
                        (left.as_ref(), right.as_ref())
                    {
                        let col_name = ident.value.to_lowercase();
                        if let Some(val_str) = self.extract_string_value(&value_with_span.value) {
                            self.match_filter_column(&col_name, val_str, schema_filter, table_name_filter);
                        }
                    }
                    // Handle compound identifier (e.g., tc.table_name = 'xxx')
                    else if let (Expr::CompoundIdentifier(idents), Expr::Value(value_with_span)) =
                        (left.as_ref(), right.as_ref())
                    {
                        if !idents.is_empty() {
                            let col_name = idents.last().unwrap().value.to_lowercase();
                            if let Some(val_str) = self.extract_string_value(&value_with_span.value) {
                                self.match_filter_column(&col_name, val_str, schema_filter, table_name_filter);
                            }
                        }
                    }
                    // Also check reversed operand order (simple identifier)
                    else if let (Expr::Value(value_with_span), Expr::Identifier(ident)) =
                        (left.as_ref(), right.as_ref())
                    {
                        let col_name = ident.value.to_lowercase();
                        if let Some(val_str) = self.extract_string_value(&value_with_span.value) {
                            self.match_filter_column(&col_name, val_str, schema_filter, table_name_filter);
                        }
                    }
                    // Also check reversed operand order (compound identifier)
                    else if let (Expr::Value(value_with_span), Expr::CompoundIdentifier(idents)) =
                        (left.as_ref(), right.as_ref())
                    {
                        if !idents.is_empty() {
                            let col_name = idents.last().unwrap().value.to_lowercase();
                            if let Some(val_str) = self.extract_string_value(&value_with_span.value) {
                                self.match_filter_column(&col_name, val_str, schema_filter, table_name_filter);
                            }
                        }
                    }
                }

                // Recurse into AND expressions
                if matches!(op, sqlparser::ast::BinaryOperator::And) {
                    self.visit_expr_for_filters(left, schema_filter, table_name_filter);
                    self.visit_expr_for_filters(right, schema_filter, table_name_filter);
                }
            }
            Expr::Nested(inner) => {
                self.visit_expr_for_filters(inner, schema_filter, table_name_filter);
            }
            _ => {}
        }
    }

    fn match_filter_column(
        &self,
        col_name: &str,
        val_str: String,
        schema_filter: &mut Option<String>,
        table_name_filter: &mut Option<String>,
    ) {
        match col_name {
            "table_schema" | "schemaname" => {
                *schema_filter = Some(val_str);
            }
            "table_name" | "tablename" | "relname" => {
                *table_name_filter = Some(val_str);
            }
            _ => {}
        }
    }

    fn extract_string_value(&self, value: &sqlparser::ast::Value) -> Option<String> {
        match value {
            sqlparser::ast::Value::SingleQuotedString(s) => Some(s.clone()),
            sqlparser::ast::Value::DoubleQuotedString(s) => Some(s.clone()),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_information_schema_tables() {
        let detector = SchemaQueryDetector::new();

        let info = detector.detect("SELECT * FROM information_schema.tables");
        assert!(info.should_intercept());
        assert_eq!(info.primary_view, SchemaView::InformationSchemaTables);
        assert!(matches!(
            info.query_type,
            SchemaQueryType::InformationSchema(_)
        ));
    }

    #[test]
    fn test_detect_information_schema_columns() {
        let detector = SchemaQueryDetector::new();

        let info =
            detector.detect("SELECT column_name FROM information_schema.columns WHERE table_name = 'users'");
        assert!(info.should_intercept());
        assert_eq!(info.primary_view, SchemaView::InformationSchemaColumns);
        assert_eq!(info.table_name_filter, Some("users".to_string()));
    }

    #[test]
    fn test_detect_pg_catalog_pg_tables() {
        let detector = SchemaQueryDetector::new();

        let info = detector.detect("SELECT tablename FROM pg_catalog.pg_tables WHERE schemaname = 'public'");
        assert!(info.should_intercept());
        assert_eq!(info.primary_view, SchemaView::PgCatalogPgTables);
        assert_eq!(info.schema_filter, Some("public".to_string()));
    }

    #[test]
    fn test_detect_regular_query() {
        let detector = SchemaQueryDetector::new();

        let info = detector.detect("SELECT * FROM users WHERE id = 1");
        assert!(!info.should_intercept());
        assert!(matches!(info.query_type, SchemaQueryType::NotSchemaQuery));
    }

    #[test]
    fn test_extract_schema_filter() {
        let detector = SchemaQueryDetector::new();

        let info = detector
            .detect("SELECT table_name FROM information_schema.tables WHERE table_schema = 'public'");
        assert_eq!(info.schema_filter, Some("public".to_string()));
    }

    #[test]
    fn test_schema_view_classification() {
        assert!(SchemaView::InformationSchemaTables.is_information_schema());
        assert!(!SchemaView::InformationSchemaTables.is_pg_catalog());
        assert!(SchemaView::PgCatalogPgTables.is_pg_catalog());
        assert!(!SchemaView::PgCatalogPgTables.is_information_schema());
    }

    #[test]
    fn test_detect_unqualified_pg_catalog_tables() {
        let detector = SchemaQueryDetector::new();

        // Test pg_type without schema prefix
        let info = detector.detect("SELECT oid, typname FROM pg_type WHERE typname='geometry'");
        assert!(info.should_intercept());
        assert!(info.primary_view.is_pg_catalog());

        // Test pg_class without schema prefix
        let info = detector.detect("SELECT relname FROM pg_class WHERE relkind='r'");
        assert!(info.should_intercept());
        assert_eq!(info.primary_view, SchemaView::PgCatalogPgClass);

        // Test pg_namespace without schema prefix
        let info = detector.detect("SELECT nspname FROM pg_namespace");
        assert!(info.should_intercept());
        assert_eq!(info.primary_view, SchemaView::PgCatalogPgNamespace);
    }

    #[test]
    fn test_detect_pg_catalog_joins() {
        let detector = SchemaQueryDetector::new();

        // Test JOIN between pg_class and pg_namespace (common pattern in schema introspection)
        let info = detector.detect(
            "SELECT p.relname, n.nspname FROM pg_class AS p JOIN pg_namespace AS n ON p.relnamespace=n.oid"
        );
        assert!(info.should_intercept());
        assert!(info.primary_view.is_pg_catalog());
    }

    #[test]
    fn test_detect_qualified_table_name_filter() {
        let detector = SchemaQueryDetector::new();

        // Test qualified column name (tc.table_name) - common in TablePlus queries
        let info = detector.detect(
            "SELECT tc.constraint_name, kc.column_name \
             FROM information_schema.table_constraints tc, information_schema.key_column_usage kc \
             WHERE tc.constraint_type = 'PRIMARY KEY' \
             AND kc.table_name = tc.table_name \
             AND kc.table_schema = tc.table_schema \
             AND kc.constraint_name = tc.constraint_name \
             AND tc.table_schema = 'public' \
             AND tc.table_name = 'communications'"
        );
        assert!(info.should_intercept());
        assert_eq!(info.primary_view, SchemaView::InformationSchemaTableConstraints);
        assert_eq!(info.schema_filter, Some("public".to_string()));
        assert_eq!(info.table_name_filter, Some("communications".to_string()));
    }
}

