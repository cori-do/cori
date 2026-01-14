//! # cori-audit
//!
//! Audit logging for Cori MCP Server.
//!
//! This crate provides functionality for:
//! - Logging all MCP tool calls with [role - tenant - action - sql] format
//! - Tracking approval workflow events (approval_request / approved / denied)
//! - Storing audit events in files (JSON Lines) and console (human-readable)
//! - Querying audit history with filters
//!
//! ## Event Format
//!
//! Events follow the format: `[role - tenant - action - sql]`
//!
//! - **File output**: JSON Lines (one JSON object per line)
//! - **Console output**: Human-readable log lines
//!
//! ## Event Types
//!
//! | Event Type | Description |
//! |------------|-------------|
//! | `ToolCalled` | MCP tool was invoked |
//! | `QueryExecuted` | SQL query executed successfully |
//! | `QueryFailed` | SQL query execution failed |
//! | `ApprovalRequested` | Action requires human approval |
//! | `Approved` | Action was approved |
//! | `Denied` | Action was denied/rejected |
//! | `AuthenticationFailed` | Token verification failed |
//! | `AuthorizationDenied` | Request blocked by policy |
//!
//! ## Example Usage
//!
//! ```rust,no_run
//! use cori_audit::{AuditLogger, AuditEvent, AuditEventType};
//! use cori_core::AuditConfig;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! // Create logger from config
//! let config = AuditConfig::default();
//! let logger = AuditLogger::new(config)?;
//!
//! // Log a tool call
//! logger.log_tool_call(
//!     "support_agent",
//!     "client_a",
//!     "listOrders",
//!     Some("SELECT * FROM orders WHERE tenant_id = 'client_a'"),
//!     false,
//! ).await?;
//!
//! // Log an approval request
//! logger.log_approval_requested(
//!     "support_agent",
//!     "client_a",
//!     "deleteOrder",
//!     "apr_123",
//! ).await?;
//! # Ok(())
//! # }
//! ```

pub mod error;
pub mod event;
pub mod logger;
pub mod storage;

pub use error::AuditError;
pub use event::{AuditEvent, AuditEventBuilder, AuditEventType};
pub use logger::{AuditFilter, AuditLogger};
pub use storage::{AuditStorage, ConsoleStorage, DualStorage, FileStorage, NullStorage};
