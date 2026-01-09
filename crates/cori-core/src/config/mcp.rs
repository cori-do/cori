//! MCP server configuration.
//!
//! This module defines configuration for the MCP (Model Context Protocol) server.
//! Tools are auto-generated from schema and role permissions.

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

    /// HTTP host (only used when transport is HTTP).
    #[serde(default = "default_http_host")]
    pub host: String,

    /// HTTP port (only used when transport is HTTP).
    #[serde(default = "default_http_port")]
    pub port: u16,
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
            host: default_http_host(),
            port: default_http_port(),
        }
    }
}

impl McpConfig {
    /// Get the HTTP port.
    pub fn get_port(&self) -> u16 {
        self.port
    }

    /// Check if using HTTP transport.
    pub fn is_http(&self) -> bool {
        self.transport == Transport::Http
    }

    /// Check if using stdio transport.
    pub fn is_stdio(&self) -> bool {
        self.transport == Transport::Stdio
    }
}

fn default_enabled() -> bool {
    true
}

fn default_http_host() -> String {
    "127.0.0.1".to_string()
}

fn default_http_port() -> u16 {
    3000
}
