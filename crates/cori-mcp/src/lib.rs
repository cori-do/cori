//! # cori-mcp
//!
//! MCP (Model Context Protocol) server implementation for Cori.
//!
//! This crate provides an MCP server that exposes database actions as typed tools
//! for AI agents to consume. It supports:
//!
//! - **Role-Driven Tool Generation**: Tools are dynamically generated based on token claims
//! - **Dry-Run Mode**: Preview mutations before execution
//! - **Human-in-the-Loop**: Flag actions requiring approval
//! - **Multiple Transports**: stdio and HTTP
//! - **Constraint Enforcement**: Validate inputs against role-defined constraints
//!
//! ## Architecture
//!
//! ```text
//! AI Agent (Claude, GPT, etc.)
//!       │
//!       │ MCP protocol (list tools / call tool)
//!       ▼
//! ┌─────────────────┐
//! │  Cori MCP Server│
//! │  1. Verify token│  ← cori-biscuit
//! │  2. Generate    │  ← role_config + schema
//! │     tools       │
//! │  3. Map to SQL  │
//! │  4. Inject RLS  │  ← cori-rls
//! │  5. Dry-run or  │
//! │     execute     │
//! │  6. Return JSON │
//! └────────┬────────┘
//!          │
//!          ▼
//!    Upstream Postgres
//! ```
//!
//! ## Tool Generation
//!
//! Tools are generated dynamically based on the connecting token's role claims:
//!
//! | Tool Pattern | Generated When | Description |
//! |--------------|----------------|-------------|
//! | `get<Entity>(id)` | Table has readable columns | Fetch single record |
//! | `list<Entity>(filters)` | Table has readable columns | Query multiple records |
//! | `create<Entity>(data)` | Role has create + editable columns | Create new record |
//! | `update<Entity>(id, data)` | Role has update + editable columns | Modify record |
//! | `delete<Entity>(id)` | Role has delete permission | Remove record |
//!
//! ## Example Usage
//!
//! ```ignore
//! use cori_core::config::{McpConfig, RoleConfig};
//! use cori_mcp::McpServer;
//!
//! // Load role configuration
//! let role_config = RoleConfig::from_file("roles/support_agent.yaml")?;
//!
//! // Create MCP server with role-driven tools
//! let mut server = McpServer::new(McpConfig::default())
//!     .with_role_config(role_config)
//!     .with_tenant_id("client_a");
//!
//! // Generate tools from role permissions
//! server.generate_tools();
//!
//! // Start the server
//! server.run().await?;
//! ```

pub mod approval;
pub mod error;
pub mod executor;
pub mod http_transport;
pub mod protocol;
pub mod schema;
pub mod server;
pub mod tool_generator;
pub mod tools;

// Re-export sqlx types for convenience
pub use sqlx::PgPool;

// Re-export main types
pub use approval::{ApprovalError, ApprovalManager, ApprovalRequest, ApprovalStatus};
pub use error::McpError;
pub use executor::{ExecutionContext, ExecutionResult, ToolExecutor};
pub use protocol::{
    CallToolOptions, CallToolParams, DryRunResult, JsonRpcRequest, JsonRpcResponse,
    ToolAnnotations, ToolContent, ToolDefinition,
};
pub use schema::{ColumnSchema, DatabaseSchema, TableSchema};
pub use server::McpServer;
pub use tool_generator::ToolGenerator;
pub use tools::ToolRegistry;


