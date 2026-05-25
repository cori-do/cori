//! LLM provider credential resolution.
//!
//! The CLI loads `~/.cori/config.toml` and passes any `llm.<provider>.api_key`
//! values to the broker via [`LlmCredentials`]. Env vars
//! (`OPENAI_API_KEY` etc.) take precedence over config values so users can
//! override per-shell without rewriting config.
//!
//! If neither source supplies a key and the run is interactive (stdin is
//! a TTY), [`prompt_for_missing`] writes to stderr asking the user to set
//! the env var or run `cori config set`, then waits for Enter and
//! re-reads the env. Non-interactive runs surface
//! [`BrokerError::LlmMissingCredentials`] immediately.

use std::io::{self, BufRead, IsTerminal, Write};

use crate::{BrokerError, Result};

/// Resolution sources, in priority order: env > config-supplied.
#[derive(Debug, Clone, Default)]
pub struct LlmCredentials {
    pub openai_api_key: Option<String>,
    pub anthropic_api_key: Option<String>,
    pub gemini_api_key: Option<String>,
}

impl LlmCredentials {
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
        match provider {
            "openai" => self.openai_api_key.as_deref(),
            "anthropic" => self.anthropic_api_key.as_deref(),
            "gemini" => self.gemini_api_key.as_deref(),
            _ => None,
        }
    }
}

fn env_nonempty(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.trim().is_empty())
}

/// If the credential for `provider` is missing, either prompt the user
/// (interactive) or return an error (non-interactive). On success, returns
/// the resolved key.
pub fn require(creds: &LlmCredentials, provider: &'static str) -> Result<String> {
    if let Some(k) = creds.key_for(provider) {
        return Ok(k.to_string());
    }
    let env_var = match provider {
        "openai" => "OPENAI_API_KEY",
        "anthropic" => "ANTHROPIC_API_KEY",
        "gemini" => "GEMINI_API_KEY",
        _ => "",
    };

    if !io::stdin().is_terminal() {
        return Err(BrokerError::LlmMissingCredentials { provider, env_var });
    }

    let mut stderr = io::stderr();
    let _ = writeln!(
        stderr,
        "\nThis step requires an {provider} API key.\n  · Set {env_var}=<your-key> in your environment, OR\n  · Run `cori config set llm.{provider}.api_key <your-key>`\nPress Enter once the key is set (or Ctrl-C to abort)…",
    );
    let _ = stderr.flush();
    let mut line = String::new();
    let _ = io::stdin().lock().read_line(&mut line);

    // Re-read just this var from the env — handles the case where the
    // user exported it in another shell or wrote it to config in
    // another terminal.
    if let Some(k) = env_nonempty(env_var) {
        return Ok(k);
    }
    Err(BrokerError::LlmMissingCredentials { provider, env_var })
}
