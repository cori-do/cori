//! Machine-owned LLM provider selection.
//!
//! Connections and credentials may exist for many backends, but exactly one
//! backend (or none) is active. There is no priority order and no fallback.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::catalog::{self, LlmLevel};
use super::subscription::{self, BackendSpec};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Deployment {
    Local,
    Shared,
}

impl Deployment {
    pub fn from_identity(identity: &cori_protocol::WorkerIdentity) -> Self {
        match identity {
            cori_protocol::WorkerIdentity::Person { .. } => Self::Local,
            cori_protocol::WorkerIdentity::Service { .. } => Self::Shared,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendKind {
    Subscription,
    Api,
}

impl BackendKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Subscription => "subscription",
            Self::Api => "api",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Subscription(&'static BackendSpec),
    Api(&'static str),
}

impl Backend {
    pub fn id(&self) -> &'static str {
        match self {
            Self::Subscription(spec) => spec.id,
            Self::Api(id) => id,
        }
    }

    pub fn kind(&self) -> BackendKind {
        match self {
            Self::Subscription(_) => BackendKind::Subscription,
            Self::Api(_) => BackendKind::Api,
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Subscription(spec) => spec.display_name,
            Self::Api(id) => catalog::api_display_name(id),
        }
    }

    pub fn model_suggestions(&self) -> &'static [&'static str] {
        match self {
            Self::Subscription(spec) => spec.model_suggestions,
            Self::Api(id) => catalog::api_model_suggestions(id),
        }
    }

    pub fn default_model_for(&self, level: LlmLevel) -> Option<&'static str> {
        match self {
            Self::Subscription(spec) => spec.default_model_for(level),
            Self::Api(id) => catalog::api_model_for_level(id, level),
        }
    }

    pub fn find(id: &str) -> Option<Self> {
        if let Some(spec) = subscription::spec_for(id) {
            return Some(Self::Subscription(spec));
        }
        catalog::API_PROVIDERS
            .iter()
            .find(|provider| **provider == id)
            .map(|provider| Self::Api(provider))
    }

    pub fn all() -> Vec<Self> {
        subscription::BACKENDS
            .iter()
            .map(Self::Subscription)
            .chain(
                catalog::API_PROVIDERS
                    .iter()
                    .map(|provider| Self::Api(provider)),
            )
            .collect()
    }
}

/// Non-secret `[llm]` configuration. Unknown legacy fields are ignored by
/// serde, so old priority lists never silently select a backend.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LlmConfig {
    #[serde(default)]
    pub active: Option<String>,
    /// backend id -> level -> model. Old fast/balanced/deep keys remain
    /// readable as fallbacks until the user edits that provider.
    #[serde(default)]
    pub models: BTreeMap<String, BTreeMap<String, String>>,
}

#[derive(Debug, Clone)]
pub struct LlmPolicy {
    active: Option<Backend>,
    configured_active: Option<String>,
    deployment: Deployment,
    subscriptions_gated_off: bool,
    model_overrides: BTreeMap<String, BTreeMap<String, String>>,
}

impl Default for LlmPolicy {
    fn default() -> Self {
        Self::for_deployment(&LlmConfig::default(), Deployment::Local)
    }
}

impl LlmPolicy {
    pub fn for_deployment(config: &LlmConfig, deployment: Deployment) -> Self {
        let configured_active = config
            .active
            .as_deref()
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(str::to_string);
        let selected = configured_active.as_deref().and_then(Backend::find);
        let subscriptions_gated_off =
            deployment == Deployment::Shared && matches!(selected, Some(Backend::Subscription(_)));
        let active = if subscriptions_gated_off {
            None
        } else {
            selected
        };
        Self {
            active,
            configured_active,
            deployment,
            subscriptions_gated_off,
            model_overrides: config.models.clone(),
        }
    }

    pub fn active(&self) -> Option<Backend> {
        self.active
    }

    pub fn configured_active(&self) -> Option<&str> {
        self.configured_active.as_deref()
    }

    pub fn deployment(&self) -> Deployment {
        self.deployment
    }

    pub fn subscriptions_gated_off(&self) -> bool {
        self.subscriptions_gated_off
    }

    pub fn subscription_order(&self) -> Vec<&'static BackendSpec> {
        match self.active {
            Some(Backend::Subscription(spec)) => vec![spec],
            _ => Vec::new(),
        }
    }

    pub fn model_for(&self, backend: &Backend, level: LlmLevel) -> Option<String> {
        self.model_override(backend.id(), level)
            .or_else(|| backend.default_model_for(level).map(str::to_string))
    }

    pub fn model_override(&self, backend_id: &str, level: LlmLevel) -> Option<String> {
        let levels = self.model_overrides.get(backend_id)?;
        let legacy = match level {
            LlmLevel::Low => "fast",
            LlmLevel::Medium => "balanced",
            LlmLevel::High => "deep",
        };
        levels
            .get(level.as_str())
            .or_else(|| levels.get(legacy))
            .map(|model| model.trim())
            .filter(|model| !model.is_empty())
            .map(str::to_string)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cori_protocol::WorkerIdentity;

    fn local(config: LlmConfig) -> LlmPolicy {
        LlmPolicy::for_deployment(&config, Deployment::Local)
    }

    #[test]
    fn no_active_backend_is_the_default() {
        assert!(local(LlmConfig::default()).active().is_none());
    }

    #[test]
    fn legacy_ranking_fields_do_not_select_a_backend() {
        let config: LlmConfig = toml::from_str(
            "priority = ['cursor', 'openai']\ndisabled = ['openai']\nmode = 'subscription'\norder = ['codex']\n",
        )
        .expect("legacy config remains readable");
        assert!(config.active.is_none());
        assert!(local(config).active().is_none());
    }

    #[test]
    fn configured_backend_is_the_only_active_backend() {
        let policy = local(LlmConfig {
            active: Some("cursor".into()),
            ..Default::default()
        });
        assert_eq!(policy.active().map(|backend| backend.id()), Some("cursor"));
        assert_eq!(policy.subscription_order()[0].id, "cursor");
    }

    #[test]
    fn unknown_active_backend_is_retained_for_diagnostics() {
        let policy = local(LlmConfig {
            active: Some("future-provider".into()),
            ..Default::default()
        });
        assert!(policy.active().is_none());
        assert_eq!(policy.configured_active(), Some("future-provider"));
    }

    #[test]
    fn shared_deployment_gates_an_active_subscription() {
        let policy = LlmPolicy::for_deployment(
            &LlmConfig {
                active: Some("claude".into()),
                ..Default::default()
            },
            Deployment::Shared,
        );
        assert!(policy.active().is_none());
        assert!(policy.subscriptions_gated_off());
    }

    #[test]
    fn shared_deployment_accepts_an_active_api() {
        let policy = LlmPolicy::for_deployment(
            &LlmConfig {
                active: Some("openai".into()),
                ..Default::default()
            },
            Deployment::Shared,
        );
        assert_eq!(policy.active().map(|backend| backend.id()), Some("openai"));
    }

    #[test]
    fn deployment_follows_worker_identity() {
        assert_eq!(
            Deployment::from_identity(&WorkerIdentity::Person {
                user_id: "adrien".into()
            }),
            Deployment::Local
        );
        assert_eq!(
            Deployment::from_identity(&WorkerIdentity::Service {
                pool: "notion-pool".into()
            }),
            Deployment::Shared
        );
    }

    #[test]
    fn new_model_override_beats_legacy_fallback() {
        let models = BTreeMap::from([(
            "codex".to_string(),
            BTreeMap::from([
                ("medium".to_string(), "new-medium".to_string()),
                ("balanced".to_string(), "legacy-medium".to_string()),
                ("deep".to_string(), "legacy-high".to_string()),
            ]),
        )]);
        let policy = local(LlmConfig {
            active: Some("codex".into()),
            models,
        });
        let codex = policy.active().unwrap();
        assert_eq!(
            policy.model_for(&codex, LlmLevel::Medium).as_deref(),
            Some("new-medium")
        );
        assert_eq!(
            policy.model_for(&codex, LlmLevel::High).as_deref(),
            Some("legacy-high")
        );
    }

    #[test]
    fn every_backend_has_models_for_every_level() {
        for backend in Backend::all() {
            assert!(!backend.display_name().is_empty());
            for level in LlmLevel::ALL {
                assert!(backend.default_model_for(level).is_some());
            }
        }
    }
}
