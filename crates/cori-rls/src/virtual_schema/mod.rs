//! Virtual Schema implementation for Cori.
//!
//! This module intercepts queries to `information_schema` and `pg_catalog` views,
//! returning filtered responses based on token permissions. This prevents AI agents
//! from discovering tables and columns they cannot access.
//!
//! ## Intercepted Views
//!
//! | Schema Object | Description |
//! |---------------|-------------|
//! | `information_schema.tables` | Lists only token-accessible tables |
//! | `information_schema.columns` | Lists only readable columns per allowed table |
//! | `information_schema.table_constraints` | Constraints for accessible tables only |
//! | `information_schema.key_column_usage` | Foreign keys between accessible tables only |
//! | `pg_catalog.pg_tables` | Postgres-native table listing (filtered) |
//! | `pg_catalog.pg_attribute` | Column metadata (filtered) |
//!
//! ## Example
//!
//! Given a token with access to `customers` and `tickets` tables:
//!
//! ```text
//! Agent query:
//!   SELECT table_name FROM information_schema.tables WHERE table_schema = 'public';
//!
//! Unfiltered response (what Postgres would return):
//!   customers, tickets, users, billing, api_keys
//!
//! Virtual schema response (what agent sees):
//!   customers, tickets
//! ```

mod config;
mod detector;
mod handler;
mod responses;

pub use config::VirtualSchemaConfig;
pub use detector::{SchemaQueryDetector, SchemaQueryInfo, SchemaQueryType, SchemaView};
pub use handler::{VirtualSchemaAnalysis, VirtualSchemaHandler};
pub use responses::{ColumnDef, ResponseGenerator, SchemaPermissions, VirtualSchemaResponse};

