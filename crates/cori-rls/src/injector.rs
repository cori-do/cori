//! RLS predicate injection.

use cori_core::TenancyConfig;

use crate::error::RlsError;
use crate::parser::{SqlAnalyzer, SqlOperation};

/// Injects RLS (Row-Level Security) predicates into SQL statements.
#[derive(Clone)]
pub struct RlsInjector {
    analyzer: SqlAnalyzer,
    config: TenancyConfig,
}

impl RlsInjector {
    /// Create a new RLS injector with the given tenancy configuration.
    pub fn new(config: TenancyConfig) -> Self {
        Self {
            analyzer: SqlAnalyzer::new(),
            config,
        }
    }

    /// Inject RLS predicates into a SQL statement.
    ///
    /// Returns the rewritten SQL with tenant predicates injected.
    pub fn inject(&self, sql: &str, tenant_id: &str) -> Result<InjectionResult, RlsError> {
        let statements = self.analyzer.parse(sql)?;

        if statements.is_empty() {
            return Ok(InjectionResult {
                original_sql: sql.to_string(),
                rewritten_sql: sql.to_string(),
                tables_scoped: vec![],
                predicates_added: vec![],
            });
        }

        let stmt = &statements[0];

        // Check for DDL
        if self.analyzer.is_ddl(stmt) {
            return Err(RlsError::DdlNotAllowed {
                statement: sql.to_string(),
            });
        }

        let tables = self.analyzer.extract_tables(stmt);
        let operation = self.analyzer.get_operation(stmt);

        // Build predicates for each table
        let mut predicates_added = Vec::new();
        let mut tables_scoped = Vec::new();

        for table in &tables {
            // Strip schema prefix if present (e.g., "public.users" -> "users")
            let table_name = table.name.split('.').last().unwrap_or(&table.name);
            
            // Never inject RLS predicates into system catalog tables
            // These should be handled by the virtual schema handler
            if self.is_system_catalog_table(&table.name) {
                tracing::debug!(
                    table = table.name,
                    "Skipping RLS injection for system catalog table"
                );
                continue;
            }
            
            if let Some(tenant_column) = self.config.get_tenant_column(table_name) {
                let alias_or_name = table.alias.as_deref().unwrap_or(&table.name);
                let predicate = format!("{}.{} = '{}'", alias_or_name, tenant_column, tenant_id);
                predicates_added.push(predicate);
                tables_scoped.push(table_name.to_string());
            }
        }

        // Rewrite the SQL
        let rewritten_sql = self.rewrite_sql(sql, &predicates_added, operation)?;

        Ok(InjectionResult {
            original_sql: sql.to_string(),
            rewritten_sql,
            tables_scoped,
            predicates_added,
        })
    }

    fn rewrite_sql(
        &self,
        sql: &str,
        predicates: &[String],
        operation: SqlOperation,
    ) -> Result<String, RlsError> {
        if predicates.is_empty() {
            return Ok(sql.to_string());
        }

        let predicates_clause = predicates.join(" AND ");

        match operation {
            SqlOperation::Select | SqlOperation::Update | SqlOperation::Delete => {
                // Check if there's already a WHERE clause
                let sql_upper = sql.to_uppercase();
                if let Some(where_pos) = sql_upper.find(" WHERE ") {
                    // Insert after existing WHERE
                    let insert_pos = where_pos + 7; // " WHERE " is 7 chars
                    let (before, after) = sql.split_at(insert_pos);
                    Ok(format!("{}({}) AND {}", before, predicates_clause, after))
                } else {
                    // Find position to insert WHERE (before ORDER BY, LIMIT, etc.)
                    let insert_keywords = [" ORDER BY", " LIMIT", " GROUP BY", " HAVING", ";"];
                    let mut insert_pos = sql.len();
                    
                    for keyword in insert_keywords {
                        if let Some(pos) = sql_upper.find(keyword) {
                            if pos < insert_pos {
                                insert_pos = pos;
                            }
                        }
                    }
                    
                    let (before, after) = sql.split_at(insert_pos);
                    Ok(format!("{} WHERE {}{}", before.trim_end(), predicates_clause, after))
                }
            }
            SqlOperation::Insert => {
                // For INSERT, we need to inject the tenant column into VALUES
                // This is more complex and requires AST manipulation
                // For now, return a placeholder that indicates injection is needed
                tracing::warn!("INSERT RLS injection not yet fully implemented");
                Ok(sql.to_string())
            }
            _ => Ok(sql.to_string()),
        }
    }

    /// Check if a table is a system catalog table that should not have RLS predicates injected.
    fn is_system_catalog_table(&self, table_name: &str) -> bool {
        let table_lower = table_name.to_lowercase();
        
        // Check for explicit pg_catalog schema prefix
        if table_lower.starts_with("pg_catalog.") {
            return true;
        }
        
        // Check for information_schema prefix
        if table_lower.starts_with("information_schema.") {
            return true;
        }
        
        // Check for unqualified pg_* tables (in default search path)
        // Common system catalogs include: pg_class, pg_namespace, pg_type, pg_attribute,
        // pg_proc, pg_tables, pg_index, pg_constraint, etc.
        if table_lower.starts_with("pg_") {
            return true;
        }
        
        false
    }

    /// Explain what predicates would be injected without actually modifying the SQL.
    pub fn explain(&self, sql: &str, tenant_id: &str) -> Result<InjectionExplanation, RlsError> {
        let result = self.inject(sql, tenant_id)?;
        
        Ok(InjectionExplanation {
            original_sql: result.original_sql,
            rewritten_sql: result.rewritten_sql,
            tables_scoped: result.tables_scoped,
            predicates_added: result.predicates_added,
            tenant_id: tenant_id.to_string(),
        })
    }
}

/// Result of RLS injection.
#[derive(Debug, Clone)]
pub struct InjectionResult {
    /// The original SQL statement.
    pub original_sql: String,
    /// The rewritten SQL with RLS predicates.
    pub rewritten_sql: String,
    /// Tables that were scoped with tenant predicates.
    pub tables_scoped: Vec<String>,
    /// The predicates that were added.
    pub predicates_added: Vec<String>,
}

/// Explanation of RLS injection (for `cori proxy explain` command).
#[derive(Debug, Clone)]
pub struct InjectionExplanation {
    /// The original SQL statement.
    pub original_sql: String,
    /// The rewritten SQL with RLS predicates.
    pub rewritten_sql: String,
    /// Tables that were scoped.
    pub tables_scoped: Vec<String>,
    /// Predicates that were added.
    pub predicates_added: Vec<String>,
    /// The tenant ID used.
    pub tenant_id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_injector() -> RlsInjector {
        RlsInjector::new(TenancyConfig::default())
    }

    #[test]
    fn test_simple_select_injection() {
        let injector = default_injector();
        let result = injector
            .inject("SELECT * FROM orders WHERE status = 'pending'", "client_a")
            .unwrap();

        assert!(result.rewritten_sql.contains("tenant_id = 'client_a'"));
        assert!(result.rewritten_sql.contains("status = 'pending'"));
    }

    #[test]
    fn test_select_without_where() {
        let injector = default_injector();
        let result = injector
            .inject("SELECT * FROM orders", "client_a")
            .unwrap();

        assert!(result.rewritten_sql.contains("WHERE"));
        assert!(result.rewritten_sql.contains("tenant_id = 'client_a'"));
    }

    #[test]
    fn test_ddl_rejected() {
        let injector = default_injector();
        let result = injector.inject("DROP TABLE users", "client_a");

        assert!(matches!(result, Err(RlsError::DdlNotAllowed { .. })));
    }

    #[test]
    fn test_global_table_no_injection() {
        let mut config = TenancyConfig::default();
        config.global_tables.push("products".to_string());
        let injector = RlsInjector::new(config);

        let result = injector
            .inject("SELECT * FROM products", "client_a")
            .unwrap();

        // No tenant predicate should be added for global tables
        assert!(!result.rewritten_sql.contains("tenant_id"));
        assert!(result.tables_scoped.is_empty());
    }

    #[test]
    fn test_system_catalog_tables_no_injection() {
        let injector = default_injector();

        // Test unqualified pg_type
        let result = injector
            .inject("SELECT oid, typname FROM pg_type WHERE typname='geometry'", "client_a")
            .unwrap();
        assert!(!result.rewritten_sql.contains("organization_id"));
        assert!(result.tables_scoped.is_empty());

        // Test qualified pg_catalog.pg_class
        let result = injector
            .inject("SELECT relname FROM pg_catalog.pg_class WHERE relkind='r'", "client_a")
            .unwrap();
        assert!(!result.rewritten_sql.contains("organization_id"));
        assert!(result.tables_scoped.is_empty());

        // Test pg_namespace
        let result = injector
            .inject("SELECT nspname FROM pg_namespace", "client_a")
            .unwrap();
        assert!(!result.rewritten_sql.contains("organization_id"));
        assert!(result.tables_scoped.is_empty());

        // Test information_schema
        let result = injector
            .inject("SELECT table_name FROM information_schema.tables", "client_a")
            .unwrap();
        assert!(!result.rewritten_sql.contains("organization_id"));
        assert!(result.tables_scoped.is_empty());
    }

    #[test]
    fn test_mixed_system_and_user_tables() {
        let injector = default_injector();

        // A query joining user table with system catalog should only inject RLS on user table
        let result = injector
            .inject(
                "SELECT o.id, c.relname FROM orders o, pg_class c WHERE o.status = 'pending'",
                "client_a",
            )
            .unwrap();

        // Should have RLS for orders but not pg_class
        assert!(result.tables_scoped.contains(&"orders".to_string()));
        assert!(!result.tables_scoped.contains(&"pg_class".to_string()));
        assert_eq!(result.tables_scoped.len(), 1);
    }
}

