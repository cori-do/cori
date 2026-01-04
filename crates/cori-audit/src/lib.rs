//! # cori-audit
//!
//! Audit logging for Cori AI Database Proxy.
//!
//! This crate provides functionality for:
//! - Logging all queries with tenant, role, timing, and outcome
//! - Storing audit events in various backends (file, database, etc.)
//! - Querying audit history with filters
//! - Optional tamper-evident integrity (hashing, chaining)
//!
//! ## Audit Event Types
//!
//! | Event Type | Description |
//! |------------|-------------|
//! | `QueryReceived` | A query was received from a client |
//! | `QueryRewritten` | RLS predicates were injected |
//! | `QueryExecuted` | Query was forwarded and executed |
//! | `QueryFailed` | Query execution failed |
//! | `AuthenticationFailed` | Token verification failed |
//! | `AuthorizationDenied` | Query was blocked by policy |

pub mod error;
pub mod event;
pub mod logger;
pub mod storage;

pub use error::AuditError;
pub use event::{AuditEvent, AuditEventBuilder, AuditEventType};
pub use logger::AuditLogger;

