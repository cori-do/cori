//! # cori-dashboard
//!
//! Admin web dashboard for Cori AI Database Proxy.
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

pub mod error;
pub mod handlers;
pub mod routes;
pub mod server;

pub use error::DashboardError;
pub use server::DashboardServer;

