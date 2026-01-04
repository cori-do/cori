//! MCP server configuration.

use serde::{Deserialize, Serialize};

/// Configuration for the MCP server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpConfig {
    /// Whether the MCP server is enabled.
    #[serde(default = "default_enabled")]
    pub enabled: bool,

    /// Transport type: "stdio" or "http".
    #[serde(default)]
    pub transport: Transport,

    /// HTTP port (only used when transport is HTTP).
    #[serde(default = "default_http_port")]
    pub http_port: u16,

    /// Whether dry-run mode is enabled.
    #[serde(default = "default_dry_run_enabled")]
    pub dry_run_enabled: bool,

    /// Whether to auto-generate MCP tools from schema.
    #[serde(default = "default_enabled")]
    pub auto_generate_tools: bool,

    /// Actions that require human approval (glob patterns).
    #[serde(default)]
    pub require_approval: Vec<String>,

    /// Exceptions to approval requirements.
    #[serde(default)]
    pub approval_exceptions: Vec<String>,
}

/// MCP transport type.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Transport {
    /// Standard input/output transport (for Claude Desktop, etc.).
    #[default]
    Stdio,
    /// HTTP transport.
    Http,
}

impl Default for McpConfig {
    fn default() -> Self {
        Self {
            enabled: default_enabled(),
            transport: Transport::default(),
            http_port: default_http_port(),
            dry_run_enabled: default_dry_run_enabled(),
            auto_generate_tools: default_enabled(),
            require_approval: Vec::new(),
            approval_exceptions: Vec::new(),
        }
    }
}

fn default_enabled() -> bool {
    true
}

fn default_http_port() -> u16 {
    3000
}

fn default_dry_run_enabled() -> bool {
    true
}
