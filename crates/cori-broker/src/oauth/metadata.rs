//! OAuth authorization-server metadata for an MCP server.
//!
//! The migration plan calls for full RFC 8414 discovery + MCP
//! protected-resource discovery. v1 takes a pragmatic short-cut: the
//! relevant endpoints (authorization, token) and the `client_id` Cori
//! was registered as are written directly into the MCP server's entry
//! in `~/.cori/mcp-servers.json`. Full discovery + DCR is a follow-up.

use serde::{Deserialize, Serialize};

/// OAuth metadata for one MCP server, as read from `mcp-servers.json`.
///
/// ```json
/// {
///   "servers": {
///     "notion": {
///       "command": ["notion-mcp-server"],
///       "oauth": {
///         "authorization_endpoint": "https://api.notion.com/v1/oauth/authorize",
///         "token_endpoint":         "https://api.notion.com/v1/oauth/token",
///         "client_id":              "abcd-1234",
///         "scopes":                 ["read_content", "update_content"],
///         "token_env_var":          "NOTION_TOKEN"
///       }
///     }
///   }
/// }
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct McpOAuthConfig {
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub client_id: String,
    #[serde(default)]
    pub scopes: Vec<String>,
    /// Environment variable to inject the resolved access token into
    /// when spawning the MCP server. Defaults to `MCP_OAUTH_TOKEN`.
    #[serde(default = "default_token_env_var")]
    pub token_env_var: String,
}

fn default_token_env_var() -> String {
    "MCP_OAUTH_TOKEN".to_string()
}
