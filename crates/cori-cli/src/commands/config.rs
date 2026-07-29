//! `cori config get|set`.
//!
//! Config holds non-secret settings only. LLM API keys live in the
//! shared secret store (OS keychain / file fallback) via `cori login
//! <provider>`; `config set` refuses them, and `config get
//! llm.<provider>.api_key` reads through to the store so existing
//! scripts keep working.

use anyhow::Result;

use cori_run::config::Config;

pub fn get(key: Option<&str>) -> Result<()> {
    match key {
        Some(k) => {
            if let Some(provider) = llm_api_key_provider(k) {
                let store = cori_secrets::SecretStore::open_default()?;
                match store.get(&cori_secrets::llm_account(provider))? {
                    Some(v) => println!("{v}"),
                    None => {
                        eprintln!("no {provider} API key is stored — run `cori login {provider}`");
                        std::process::exit(1);
                    }
                }
                return Ok(());
            }
            let cfg = Config::load()?;
            match cfg.get(k) {
                Some(v) => {
                    println!("{}", render_value(v));
                }
                None => {
                    eprintln!("no config key `{k}`");
                    std::process::exit(1);
                }
            }
        }
        None => {
            let cfg = Config::load()?;
            for (k, v) in cfg.flatten() {
                println!("{k} = {}", render_value(&v));
            }
            // Secrets are listed by presence only, never by value.
            let store = cori_secrets::SecretStore::open_default()?;
            let where_stored = if store.uses_keychain() {
                "keychain"
            } else {
                "credentials file"
            };
            for provider in store.configured_llm_providers() {
                println!("llm.{provider}.api_key = <stored in {where_stored}>");
            }
        }
    }
    Ok(())
}

pub fn set(key: &str, value: &str) -> Result<()> {
    if let Some(provider) = llm_api_key_provider(key) {
        anyhow::bail!(
            "secrets don't live in config — run `cori login {provider}` to store the API key in the OS keychain"
        );
    }
    let mut cfg = Config::load()?;
    cfg.set(key, value)?;
    cfg.save()?;
    println!("✓ {key} = {value}");
    Ok(())
}

/// `llm.<provider>.api_key` → `Some(provider)` for known providers.
fn llm_api_key_provider(key: &str) -> Option<&'static str> {
    let rest = key.strip_prefix("llm.")?;
    let provider = rest.strip_suffix(".api_key")?;
    cori_secrets::LLM_PROVIDERS
        .iter()
        .find(|p| **p == provider)
        .copied()
}

fn render_value(v: &toml::Value) -> String {
    match v {
        toml::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}
