//! # cori-dashboard
//!
//! Admin web dashboard for Cori MCP Server.
//!
//! This crate provides an embedded web UI for:
//! - Schema browser (view introspected tables, columns, foreign keys)
//! - Role management (create/edit roles with permissions)
//! - Token minting (generate role tokens, attenuate to tenant)
//! - Audit log viewer (query history filtered by tenant, role, etc.)
//! - Approvals queue (review pending human-in-the-loop actions)
//! - Settings (configure upstream DB, global defaults, guardrails)
//!
//! ## Tech Stack
//!
//! - Embedded in single binary (no separate deploy)
//! - Axum for HTTP server
//! - Static assets bundled via `rust-embed`
//! - HTMX + Alpine.js for interactivity (minimal JS)
//! - Tailwind CSS for styling

// Public modules
pub mod api_types;
pub mod error;
pub mod handlers;
pub mod pages;
pub mod pages_extra;
pub mod routes;
pub mod server;
pub mod state;
pub mod templates;

// Internal helper modules
mod schema_converter;
mod token_helpers;

// Re-exports
pub use error::DashboardError;
pub use server::DashboardServer;
pub use state::AppState;
