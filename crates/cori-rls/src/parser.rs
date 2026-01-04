//! SQL parsing and analysis.

use crate::error::RlsError;
use sqlparser::ast::{Statement, TableFactor, TableWithJoins};
use sqlparser::dialect::PostgreSqlDialect;
use sqlparser::parser::Parser;

/// Analyzes SQL statements to extract table and column references.
pub struct SqlAnalyzer {
    dialect: PostgreSqlDialect,
}

impl Clone for SqlAnalyzer {
    fn clone(&self) -> Self {
        Self {
            dialect: PostgreSqlDialect {},
        }
    }
}

impl Default for SqlAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}

impl SqlAnalyzer {
    /// Create a new SQL analyzer.
    pub fn new() -> Self {
        Self {
            dialect: PostgreSqlDialect {},
        }
    }

    /// Parse a SQL string into statements.
    pub fn parse(&self, sql: &str) -> Result<Vec<Statement>, RlsError> {
        Parser::parse_sql(&self.dialect, sql)
            .map_err(|e| RlsError::ParseError(e.to_string()))
    }

    /// Extract table names from a statement.
    pub fn extract_tables(&self, stmt: &Statement) -> Vec<TableReference> {
        let mut tables = Vec::new();
        self.visit_statement(stmt, &mut tables);
        tables
    }

    fn visit_statement(&self, stmt: &Statement, tables: &mut Vec<TableReference>) {
        match stmt {
            Statement::Query(query) => {
                if let Some(body) = query.body.as_select() {
                    for table_with_joins in &body.from {
                        self.visit_table_with_joins(table_with_joins, tables);
                    }
                }
            }
            Statement::Insert(insert) => {
                tables.push(TableReference {
                    name: insert.table.to_string(),
                    alias: None,
                    operation: SqlOperation::Insert,
                });
            }
            Statement::Update(update) => {
                if let Some(name) = self.extract_table_name(&update.table.relation) {
                    tables.push(TableReference {
                        name,
                        alias: None,
                        operation: SqlOperation::Update,
                    });
                }
            }
            Statement::Delete(delete) => {
                // Handle the from clause - it's a FromTable which contains tables
                self.visit_from_table(&delete.from, tables, SqlOperation::Delete);
            }
            _ => {}
        }
    }

    fn visit_from_table(
        &self,
        from_table: &sqlparser::ast::FromTable,
        tables: &mut Vec<TableReference>,
        operation: SqlOperation,
    ) {
        match from_table {
            sqlparser::ast::FromTable::WithFromKeyword(tables_with_joins) => {
                for twj in tables_with_joins {
                    self.visit_table_with_joins(twj, tables);
                }
            }
            sqlparser::ast::FromTable::WithoutKeyword(tables_with_joins) => {
                for twj in tables_with_joins {
                    self.visit_table_with_joins(twj, tables);
                }
            }
        }
        // Mark all tables with the correct operation
        for table in tables.iter_mut() {
            table.operation = operation;
        }
    }

    fn visit_table_with_joins(
        &self,
        table_with_joins: &TableWithJoins,
        tables: &mut Vec<TableReference>,
    ) {
        if let Some(name) = self.extract_table_name(&table_with_joins.relation) {
            let alias = self.extract_table_alias(&table_with_joins.relation);
            tables.push(TableReference {
                name,
                alias,
                operation: SqlOperation::Select,
            });
        }

        for join in &table_with_joins.joins {
            if let Some(name) = self.extract_table_name(&join.relation) {
                let alias = self.extract_table_alias(&join.relation);
                tables.push(TableReference {
                    name,
                    alias,
                    operation: SqlOperation::Select,
                });
            }
        }
    }

    fn extract_table_name(&self, table_factor: &TableFactor) -> Option<String> {
        match table_factor {
            TableFactor::Table { name, .. } => Some(name.to_string()),
            _ => None,
        }
    }

    fn extract_table_alias(&self, table_factor: &TableFactor) -> Option<String> {
        match table_factor {
            TableFactor::Table { alias, .. } => alias.as_ref().map(|a| a.name.value.clone()),
            _ => None,
        }
    }

    /// Check if a statement is a DDL statement.
    pub fn is_ddl(&self, stmt: &Statement) -> bool {
        matches!(
            stmt,
            Statement::CreateTable { .. }
                | Statement::AlterTable { .. }
                | Statement::Drop { .. }
                | Statement::Truncate { .. }
                | Statement::CreateIndex { .. }
                | Statement::CreateView { .. }
        )
    }

    /// Get the type of SQL operation.
    pub fn get_operation(&self, stmt: &Statement) -> SqlOperation {
        match stmt {
            Statement::Query(_) => SqlOperation::Select,
            Statement::Insert { .. } => SqlOperation::Insert,
            Statement::Update { .. } => SqlOperation::Update,
            Statement::Delete(_) => SqlOperation::Delete,
            Statement::CreateTable { .. }
            | Statement::AlterTable { .. }
            | Statement::Drop { .. }
            | Statement::Truncate { .. } => SqlOperation::Ddl,
            _ => SqlOperation::Other,
        }
    }
}

/// A reference to a table in a SQL statement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableReference {
    /// The table name.
    pub name: String,
    /// Optional alias.
    pub alias: Option<String>,
    /// The operation being performed on this table.
    pub operation: SqlOperation,
}

/// Types of SQL operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SqlOperation {
    Select,
    Insert,
    Update,
    Delete,
    Ddl,
    Other,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_select() {
        let analyzer = SqlAnalyzer::new();
        let stmts = analyzer.parse("SELECT * FROM users").unwrap();
        assert_eq!(stmts.len(), 1);

        let tables = analyzer.extract_tables(&stmts[0]);
        assert_eq!(tables.len(), 1);
        assert_eq!(tables[0].name, "users");
    }

    #[test]
    fn test_parse_join() {
        let analyzer = SqlAnalyzer::new();
        let stmts = analyzer
            .parse("SELECT * FROM orders o JOIN users u ON o.user_id = u.id")
            .unwrap();

        let tables = analyzer.extract_tables(&stmts[0]);
        assert_eq!(tables.len(), 2);
        assert_eq!(tables[0].name, "orders");
        assert_eq!(tables[0].alias, Some("o".to_string()));
        assert_eq!(tables[1].name, "users");
        assert_eq!(tables[1].alias, Some("u".to_string()));
    }

    #[test]
    fn test_detect_ddl() {
        let analyzer = SqlAnalyzer::new();
        
        let stmts = analyzer.parse("CREATE TABLE test (id INT)").unwrap();
        assert!(analyzer.is_ddl(&stmts[0]));

        let stmts = analyzer.parse("SELECT * FROM users").unwrap();
        assert!(!analyzer.is_ddl(&stmts[0]));
    }
}

