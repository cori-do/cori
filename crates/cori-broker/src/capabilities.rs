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

use cori_protocol::{WorkerIdentity, task_queue_for};
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
    discover_mcp_for_login(home)
}

/// Public-API alias used by `cori login` to enumerate configured MCP
/// servers and their `oauth` metadata. Identical semantics to the
/// private `discover_mcp` used by capability reporting.
pub fn discover_mcp_for_login(home: &Path) -> BTreeMap<String, McpServerConfig> {
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

// ---------------------------------------------------------------------------
// CapabilityReport (Phase 4) — what a worker advertises to the cluster.
// ---------------------------------------------------------------------------

/// What a worker reports about itself so the CLI planner can route
/// steps to the right queue.
///
/// Written to `~/.cori/cluster/<task_queue>.json` by `cori work`, read
/// by `cori run`'s planner. Liveness is **not** encoded here — Temporal
/// `DescribeTaskQueue` is the source of truth for "is a worker
/// polling". This descriptor only answers "what can I do, am I authed".
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityReport {
    pub identity: WorkerIdentity,
    pub task_queue: String,
    pub capabilities: Vec<Capability>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Capability {
    /// Stable id — `"gws"`, `"notion"`, `"openai"`, `"local_fs"`, …
    pub id: String,
    pub kind: CapabilityKind,
    /// True when the worker can use the capability right now. v1 = the
    /// underlying credential / binary is present. Phase 5 makes this
    /// OAuth-aware.
    pub authed: bool,
    /// Human-readable extra ("token expires in 42m", "/usr/bin/curl").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityKind {
    Cli,
    McpOauth,
    McpStatic,
    Llm,
    LocalFs,
}

impl CapabilityReport {
    /// Build a [`CapabilityReport`] from a worker's identity and its
    /// discovered local [`Capabilities`].
    ///
    /// A [`CapabilityKind::LocalFs`] entry is added iff the identity is
    /// [`WorkerIdentity::Person`] — service workers never advertise the
    /// requesting user's local disk.
    ///
    /// Phase 5: the `authed` bit is derived from real sources where
    /// possible. CLI adapters (`crate::cli_auth`) probe known CLIs;
    /// OAuth-configured MCP servers are checked against the token-store
    /// metadata at `credentials_dir`. CLIs without an adapter and MCP
    /// servers without OAuth are reported `authed = true` (the spawn
    /// itself will surface real failures).
    pub fn from_capabilities(identity: WorkerIdentity, caps: &Capabilities) -> Self {
        Self::from_capabilities_with(identity, caps, None)
    }

    /// Variant that consults a token store under `credentials_dir` for
    /// OAuth-configured MCP servers. Pass the absolute path to
    /// `~/.cori/credentials/` (or whatever override is in play).
    pub fn from_capabilities_with(
        identity: WorkerIdentity,
        caps: &Capabilities,
        credentials_dir: Option<&Path>,
    ) -> Self {
        use crate::cli_auth;
        use crate::oauth::{Owner, TokenKey, default_store};

        let task_queue = task_queue_for(&identity);
        let mut capabilities: Vec<Capability> = Vec::new();

        if matches!(identity, WorkerIdentity::Person { .. }) {
            capabilities.push(Capability {
                id: "local_fs".to_string(),
                kind: CapabilityKind::LocalFs,
                authed: true,
                detail: None,
            });
        }

        // Per-CLI auth state is best-effort: only known CLIs are probed.
        for (name, path) in &caps.cli_binaries {
            let authed = !matches!(
                cli_auth::check_known(name),
                cli_auth::AuthState::NeedsReauth { .. }
            );
            capabilities.push(Capability {
                id: name.clone(),
                kind: CapabilityKind::Cli,
                authed,
                detail: Some(path.display().to_string()),
            });
        }

        // Owner for token lookup: only `Person` workers have a per-user
        // OAuth store. `Service` pools use shared client-credentials in
        // a follow-up phase.
        let owner = match &identity {
            WorkerIdentity::Person { user_id } => Some(Owner::User(user_id.clone())),
            WorkerIdentity::Service { pool } => Some(Owner::Service(pool.clone())),
        };

        for (name, server_cfg) in &caps.mcp_servers {
            let (kind, authed) = if let Some(_oauth) = &server_cfg.oauth {
                let authed = match (credentials_dir, &owner) {
                    (Some(dir), Some(o)) => {
                        let store = default_store(dir.to_path_buf());
                        let key = TokenKey::new(name.clone(), o.clone());
                        match store.get(&key) {
                            Ok(Some(t)) => !t.is_expiring(0),
                            _ => false,
                        }
                    }
                    _ => false,
                };
                (CapabilityKind::McpOauth, authed)
            } else {
                (CapabilityKind::McpStatic, true)
            };
            capabilities.push(Capability {
                id: name.clone(),
                kind,
                authed,
                detail: None,
            });
        }
        for provider in &caps.llm_providers {
            capabilities.push(Capability {
                id: provider.clone(),
                kind: CapabilityKind::Llm,
                authed: true,
                detail: None,
            });
        }

        Self {
            identity,
            task_queue,
            capabilities,
        }
    }

    pub fn advertises(&self, id: &str) -> bool {
        self.capabilities.iter().any(|c| c.id == id && c.authed)
    }
}

/// Wrap a discovered [`Capabilities`] snapshot as a
/// [`CapabilityReport`] for the given identity.
pub fn report(identity: WorkerIdentity, caps: &Capabilities) -> CapabilityReport {
    CapabilityReport::from_capabilities(identity, caps)
}
