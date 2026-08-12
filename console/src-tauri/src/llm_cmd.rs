//! LLM backend management — API keys *and* subscriptions.
//!
//! Two routes serve an `llm` step, and this module exposes both to the
//! launcher:
//!
//! - **API keys.** Stored in the shared secret store (`cori-secrets`: OS
//!   keychain, file fallback), the same entries the CLI reads at run
//!   time, so a key saved here is immediately usable by `cori run` /
//!   `cori mcp` and vice versa. Values never travel back to the frontend
//!   — the UI only ever sees booleans derived from the non-secret index.
//!   `set_llm_provider_key` verifies the key against the provider's
//!   models endpoint before storing: an explicit 401/403 rejects with a
//!   clear message, while transport failures (offline) store anyway —
//!   never lock the user out of saving a key because the network is down.
//!
//! - **Subscriptions.** The agent CLIs already signed in on this
//!   machine (Claude Code, Codex, Cursor, Gemini CLI). Cori stores no
//!   credential for these; it probes the vendor's own session and spends
//!   it through the CLI. Nothing to save, so the UI shows install/sign-in
//!   state and the remedy command.
//!
//! # Why this lives only in the launcher
//!
//! Subscriptions are a property of *a person's machine*, so the mode
//! that selects between the two routes is written here, by the launcher.
//! A deployed worker (`cori work --shared <pool>`) reads the same config
//! file but is forced to API-only by the identity gate in
//! `cori_broker::llm::policy`, so the setting is only ever *in force* on
//! a machine someone is signed in to. The settings tab says as much
//! rather than implying it applies to shared workers too.

use cori_broker::llm::LlmCredentials;
use cori_broker::llm::catalog::{self, ModelTier};
use cori_broker::llm::policy::{Backend, Deployment, LlmPolicy};
use cori_broker::llm::resolve;
use cori_broker::llm::subscription::{self, BackendState};
use serde::Serialize;

use crate::error::{IpcError, IpcResult};

const PROVIDERS: [(&str, &str); 3] = [
    ("openai", "OpenAI"),
    ("anthropic", "Anthropic Claude"),
    ("gemini", "Google Gemini"),
];

#[derive(Debug, Clone, Serialize)]
pub struct LlmProviderInfo {
    pub id: String,
    pub display_name: String,
    /// A key is stored for this provider (from the non-secret index).
    pub configured: bool,
    /// An env var (e.g. `ANTHROPIC_API_KEY`) is set in the Console's
    /// environment and overrides the stored key at run time.
    pub env_override: bool,
    /// Secrets go to the OS keychain (false → 0600 file fallback).
    pub keychain: bool,
}

#[tauri::command(rename_all = "snake_case")]
pub async fn list_llm_providers() -> IpcResult<Vec<LlmProviderInfo>> {
    tokio::task::spawn_blocking(list_blocking)
        .await
        .map_err(|e| IpcError::Internal(anyhow::anyhow!("llm provider list join: {e}")))?
}

#[tauri::command(rename_all = "snake_case")]
pub async fn set_llm_provider_key(provider: String, api_key: String) -> IpcResult<LlmProviderInfo> {
    tokio::task::spawn_blocking(move || set_blocking(&provider, &api_key))
        .await
        .map_err(|e| IpcError::Internal(anyhow::anyhow!("llm provider set join: {e}")))?
}

#[tauri::command(rename_all = "snake_case")]
pub async fn remove_llm_provider_key(provider: String) -> IpcResult<LlmProviderInfo> {
    tokio::task::spawn_blocking(move || remove_blocking(&provider))
        .await
        .map_err(|e| IpcError::Internal(anyhow::anyhow!("llm provider remove join: {e}")))?
}

fn list_blocking() -> IpcResult<Vec<LlmProviderInfo>> {
    let store = open_store()?;
    let env = LlmCredentials::from_env();
    Ok(PROVIDERS
        .iter()
        .map(|(id, name)| info_for(&store, &env, id, name))
        .collect())
}

fn set_blocking(provider: &str, api_key: &str) -> IpcResult<LlmProviderInfo> {
    let (id, name) = lookup(provider)?;
    let api_key = api_key.trim();
    if api_key.is_empty() {
        return Err(IpcError::BadRequest("no API key entered".into()));
    }
    verify_key(id, api_key)?;

    let store = open_store()?;
    store
        .set(&cori_secrets::llm_account(id), api_key)
        .map_err(|e| IpcError::Internal(anyhow::anyhow!("storing the {id} API key: {e}")))?;
    Ok(info_for(&store, &LlmCredentials::from_env(), id, name))
}

fn remove_blocking(provider: &str) -> IpcResult<LlmProviderInfo> {
    let (id, name) = lookup(provider)?;
    let store = open_store()?;
    store
        .delete(&cori_secrets::llm_account(id))
        .map_err(|e| IpcError::Internal(anyhow::anyhow!("deleting the {id} API key: {e}")))?;
    Ok(info_for(&store, &LlmCredentials::from_env(), id, name))
}

fn open_store() -> IpcResult<cori_secrets::SecretStore> {
    cori_secrets::SecretStore::open_default()
        .map_err(|e| IpcError::Internal(anyhow::anyhow!("opening the secret store: {e}")))
}

fn lookup(provider: &str) -> IpcResult<(&'static str, &'static str)> {
    PROVIDERS
        .iter()
        .find(|(id, _)| *id == provider)
        .copied()
        .ok_or_else(|| IpcError::BadRequest(format!("unknown LLM provider `{provider}`")))
}

fn info_for(
    store: &cori_secrets::SecretStore,
    env: &LlmCredentials,
    id: &'static str,
    name: &str,
) -> LlmProviderInfo {
    LlmProviderInfo {
        id: id.to_string(),
        display_name: name.to_string(),
        configured: store.is_configured(&cori_secrets::llm_account(id)),
        env_override: env.key_for(id).is_some(),
        keychain: store.uses_keychain(),
    }
}

// ---------------------------------------------------------------------------
// Backends — the unified priority list
// ---------------------------------------------------------------------------

/// One row of the AI Providers list: a place an `llm` step can run.
///
/// Subscriptions and API providers are deliberately the *same* shape.
/// The user ranks them against each other in one list, so the UI should
/// not need two code paths to render them — the only differences are
/// which fields are populated (`subscription_name` / `binary` vs
/// `key_configured`) and what "ready" means.
#[derive(Debug, Clone, Serialize)]
pub struct LlmBackendInfo {
    /// Stable id: `claude` | `codex` | `cursor` | `gemini-cli` |
    /// `openai` | `anthropic` | `gemini`.
    pub id: String,
    pub display_name: String,
    /// `subscription` | `api`.
    pub kind: String,
    /// 1-based rank in the user's priority order.
    pub rank: usize,
    /// False when the user switched this backend off.
    pub enabled: bool,
    /// `ready` | `signed_out` | `not_installed` | `no_key`.
    pub status: String,
    /// The one action that makes it usable, when it isn't.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remedy: Option<String>,

    // Subscription-only.
    /// Which plan pays for it ("Claude Pro or Max").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subscription_name: Option<String>,
    /// Executable Cori looks for on PATH.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub binary: Option<String>,

    // API-only.
    /// A key is stored (non-secret index; the value never reaches the UI).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key_configured: Option<bool>,
    /// An env var overrides the stored key at run time.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key_env_override: Option<bool>,

    /// The model used for each tier, and whether the user chose it.
    pub models: Vec<LlmBackendModel>,
    /// Model names worth suggesting in the picker. Not a closed set.
    pub model_suggestions: Vec<String>,
}

/// One tier's model for one backend.
#[derive(Debug, Clone, Serialize)]
pub struct LlmBackendModel {
    /// `fast` | `balanced` | `deep`.
    pub tier: String,
    /// The model that will actually be sent.
    pub model: String,
    /// The built-in value, shown as the placeholder / reset target.
    pub default_model: String,
    /// True when the user overrode the default.
    pub overridden: bool,
}

/// Everything the AI Providers page renders in one round trip.
#[derive(Debug, Clone, Serialize)]
pub struct LlmSettings {
    pub backends: Vec<LlmBackendInfo>,
    /// What a step declaring no model would run on right now — the
    /// page's "currently" line. `None` when nothing is ready.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active: Option<LlmResolutionInfo>,
    /// Why nothing is ready, when `active` is `None`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocked_reason: Option<String>,
    /// True on a shared worker, where personal subscriptions are gated
    /// off regardless of this list.
    pub subscriptions_gated_off: bool,
}

/// Which backend serves a given model preference, for tooltips and the
/// "currently" line.
#[derive(Debug, Clone, Serialize)]
pub struct LlmResolutionInfo {
    pub backend_id: String,
    pub display_name: String,
    /// `subscription` | `api`.
    pub kind: String,
    /// Which plan pays, for subscriptions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subscription_name: Option<String>,
    /// What the step asked for.
    pub requested: String,
    /// The model that will actually be sent.
    pub model: String,
    /// The requested model isn't served here; the tier was matched instead.
    pub degraded: bool,
    pub tier: String,
}

const TIERS: [ModelTier; 3] = [ModelTier::Fast, ModelTier::Balanced, ModelTier::Deep];

#[tauri::command(rename_all = "snake_case")]
pub async fn get_llm_settings() -> IpcResult<LlmSettings> {
    tokio::task::spawn_blocking(settings_blocking)
        .await
        .map_err(|e| IpcError::Internal(anyhow::anyhow!("llm settings join: {e}")))?
}

/// Re-probe every subscription, ignoring the TTL cache. The button the
/// user presses right after signing in to a CLI in their terminal.
#[tauri::command(rename_all = "snake_case")]
pub async fn refresh_llm_settings() -> IpcResult<LlmSettings> {
    tokio::task::spawn_blocking(|| {
        subscription::invalidate_checks();
        settings_blocking()
    })
    .await
    .map_err(|e| IpcError::Internal(anyhow::anyhow!("llm settings refresh join: {e}")))?
}

/// Persist a new priority order. Ids not named are appended in built-in
/// order by the policy layer, so a partial list is safe.
#[tauri::command(rename_all = "snake_case")]
pub async fn set_llm_priority(order: Vec<String>) -> IpcResult<LlmSettings> {
    tokio::task::spawn_blocking(move || {
        for id in &order {
            if Backend::find(id).is_none() {
                return Err(IpcError::BadRequest(format!("unknown LLM backend `{id}`")));
            }
        }
        let mut config = load_config()?;
        config
            .set_value(
                "llm.priority",
                toml::Value::Array(order.into_iter().map(toml::Value::String).collect()),
            )
            .map_err(|e| IpcError::Internal(anyhow::anyhow!("setting llm.priority: {e}")))?;
        save_config(config)?;
        settings_blocking()
    })
    .await
    .map_err(|e| IpcError::Internal(anyhow::anyhow!("llm priority write join: {e}")))?
}

/// Turn one backend on or off without changing its rank.
#[tauri::command(rename_all = "snake_case")]
pub async fn set_llm_backend_enabled(backend: String, enabled: bool) -> IpcResult<LlmSettings> {
    tokio::task::spawn_blocking(move || {
        if Backend::find(&backend).is_none() {
            return Err(IpcError::BadRequest(format!(
                "unknown LLM backend `{backend}`"
            )));
        }
        let mut disabled: Vec<String> = cori_run::resolve_llm_config().disabled;
        disabled.retain(|id| id != &backend);
        if !enabled {
            disabled.push(backend);
        }
        let mut config = load_config()?;
        config
            .set_value(
                "llm.disabled",
                toml::Value::Array(disabled.into_iter().map(toml::Value::String).collect()),
            )
            .map_err(|e| IpcError::Internal(anyhow::anyhow!("setting llm.disabled: {e}")))?;
        save_config(config)?;
        settings_blocking()
    })
    .await
    .map_err(|e| IpcError::Internal(anyhow::anyhow!("llm enable write join: {e}")))?
}

/// Choose the model one backend uses for one tier. An empty `model`
/// clears the override and restores the built-in default.
#[tauri::command(rename_all = "snake_case")]
pub async fn set_llm_backend_model(
    backend: String,
    tier: String,
    model: String,
) -> IpcResult<LlmSettings> {
    tokio::task::spawn_blocking(move || {
        if Backend::find(&backend).is_none() {
            return Err(IpcError::BadRequest(format!(
                "unknown LLM backend `{backend}`"
            )));
        }
        let tier = ModelTier::parse(&tier)
            .ok_or_else(|| IpcError::BadRequest(format!("unknown model tier `{tier}`")))?;

        // Rewrite the whole `[llm.models]` table: the config writer sets
        // keys, and clearing an override means removing one.
        let mut models = cori_run::resolve_llm_config().models;
        let entry = models.entry(backend).or_default();
        let model = model.trim().to_string();
        if model.is_empty() {
            entry.remove(tier.as_str());
        } else {
            entry.insert(tier.as_str().to_string(), model);
        }
        models.retain(|_, tiers| !tiers.is_empty());

        let mut table = toml::map::Map::new();
        for (backend_id, tiers) in models {
            let mut inner = toml::map::Map::new();
            for (tier_name, model) in tiers {
                inner.insert(tier_name, toml::Value::String(model));
            }
            table.insert(backend_id, toml::Value::Table(inner));
        }
        let mut config = load_config()?;
        config
            .set_value("llm.models", toml::Value::Table(table))
            .map_err(|e| IpcError::Internal(anyhow::anyhow!("setting llm.models: {e}")))?;
        save_config(config)?;
        settings_blocking()
    })
    .await
    .map_err(|e| IpcError::Internal(anyhow::anyhow!("llm model write join: {e}")))?
}

fn load_config() -> IpcResult<cori_run::config::Config> {
    cori_run::config::Config::load()
        .map_err(|e| IpcError::Internal(anyhow::anyhow!("reading config.toml: {e}")))
}

fn save_config(config: cori_run::config::Config) -> IpcResult<()> {
    config
        .save()
        .map_err(|e| IpcError::Internal(anyhow::anyhow!("writing config.toml: {e}")))
}

/// The policy as it applies to *this* machine — the Console always runs
/// as the signed-in person, so subscriptions are on the table here.
fn console_policy() -> LlmPolicy {
    LlmPolicy::for_deployment(&cori_run::resolve_llm_config(), Deployment::Local)
}

fn settings_blocking() -> IpcResult<LlmSettings> {
    let policy = console_policy();
    let credentials = cori_run::resolve_llm_credentials();
    let env = LlmCredentials::from_env();
    let store = open_store()?;

    // Ranked backends first, then anything disabled (which the policy
    // filtered out) so the page still lists every option.
    let mut ordered: Vec<Backend> = policy.order().to_vec();
    for backend in Backend::all() {
        if !ordered.iter().any(|b| b.id() == backend.id()) {
            ordered.push(backend);
        }
    }
    let enabled_ids: Vec<&str> = policy.order().iter().map(|b| b.id()).collect();

    let backends = ordered
        .iter()
        .enumerate()
        .map(|(index, backend)| {
            backend_info(
                backend,
                index + 1,
                enabled_ids.contains(&backend.id()),
                &policy,
                &credentials,
                &env,
                &store,
            )
        })
        .collect();

    // What a step that declares nothing would run on right now.
    let request = catalog::parse_request(None);
    let (active, blocked_reason) = match resolve::preview(&request, &policy, &credentials) {
        Ok(selection) => (Some(resolution_info(&selection)), None),
        Err(error) => (None, Some(error.to_string())),
    };

    Ok(LlmSettings {
        backends,
        active,
        blocked_reason,
        subscriptions_gated_off: policy.subscriptions_gated_off(),
    })
}

#[allow(clippy::too_many_arguments)]
fn backend_info(
    backend: &Backend,
    rank: usize,
    enabled: bool,
    policy: &LlmPolicy,
    credentials: &LlmCredentials,
    env: &LlmCredentials,
    store: &cori_secrets::SecretStore,
) -> LlmBackendInfo {
    let models = TIERS
        .iter()
        .map(|tier| {
            let default_model = backend.default_model_for(*tier).unwrap_or_default();
            let override_model = policy.model_override(backend.id(), *tier);
            LlmBackendModel {
                tier: tier.as_str().to_string(),
                model: override_model
                    .clone()
                    .unwrap_or_else(|| default_model.to_string()),
                default_model: default_model.to_string(),
                overridden: override_model.is_some(),
            }
        })
        .collect();

    let mut info = LlmBackendInfo {
        id: backend.id().to_string(),
        display_name: backend.display_name().to_string(),
        kind: backend.kind().as_str().to_string(),
        rank,
        enabled,
        status: String::new(),
        remedy: None,
        subscription_name: None,
        binary: None,
        key_configured: None,
        key_env_override: None,
        models,
        model_suggestions: backend
            .model_suggestions()
            .iter()
            .map(|m| (*m).to_string())
            .collect(),
    };

    match backend {
        Backend::Subscription(spec) => {
            info.subscription_name = Some(spec.subscription_name.to_string());
            info.binary = Some(spec.binary.to_string());
            let (status, remedy) = match subscription::check(spec) {
                BackendState::Ready => ("ready", None),
                BackendState::SignedOut { hint } => ("signed_out", Some(hint)),
                BackendState::NotInstalled => (
                    "not_installed",
                    Some(format!("install `{}`, then sign in", spec.binary)),
                ),
            };
            info.status = status.to_string();
            info.remedy = remedy;
        }
        Backend::Api(id) => {
            let configured = credentials.key_for_str(id).is_some();
            info.key_configured = Some(store.is_configured(&cori_secrets::llm_account(id)));
            info.key_env_override = Some(env.key_for_str(id).is_some());
            info.status = if configured { "ready" } else { "no_key" }.to_string();
            if !configured {
                info.remedy = Some(format!("add an API key, or run `cori login {id}`"));
            }
        }
    }
    info
}

fn resolution_info(selection: &resolve::Selection) -> LlmResolutionInfo {
    LlmResolutionInfo {
        backend_id: selection.backend_id().to_string(),
        display_name: selection.backend.display_name().to_string(),
        kind: selection.kind().as_str().to_string(),
        subscription_name: match selection.backend {
            Backend::Subscription(spec) => Some(spec.subscription_name.to_string()),
            Backend::Api(_) => None,
        },
        requested: selection.requested.clone(),
        model: selection
            .resolved_model
            .clone()
            .unwrap_or_else(|| "(backend default)".to_string()),
        degraded: selection.degraded,
        tier: selection.tier.as_str().to_string(),
    }
}

/// Resolved policy + credentials, so a workflow with many `llm` steps
/// answers "which backend serves this?" once per *workflow* rather than
/// once per step. Reading credentials touches the OS keychain, which is
/// slow and can prompt — doing that per step would be felt.
pub struct PreviewContext {
    policy: LlmPolicy,
    credentials: LlmCredentials,
}

impl PreviewContext {
    pub fn new() -> Self {
        Self {
            policy: console_policy(),
            credentials: cori_run::resolve_llm_credentials(),
        }
    }

    /// Which backend would serve a step declaring `model` (or nothing).
    /// Goes through the same [`resolve::preview`] the runtime uses, so
    /// the tooltip cannot promise a backend the run won't use.
    pub fn for_model(&self, model: Option<&str>) -> Option<LlmResolutionInfo> {
        let request = catalog::parse_request(model);
        resolve::preview(&request, &self.policy, &self.credentials)
            .ok()
            .map(|selection| resolution_info(&selection))
    }
}

impl Default for PreviewContext {
    fn default() -> Self {
        Self::new()
    }
}

#[tauri::command(rename_all = "snake_case")]
pub async fn preview_llm_resolution(model: Option<String>) -> IpcResult<Option<LlmResolutionInfo>> {
    tokio::task::spawn_blocking(move || PreviewContext::new().for_model(model.as_deref()))
        .await
        .map_err(|e| IpcError::Internal(anyhow::anyhow!("llm preview join: {e}")))
}

/// Probe the provider's models endpoint with the pasted key. Rejects on
/// an explicit auth failure; accepts when the network is unreachable.
fn verify_key(provider: &str, api_key: &str) -> IpcResult<()> {
    let client = match reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
    {
        Ok(c) => c,
        Err(_) => return Ok(()),
    };
    let response = match provider {
        "openai" => client
            .get("https://api.openai.com/v1/models")
            .bearer_auth(api_key)
            .send(),
        "anthropic" => client
            .get("https://api.anthropic.com/v1/models")
            .header("x-api-key", api_key)
            .header("anthropic-version", "2023-06-01")
            .send(),
        "gemini" => client
            .get("https://generativelanguage.googleapis.com/v1beta/models")
            .header("x-goog-api-key", api_key)
            .send(),
        _ => return Ok(()),
    };
    match response {
        Ok(r) if r.status() == 401 || r.status() == 403 => Err(IpcError::BadRequest(format!(
            "the {provider} API rejected this key (HTTP {}) — check that it was pasted completely",
            r.status().as_u16()
        ))),
        // Other statuses (200, 429, 5xx) or transport errors: the key is
        // plausibly fine; never block saving on provider hiccups.
        _ => Ok(()),
    }
}
