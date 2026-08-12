//! Worker capability discovery and validation.
//!
//! Before any step runs, the broker resolves the set of capabilities the
//! current process can offer — which CLI binaries are on PATH, which MCP
//! servers are declared in `~/.cori/mcp-servers.json`, which LLM providers
//! have credentials configured. The CLI then cross-checks a workflow's
//! requirements against this snapshot and refuses to start if anything is
//! missing.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
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
    /// API provider names with usable credentials.
    pub llm_providers: BTreeSet<String>,
    /// Subscription backend ids (`claude`, `codex`, `cursor`,
    /// `gemini-cli`) that are installed *and* signed in, so they can
    /// serve an `llm` step from the user's own plan.
    #[serde(default)]
    pub llm_subscriptions: BTreeSet<String>,
}

impl Capabilities {
    pub fn has_cli(&self, name: &str) -> bool {
        self.cli_binaries.contains_key(name)
    }
    pub fn has_mcp(&self, name: &str) -> bool {
        self.mcp_servers.contains_key(name)
    }
    /// Can this machine serve an `llm` step at all, by either route?
    pub fn has_any_llm(&self) -> bool {
        !self.llm_providers.is_empty() || !self.llm_subscriptions.is_empty()
    }
}

/// Discover capabilities. `home` is the Cori home directory
/// (`~/.cori/`); `wanted_clis` is the set of CLI binary names the caller
/// cares about — only those are probed so we don't enumerate PATH for
/// nothing. `llm_creds` is the credential set the CLI resolved from
/// config + env; we report any provider whose key is present.
pub fn discover(home: &Path, wanted_clis: &[String], llm_creds: &LlmCredentials) -> Capabilities {
    discover_with_policy(
        home,
        wanted_clis,
        llm_creds,
        &crate::llm::LlmPolicy::default(),
        LlmProbe::Skip,
    )
}

/// Whether [`discover_with_policy`] should probe subscription backends.
///
/// Probing is not free: one backend (`cursor-agent status`) has no
/// readable credential file and must be asked, which spawns a process
/// that talks to the network. That is fine when the user is asking
/// "what can this machine do", and wrong on the hot path of every run —
/// so the caller states which situation it is in rather than paying the
/// cost unconditionally.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LlmProbe {
    /// Don't ask. Subscriptions are reported empty. Use when nothing in
    /// this call depends on them (no `llm` step, worker bootstrap).
    Skip,
    /// Ask each backend whether it is signed in. Use when the answer is
    /// the point: `cori status`, the Console's settings tab, or a
    /// preflight for a workflow that actually has an `llm` step.
    Probe,
}

impl LlmProbe {
    /// Probe only when the workflow being checked needs an LLM.
    ///
    /// Takes the same two fields [`validate`] reads, via
    /// [`workflow_needs_llm`], so the two can't disagree — skipping the
    /// probe for a workflow that `validate` then checks would report
    /// "no LLM backend" on a machine that has one.
    pub fn for_workflow(requires_llm: bool, required_llm_providers: &[String]) -> Self {
        if workflow_needs_llm(requires_llm, required_llm_providers) {
            LlmProbe::Probe
        } else {
            LlmProbe::Skip
        }
    }
}

/// Does this workflow need an LLM backend to run?
///
/// `requires_llm` is the compiler's answer. The `required_llm_providers`
/// fallback covers DAGs compiled before that flag existed — those always
/// named a concrete model.
pub fn workflow_needs_llm(requires_llm: bool, required_llm_providers: &[String]) -> bool {
    requires_llm || !required_llm_providers.is_empty()
}

/// [`discover`] with an explicit LLM policy, so a shared worker reports
/// no subscription backends even when the agent CLIs happen to be
/// installed on its host — a service worker must never advertise a
/// capability it is not allowed to use (see `crate::llm::policy`).
pub fn discover_with_policy(
    home: &Path,
    wanted_clis: &[String],
    llm_creds: &LlmCredentials,
    policy: &crate::llm::LlmPolicy,
    probe: LlmProbe,
) -> Capabilities {
    let cli_binaries = discover_clis(wanted_clis);
    let mcp_servers = discover_mcp(home);
    let llm_providers = llm_creds
        .configured_providers()
        .into_iter()
        .map(str::to_string)
        .collect();
    let llm_subscriptions = match probe {
        LlmProbe::Skip => BTreeSet::new(),
        LlmProbe::Probe => policy
            .subscription_order()
            .iter()
            .filter(|spec| crate::llm::subscription::check(spec).is_ready())
            .map(|spec| spec.id.to_string())
            .collect(),
    };
    Capabilities {
        cli_binaries,
        mcp_servers,
        llm_providers,
        llm_subscriptions,
    }
}

fn discover_clis(wanted: &[String]) -> BTreeMap<String, PathBuf> {
    // Registry capabilities are always probed in addition to the
    // caller's wanted set: a worker advertises every installed
    // Cori-blessed binary, and `cori status` / the MCP `status` tool
    // surface them without the caller having to know the registry.
    let mut names: BTreeSet<&str> = wanted.iter().map(String::as_str).collect();
    names.extend(crate::install::REGISTRY.iter().map(|s| s.id));

    let mut out = BTreeMap::new();
    for name in names {
        // PATH first, then Cori-managed installs in `~/.cori/bin`.
        if let Some(p) = crate::install::resolve_binary(name) {
            out.insert(name.to_string(), p);
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Registry advertisement — the one artifact every consumer derives from.
// ---------------------------------------------------------------------------

/// Advertisement row for one registry capability, installed or not.
///
/// This is what makes capability discovery *dynamic* for agents: `cori
/// status`, the MCP `status` tool, and `cori capability list --json`
/// all render this same struct, so adding a capability to
/// [`crate::install::REGISTRY`] advertises it everywhere at once — no
/// skill-prose edits, no per-consumer sidecars.
#[derive(Debug, Clone, Serialize)]
pub struct RegistryCapability {
    /// Capability id == executable name (`gws`, `anydoc`, `lightpanda`).
    pub id: String,
    pub display_name: String,
    /// Full human-facing detail (the Console tooltip text).
    pub details: String,
    /// One agent-facing line: when to reach for this capability.
    pub use_for: String,
    pub installed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// The capability has a sign-in step. `false` == installed is ready.
    pub requires_auth: bool,
    /// Auth probe result; `None` when not installed or auth-free.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authed: Option<bool>,
    /// The one command that makes this capability ready, when it isn't.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remedy: Option<String>,
}

/// Snapshot the capability registry with per-entry install and auth
/// state. Includes entries that are *not* installed — advertising what
/// could be one command away is the point.
pub fn registry_status() -> Vec<RegistryCapability> {
    crate::install::REGISTRY
        .iter()
        .map(|spec| {
            let path = crate::install::resolve_binary(spec.id);
            let installed = path.is_some();
            let requires_auth = crate::cli_auth::for_binary(spec.id).is_some();
            let authed = if installed && requires_auth {
                match crate::cli_auth::check_known(spec.id) {
                    crate::cli_auth::AuthState::Ok => Some(true),
                    crate::cli_auth::AuthState::NeedsReauth { .. } => Some(false),
                    crate::cli_auth::AuthState::Unknown => None,
                }
            } else {
                None
            };
            let remedy = if !installed {
                Some(format!("cori capability install {}", spec.id))
            } else if authed == Some(false) {
                Some(format!("cori login {}", spec.id))
            } else {
                None
            };
            RegistryCapability {
                id: spec.id.to_string(),
                display_name: spec.display_name.to_string(),
                details: spec.details.to_string(),
                use_for: spec.use_for.to_string(),
                installed,
                path: path.map(|p| p.display().to_string()),
                requires_auth,
                authed,
                remedy,
            }
        })
        .collect()
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
pub fn which_on_path(name: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    which_on_path_in(name, &path_var)
}

fn which_on_path_in(name: &str, path_var: &OsStr) -> Option<PathBuf> {
    let suffixes: &[&str] = if cfg!(windows) {
        &["", ".exe", ".cmd", ".bat"]
    } else {
        &[""]
    };
    for dir in std::env::split_paths(path_var) {
        for sfx in suffixes {
            let cand = dir.join(format!("{name}{sfx}"));
            if is_executable_file(&cand) {
                return Some(cand);
            }
        }
    }
    None
}

#[cfg(unix)]
fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    path.metadata()
        .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable_file(path: &Path) -> bool {
    path.is_file()
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
///
/// `requires_llm` is the workflow's `CompiledWorkflow::requires_llm`.
/// LLM checking is deliberately weaker than CLI/MCP checking: a step's
/// model name is a preference that resolves against any usable backend,
/// so the question is "can this machine run an LLM step at all", not
/// "does it hold this particular vendor's key". `required_llm_providers`
/// only sharpens the hint.
pub fn validate(
    capabilities: &Capabilities,
    required_clis: &[String],
    required_mcp: &[String],
    required_llm_providers: &[String],
    requires_llm: bool,
) -> Vec<MissingCapability> {
    let mut out = Vec::new();
    for c in required_clis {
        if !capabilities.has_cli(c) {
            let hint = if crate::install::spec_for(c).is_some() {
                format!(
                    "run `cori login {c}` (installs and signs in) or `cori capability install {c}`"
                )
            } else {
                format!("install `{c}` and ensure it is on PATH")
            };
            out.push(MissingCapability {
                kind: "CLI",
                name: c.clone(),
                hint,
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
    if workflow_needs_llm(requires_llm, required_llm_providers) && !capabilities.has_any_llm() {
        let preferred = required_llm_providers
            .first()
            .map(String::as_str)
            .unwrap_or("openai");
        out.push(MissingCapability {
            kind: "LLM backend",
            name: required_llm_providers
                .first()
                .cloned()
                .unwrap_or_else(|| "any".to_string()),
            hint: format!(
                "sign in to a subscription CLI (Claude Code, Codex, Cursor, or Gemini CLI) \
                 in Cori Console → Settings → AI Providers, or run `cori login {preferred}` \
                 to use an API key"
            ),
        });
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
                detail: Some("API key".to_string()),
            });
        }
        // Subscription backends are advertised the same way, so `cori
        // status` and the Console show both routes to an `llm` step.
        for id in &caps.llm_subscriptions {
            let detail = crate::llm::subscription::spec_for(id)
                .map(|spec| format!("{} subscription", spec.subscription_name));
            capabilities.push(Capability {
                id: id.clone(),
                kind: CapabilityKind::Llm,
                authed: true,
                detail,
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

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use super::*;
    #[cfg(unix)]
    use std::fs;

    #[cfg(unix)]
    #[test]
    fn which_on_path_skips_non_executable_files() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().expect("tempdir");
        let shadow_dir = temp.path().join("shadow");
        let executable_dir = temp.path().join("executable");
        fs::create_dir_all(&shadow_dir).expect("shadow dir");
        fs::create_dir_all(&executable_dir).expect("executable dir");

        let shadow = shadow_dir.join("tool");
        let executable = executable_dir.join("tool");
        fs::write(&shadow, "not executable").expect("shadow file");
        fs::write(&executable, "#!/bin/sh\n").expect("executable file");
        fs::set_permissions(&shadow, fs::Permissions::from_mode(0o644))
            .expect("shadow permissions");
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755))
            .expect("executable permissions");

        let path = std::env::join_paths([shadow_dir, executable_dir]).expect("PATH");
        assert_eq!(which_on_path_in("tool", &path), Some(executable));
    }
}
