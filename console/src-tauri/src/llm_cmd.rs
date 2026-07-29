//! LLM provider key management — the Console's `cori login <provider>`.
//!
//! Keys go to the shared secret store (`cori-secrets`: OS keychain,
//! file fallback), the same entries the CLI reads at run time, so a key
//! saved here is immediately usable by `cori run` / `cori mcp` and vice
//! versa. Values never travel back to the frontend — the UI only ever
//! sees booleans derived from the store's non-secret index.
//!
//! `set_llm_provider_key` verifies the key against the provider's
//! models endpoint before storing: an explicit 401/403 rejects with a
//! clear message, while transport failures (offline) store anyway —
//! never lock the user out of saving a key because the network is down.

use cori_broker::llm::LlmCredentials;
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
