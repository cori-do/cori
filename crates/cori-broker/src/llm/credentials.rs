//! LLM provider credential resolution.
//!
//! The CLI reads API keys from the shared secret store (OS keychain,
//! file fallback — see `cori-secrets`) and passes them to the broker
//! via [`LlmCredentials`]. Env vars (`OPENAI_API_KEY` etc.) take
//! precedence over stored values so users can override per-shell.
//!
//! Missing credentials are always reported immediately. Provider setup is a
//! machine setting and never an interactive workflow preflight prompt.

use crate::BrokerError;

/// Resolution sources, in priority order: env > config-supplied.
#[derive(Debug, Clone, Default)]
pub struct LlmCredentials {
    pub openai_api_key: Option<String>,
    pub anthropic_api_key: Option<String>,
    pub gemini_api_key: Option<String>,
}

impl LlmCredentials {
    /// A credential set with nothing in it.
    pub const fn empty() -> Self {
        Self {
            openai_api_key: None,
            anthropic_api_key: None,
            gemini_api_key: None,
        }
    }

    /// Read credentials only from environment variables (no config). The
    /// CLI overlays config values on top via the public setters.
    pub fn from_env() -> Self {
        Self {
            openai_api_key: env_nonempty("OPENAI_API_KEY"),
            anthropic_api_key: env_nonempty("ANTHROPIC_API_KEY"),
            gemini_api_key: env_nonempty("GEMINI_API_KEY")
                .or_else(|| env_nonempty("GOOGLE_API_KEY")),
        }
    }

    /// Fill any unset slot from `other` (used by the CLI to layer
    /// config-derived values under the env-derived ones).
    pub fn or_fill_from(mut self, other: &LlmCredentials) -> Self {
        if self.openai_api_key.is_none() {
            self.openai_api_key = other.openai_api_key.clone();
        }
        if self.anthropic_api_key.is_none() {
            self.anthropic_api_key = other.anthropic_api_key.clone();
        }
        if self.gemini_api_key.is_none() {
            self.gemini_api_key = other.gemini_api_key.clone();
        }
        self
    }

    pub fn key_for(&self, provider: &'static str) -> Option<&str> {
        self.key_for_str(provider)
    }

    /// Same lookup, for a provider id whose lifetime isn't `'static`
    /// (config values, iteration over the provider table).
    pub fn key_for_str(&self, provider: &str) -> Option<&str> {
        match provider {
            "openai" => self.openai_api_key.as_deref(),
            "anthropic" => self.anthropic_api_key.as_deref(),
            "gemini" => self.gemini_api_key.as_deref(),
            _ => None,
        }
    }

    /// Providers with a usable key, in [`super::catalog::API_PROVIDERS`]
    /// order.
    pub fn configured_providers(&self) -> Vec<&'static str> {
        super::catalog::API_PROVIDERS
            .iter()
            .filter(|id| self.key_for_str(id).is_some())
            .copied()
            .collect()
    }
}

fn env_nonempty(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.trim().is_empty())
}

/// The env var carrying a provider's API key.
pub fn env_var_for(provider: &str) -> &'static str {
    match provider {
        "openai" => "OPENAI_API_KEY",
        "anthropic" => "ANTHROPIC_API_KEY",
        "gemini" => "GEMINI_API_KEY",
        _ => "",
    }
}

/// Read every provider key from the shared secret store (OS keychain,
/// file fallback), with env vars layered on top.
pub fn from_env_and_store() -> LlmCredentials {
    let mut stored = LlmCredentials::empty();
    if let Ok(store) = cori_secrets::SecretStore::open_default() {
        stored.openai_api_key = store
            .get(&cori_secrets::llm_account("openai"))
            .ok()
            .flatten();
        stored.anthropic_api_key = store
            .get(&cori_secrets::llm_account("anthropic"))
            .ok()
            .flatten();
        stored.gemini_api_key = store
            .get(&cori_secrets::llm_account("gemini"))
            .ok()
            .flatten();
    }
    LlmCredentials::from_env().or_fill_from(&stored)
}

/// Non-interactive callers still get the structured error.
pub fn missing_credentials(provider: &'static str) -> BrokerError {
    BrokerError::LlmMissingCredentials {
        provider,
        env_var: env_var_for(provider),
    }
}
