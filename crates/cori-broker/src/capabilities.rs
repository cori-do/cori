//! Worker capability discovery and validation.
//!
//! Before any step runs, the broker resolves the set of capabilities the
//! current process can offer — which CLI binaries are on PATH, which MCP
//! servers are declared in `~/.cori/mcp-servers.json`, which LLM providers
//! have credentials configured. The CLI then cross-checks a workflow's
//! requirements against this snapshot and refuses to start if anything is
//! missing.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::llm::LlmCredentials;
use crate::mcp::McpServerConfig;

/// A snapshot of the worker's capabilities, suitable for printing and for
/// validating against a workflow's declared requirements.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Capabilities {
    /// Binary name → resolved path on PATH.
    pub cli_binaries: BTreeMap<String, PathBuf>,
    /// Server name → connection config.
    pub mcp_servers: BTreeMap<String, McpServerConfig>,
    /// Provider names with usable credentials.
    pub llm_providers: BTreeSet<String>,
}

impl Capabilities {
    pub fn has_cli(&self, name: &str) -> bool {
        self.cli_binaries.contains_key(name)
    }
    pub fn has_mcp(&self, name: &str) -> bool {
        self.mcp_servers.contains_key(name)
    }
}

/// Discover capabilities. `home` is the Cori home directory
/// (`~/.cori/`); `wanted_clis` is the set of CLI binary names the caller
/// cares about — only those are probed so we don't enumerate PATH for
/// nothing. `llm_creds` is the credential set the CLI resolved from
/// config + env; we report any provider whose key is present.
pub fn discover(home: &Path, wanted_clis: &[String], llm_creds: &LlmCredentials) -> Capabilities {
    let cli_binaries = discover_clis(wanted_clis);
    let mcp_servers = discover_mcp(home);
    let mut llm_providers = BTreeSet::new();
    if llm_creds.openai_api_key.is_some() {
        llm_providers.insert("openai".to_string());
    }
    if llm_creds.anthropic_api_key.is_some() {
        llm_providers.insert("anthropic".to_string());
    }
    if llm_creds.gemini_api_key.is_some() {
        llm_providers.insert("gemini".to_string());
    }
    Capabilities {
        cli_binaries,
        mcp_servers,
        llm_providers,
    }
}

fn discover_clis(wanted: &[String]) -> BTreeMap<String, PathBuf> {
    let mut out = BTreeMap::new();
    for name in wanted {
        if let Some(p) = which_on_path(name) {
            out.insert(name.clone(), p);
        }
    }
    out
}

#[derive(Debug, Deserialize)]
struct McpServersFile {
    #[serde(default)]
    servers: BTreeMap<String, McpServerConfig>,
}

fn discover_mcp(home: &Path) -> BTreeMap<String, McpServerConfig> {
    let path = home.join("mcp-servers.json");
    if !path.is_file() {
        return BTreeMap::new();
    }
    let src = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(_) => return BTreeMap::new(),
    };
    match serde_json::from_str::<McpServersFile>(&src) {
        Ok(f) => f.servers,
        Err(_) => BTreeMap::new(),
    }
}

/// Minimal cross-platform PATH lookup. Avoids adding a `which` dependency.
fn which_on_path(name: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    let suffixes: &[&str] = if cfg!(windows) {
        &["", ".exe", ".cmd", ".bat"]
    } else {
        &[""]
    };
    for dir in std::env::split_paths(&path_var) {
        for sfx in suffixes {
            let cand = dir.join(format!("{name}{sfx}"));
            if cand.is_file() {
                return Some(cand);
            }
        }
    }
    None
}

/// A missing capability, surfaced before a run starts.
#[derive(Debug, Clone)]
pub struct MissingCapability {
    pub kind: &'static str,
    pub name: String,
    pub hint: String,
}

impl std::fmt::Display for MissingCapability {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "missing {}: `{}` — {}", self.kind, self.name, self.hint)
    }
}

/// Return every requirement the snapshot does not satisfy. Empty vec ==
/// ready to run.
pub fn validate(
    capabilities: &Capabilities,
    required_clis: &[String],
    required_mcp: &[String],
    required_llm_providers: &[String],
) -> Vec<MissingCapability> {
    let mut out = Vec::new();
    for c in required_clis {
        if !capabilities.has_cli(c) {
            out.push(MissingCapability {
                kind: "CLI",
                name: c.clone(),
                hint: format!("install `{c}` and ensure it is on PATH"),
            });
        }
    }
    for s in required_mcp {
        if !capabilities.has_mcp(s) {
            out.push(MissingCapability {
                kind: "MCP server",
                name: s.clone(),
                hint: format!(
                    "declare `{s}` in ~/.cori/mcp-servers.json with a `command` to launch it"
                ),
            });
        }
    }
    for p in required_llm_providers {
        if !capabilities.llm_providers.contains(p) {
            let env_var = match p.as_str() {
                "openai" => "OPENAI_API_KEY",
                "anthropic" => "ANTHROPIC_API_KEY",
                "gemini" => "GEMINI_API_KEY",
                _ => "",
            };
            out.push(MissingCapability {
                kind: "LLM provider",
                name: p.clone(),
                hint: format!("set {env_var} or run `cori config set llm.{p}.api_key <key>`"),
            });
        }
    }
    out
}
