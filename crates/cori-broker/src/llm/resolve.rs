//! Resolve one workflow level through the machine's single active backend.

use super::catalog::LlmLevel;
use super::credentials::LlmCredentials;
use super::policy::{Backend, BackendKind, LlmPolicy};
use super::providers::{self, LlmProvider};
use super::subscription::{self, SubscriptionProvider};
use crate::{BrokerError, Result};

#[derive(Debug, Clone)]
pub struct Selection {
    pub backend: Backend,
    pub level: LlmLevel,
    pub resolved_model: String,
}

impl Selection {
    pub fn backend_id(&self) -> &'static str {
        self.backend.id()
    }

    pub fn kind(&self) -> BackendKind {
        self.backend.kind()
    }

    pub fn trace_note(&self) -> String {
        format!(
            "llm: level `{}` → `{}` via {} ({})",
            self.level,
            self.resolved_model,
            self.backend_id(),
            self.kind().as_str()
        )
    }

    pub fn cost_model(&self) -> Option<&str> {
        match self.kind() {
            BackendKind::Subscription => None,
            BackendKind::Api => Some(&self.resolved_model),
        }
    }
}

pub struct Resolution {
    pub provider: Box<dyn LlmProvider>,
    pub selection: Selection,
}

impl std::fmt::Debug for Resolution {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Resolution")
            .field("selection", &self.selection)
            .finish()
    }
}

impl Resolution {
    pub fn trace_note(&self) -> String {
        self.selection.trace_note()
    }

    pub fn cost_model(&self) -> Option<&str> {
        self.selection.cost_model()
    }
}

pub fn select(
    level: LlmLevel,
    policy: &LlmPolicy,
    credentials: &LlmCredentials,
) -> Result<Selection> {
    let backend = active_backend(policy, credentials)?;
    let resolved_model = policy.model_for(&backend, level).ok_or_else(|| {
        BrokerError::LlmNoBackend {
            requested: level.to_string(),
            detail: format!(
                "{} has no model configured for `{level}` work. Set one in Settings → AI Providers → Advanced models.",
                backend.display_name()
            ),
        }
    })?;
    Ok(Selection {
        backend,
        level,
        resolved_model,
    })
}

pub fn resolve(
    level: LlmLevel,
    policy: &LlmPolicy,
    credentials: &LlmCredentials,
) -> Result<Resolution> {
    let selection = select(level, policy, credentials)?;
    let provider = build_provider(&selection, credentials);
    Ok(Resolution {
        provider,
        selection,
    })
}

pub fn preview(
    level: LlmLevel,
    policy: &LlmPolicy,
    credentials: &LlmCredentials,
) -> Result<Selection> {
    select(level, policy, credentials)
}

fn active_backend(policy: &LlmPolicy, credentials: &LlmCredentials) -> Result<Backend> {
    if policy.subscriptions_gated_off() {
        return Err(BrokerError::LlmNoBackend {
            requested: "active provider".into(),
            detail: "The active AI provider is a personal subscription, which a shared worker cannot spend. Select an API-key provider on this worker.".into(),
        });
    }

    let Some(backend) = policy.active() else {
        let detail = match policy.configured_active() {
            Some(id) => format!(
                "The configured AI provider `{id}` is not known to this Cori build. Choose another provider in Settings → AI Providers."
            ),
            None => "No AI provider is active. Choose one in Settings → AI Providers or set `llm.active` in ~/.cori/config.toml.".into(),
        };
        return Err(BrokerError::LlmNoBackend {
            requested: "active provider".into(),
            detail,
        });
    };

    match backend {
        Backend::Subscription(spec) => match subscription::check(spec) {
            subscription::BackendState::Ready => Ok(backend),
            subscription::BackendState::SignedOut { hint } => Err(BrokerError::LlmNoBackend {
                requested: backend.id().into(),
                detail: format!(
                    "{} is active but signed out — {hint}. Cori will not switch providers automatically.",
                    backend.display_name()
                ),
            }),
            subscription::BackendState::NotInstalled => Err(BrokerError::LlmNoBackend {
                requested: backend.id().into(),
                detail: format!(
                    "{} is active but `{}` is not installed. Install it, sign in, then re-check AI Providers. Cori will not switch providers automatically.",
                    backend.display_name(),
                    spec.binary
                ),
            }),
        },
        Backend::Api(id) if credentials.key_for_str(id).is_some() => Ok(backend),
        Backend::Api(id) => Err(BrokerError::LlmNoBackend {
            requested: id.into(),
            detail: format!(
                "{} is active but has no API key. Add its key in Settings → AI Providers or run `cori login {id}`. Cori will not switch providers automatically.",
                backend.display_name()
            ),
        }),
    }
}

fn build_provider(selection: &Selection, credentials: &LlmCredentials) -> Box<dyn LlmProvider> {
    match selection.backend {
        Backend::Subscription(spec) => Box::new(SubscriptionProvider::new(
            spec,
            Some(selection.resolved_model.clone()),
        )),
        Backend::Api(id) => {
            let key = credentials.key_for_str(id).unwrap_or_default().to_string();
            match id {
                "openai" => Box::new(providers::OpenAiProvider::new(key)),
                "anthropic" => Box::new(providers::AnthropicProvider::new(key)),
                _ => Box::new(providers::GeminiProvider::new(key)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::super::policy::{Deployment, LlmConfig};
    use super::*;

    fn credentials_with(provider: &str) -> LlmCredentials {
        let mut credentials = LlmCredentials::empty();
        match provider {
            "openai" => credentials.openai_api_key = Some("sk-test".into()),
            "anthropic" => credentials.anthropic_api_key = Some("sk-ant-test".into()),
            "gemini" => credentials.gemini_api_key = Some("AIza-test".into()),
            _ => {}
        }
        credentials
    }

    fn api_policy(active: Option<&str>) -> LlmPolicy {
        LlmPolicy::for_deployment(
            &LlmConfig {
                active: active.map(str::to_string),
                ..Default::default()
            },
            Deployment::Local,
        )
    }

    #[test]
    fn active_api_uses_its_level_model() {
        let selection = select(
            LlmLevel::Low,
            &api_policy(Some("openai")),
            &credentials_with("openai"),
        )
        .expect("resolves");
        assert_eq!(selection.backend_id(), "openai");
        assert_eq!(selection.resolved_model, "gpt-4o-mini");
    }

    #[test]
    fn no_active_backend_fails_clearly() {
        let error = select(
            LlmLevel::Medium,
            &api_policy(None),
            &credentials_with("openai"),
        )
        .expect_err("must select a backend");
        assert!(error.to_string().contains("No AI provider is active"));
    }

    #[test]
    fn another_ready_backend_is_never_a_fallback() {
        let mut credentials = credentials_with("openai");
        credentials.anthropic_api_key = Some("sk-ant-test".into());
        let error = select(LlmLevel::Medium, &api_policy(Some("gemini")), &credentials)
            .expect_err("active Gemini has no key");
        let message = error.to_string();
        assert!(message.contains("Google Gemini is active"), "{message}");
        assert!(message.contains("will not switch"), "{message}");
    }

    #[test]
    fn configured_model_override_is_sent() {
        let policy = LlmPolicy::for_deployment(
            &LlmConfig {
                active: Some("openai".into()),
                models: BTreeMap::from([(
                    "openai".into(),
                    BTreeMap::from([("high".into(), "gpt-custom".into())]),
                )]),
            },
            Deployment::Local,
        );
        let selection =
            select(LlmLevel::High, &policy, &credentials_with("openai")).expect("resolves");
        assert_eq!(selection.resolved_model, "gpt-custom");
    }

    #[test]
    fn trace_names_level_model_backend_and_route() {
        let selection = select(
            LlmLevel::Medium,
            &api_policy(Some("openai")),
            &credentials_with("openai"),
        )
        .expect("resolves");
        assert_eq!(
            selection.trace_note(),
            "llm: level `medium` → `gpt-4o` via openai (api)"
        );
    }

    #[test]
    fn preview_matches_resolution() {
        let policy = api_policy(Some("openai"));
        let credentials = credentials_with("openai");
        let previewed = preview(LlmLevel::High, &policy, &credentials).expect("preview");
        let resolved = resolve(LlmLevel::High, &policy, &credentials).expect("resolution");
        assert_eq!(previewed.backend_id(), resolved.selection.backend_id());
        assert_eq!(previewed.resolved_model, resolved.selection.resolved_model);
    }

    #[test]
    fn shared_worker_rejects_active_subscription() {
        let policy = LlmPolicy::for_deployment(
            &LlmConfig {
                active: Some("codex".into()),
                ..Default::default()
            },
            Deployment::Shared,
        );
        let error = select(LlmLevel::Medium, &policy, &LlmCredentials::empty())
            .expect_err("subscription is gated");
        assert!(error.to_string().contains("shared worker"));
    }

    #[test]
    fn shared_worker_accepts_active_api_provider() {
        let policy = LlmPolicy::for_deployment(
            &LlmConfig {
                active: Some("openai".into()),
                ..Default::default()
            },
            Deployment::Shared,
        );
        let selection = select(LlmLevel::Medium, &policy, &credentials_with("openai"))
            .expect("shared workers may use active API keys");
        assert_eq!(selection.backend_id(), "openai");
    }

    #[test]
    fn subscription_calls_are_not_priced_as_api_calls() {
        let selection = Selection {
            backend: Backend::find("codex").unwrap(),
            level: LlmLevel::Medium,
            resolved_model: "gpt-5".into(),
        };
        assert_eq!(selection.cost_model(), None);
    }
}
