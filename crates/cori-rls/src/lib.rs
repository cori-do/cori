//! # cori-rls
//!
//! SQL parsing and Row-Level Security (RLS) injection for Cori.
//!
//! This crate provides functionality to:
//! - Parse SQL queries using `sqlparser`
//! - Analyze queries to identify tables and columns
//! - Inject tenant predicates (RLS) based on token claims
//! - Validate queries against role permissions
//!
//! ## How It Works
//!
//! Cori parses incoming SQL and automatically injects tenant predicates:
//!
//! **Before (from agent):**
//! ```sql
//! SELECT * FROM orders WHERE status = 'pending'
//! ```
//!
//! **After (to Postgres):**
//! ```sql
//! SELECT * FROM orders WHERE status = 'pending' AND tenant_id = 'client_a'
//! ```
//!
//! ## Supported Operations
//!
//! | Operation | RLS Behavior |
//! |-----------|--------------|
//! | `SELECT`  | Add `WHERE tenant_column = ?` |
//! | `INSERT`  | Inject tenant value into column |
//! | `UPDATE`  | Add `WHERE tenant_column = ?` |
//! | `DELETE`  | Add `WHERE tenant_column = ?` |
//! | `JOIN`    | Ensure all joined tables are tenant-scoped |

pub mod error;
pub mod injector;
pub mod parser;
pub mod virtual_schema;

pub use error::RlsError;
pub use injector::{InjectionResult, RlsInjector};
pub use parser::SqlAnalyzer;
pub use virtual_schema::{
    ColumnDef, ResponseGenerator, SchemaPermissions, SchemaQueryDetector, SchemaQueryInfo,
    SchemaQueryType, SchemaView, VirtualSchemaAnalysis, VirtualSchemaConfig, VirtualSchemaHandler,
    VirtualSchemaResponse,
};

