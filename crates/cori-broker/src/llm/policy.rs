//! Which backend pays for an `llm` step, and who gets to decide.
//!
//! Cori can serve an `llm` step two ways: a metered vendor API key, or a
//! subscription the user already pays for, spent through a local agent
//! CLI ([`super::subscription`]). Both are [`Backend`]s, and the user's
//! whole preference is **one ordered list of them**:
//!
//! ```toml
//! [llm]
//! priority = ["claude", "openai", "codex", "cursor", "gemini-cli"]
//! disabled = ["gemini"]
//!
//! [llm.models.openai]
//! balanced = "gpt-4.1"
//! ```
//!
//! The first entry that is actually usable right now serves the step.
//! One list expresses everything a coarse mode could ("subscriptions
//! first" = put them on top; "never use Gemini" = disable it) and much
//! it could not ("my Claude subscription, then my OpenAI key, then my
//! Codex subscription"). The older `mode` key is still read, but only to
//! seed a default order for configs written before `priority` existed.
//!
//! # The deployment gate
//!
//! Subscriptions are a property of *a person's machine*. The agent CLIs
//! hold one user's OAuth session, their rate limits are per-account, and
//! their terms cover that account holder's own use. So the subscription
//! path is available exactly when Cori is running as that person:
//!
//! - [`Deployment::Local`] — `cori run` on a laptop, or the Cori Console
//!   desktop app. Identity is [`WorkerIdentity::Person`]. The configured
//!   priority is honoured as written.
//! - [`Deployment::Shared`] — `cori work --shared <pool>`, a service
//!   worker on org infrastructure. Identity is
//!   [`WorkerIdentity::Service`]. Subscription entries are **dropped
//!   from the order**, whatever the config file says. A shared worker
//!   running steps on behalf of many users must not spend one person's
//!   personal subscription, and its host has no interactive session to
//!   re-auth with.
//!
//! [`LlmPolicy::for_deployment`] applies that gate, so a `Service`
//! worker cannot be talked into the subscription path by a config file
//! copied from a laptop.
//!
//! [`WorkerIdentity::Person`]: cori_protocol::WorkerIdentity::Person
//! [`WorkerIdentity::Service`]: cori_protocol::WorkerIdentity::Service

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::catalog::{self, ModelTier};
use super::subscription::{self, BackendSpec};

/// Where this Cori process is running, which decides whether personal
/// subscriptions are on the table at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Deployment {
    /// A person's own machine — CLI or Console.
    Local,
    /// A shared/service worker. API keys only.
    Shared,
}

impl Deployment {
    /// Derive the deployment from the worker's identity. This is the
    /// only supported way to construct it: the gate must follow the same
    /// identity the planner routes on, not a separate flag someone can
    /// set independently.
    pub fn from_identity(identity: &cori_protocol::WorkerIdentity) -> Self {
        match identity {
            cori_protocol::WorkerIdentity::Person { .. } => Deployment::Local,
            cori_protocol::WorkerIdentity::Service { .. } => Deployment::Shared,
        }
    }
}

/// Which route a backend takes to a model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendKind {
    /// A local agent CLI spending the user's subscription.
    Subscription,
    /// A metered vendor HTTP API.
    Api,
}

impl BackendKind {
    pub fn as_str(self) -> &'static str {
        match self {
            BackendKind::Subscription => "subscription",
            BackendKind::Api => "api",
        }
    }
}

/// One place an `llm` step can be served from. The unit the user orders.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Subscription(&'static BackendSpec),
    /// An HTTP API provider id (`openai` / `anthropic` / `gemini`).
    Api(&'static str),
}

impl Backend {
    /// Stable id used in config, the trace, and the Console.
    pub fn id(&self) -> &'static str {
        match self {
            Backend::Subscription(spec) => spec.id,
            Backend::Api(id) => id,
        }
    }

    pub fn kind(&self) -> BackendKind {
        match self {
            Backend::Subscription(_) => BackendKind::Subscription,
            Backend::Api(_) => BackendKind::Api,
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            Backend::Subscription(spec) => spec.display_name,
            Backend::Api(id) => catalog::api_display_name(id),
        }
    }

    /// Does this backend natively serve `provider`'s models? Used so an
    /// exact `claude-*` model prefers Claude Code or the Anthropic API
    /// over a backend that would only approximate it.
    pub fn serves_api_provider(&self, provider: &str) -> bool {
        match self {
            Backend::Subscription(spec) => spec.serves_api_provider(provider),
            Backend::Api(id) => *id == provider,
        }
    }

    /// Model names worth suggesting for this backend, for the Console's
    /// pickers. Not a closed set — any string is accepted.
    pub fn model_suggestions(&self) -> &'static [&'static str] {
        match self {
            Backend::Subscription(spec) => spec.model_suggestions,
            Backend::Api(id) => catalog::api_model_suggestions(id),
        }
    }

    /// The built-in model for a tier, before user overrides.
    pub fn default_model_for(&self, tier: ModelTier) -> Option<&'static str> {
        match self {
            Backend::Subscription(spec) => spec.default_model_for(tier),
            Backend::Api(id) => catalog::api_model_for_tier(id, tier),
        }
    }

    /// Look up a backend by id across both routes.
    pub fn find(id: &str) -> Option<Backend> {
        if let Some(spec) = subscription::spec_for(id) {
            return Some(Backend::Subscription(spec));
        }
        catalog::API_PROVIDERS
            .iter()
            .find(|p| **p == id)
            .map(|p| Backend::Api(p))
    }

    /// Every backend Cori knows about, in built-in default order:
    /// subscriptions first (already paid for), then API providers.
    pub fn all() -> Vec<Backend> {
        subscription::BACKENDS
            .iter()
            .map(Backend::Subscription)
            .chain(catalog::API_PROVIDERS.iter().map(|p| Backend::Api(p)))
            .collect()
    }
}

/// Legacy coarse preference, superseded by [`LlmConfig::priority`].
///
/// Read only when no explicit priority list exists, to keep configs
/// written by earlier builds behaving the way they did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LlmMode {
    #[default]
    SubscriptionFirst,
    ApiFirst,
    ApiOnly,
    SubscriptionOnly,
}

impl LlmMode {
    pub fn as_str(self) -> &'static str {
        match self {
            LlmMode::SubscriptionFirst => "subscription_first",
            LlmMode::ApiFirst => "api_first",
            LlmMode::ApiOnly => "api_only",
            LlmMode::SubscriptionOnly => "subscription_only",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s
            .trim()
            .to_ascii_lowercase()
            .replace(['-', ' '], "_")
            .as_str()
        {
            "subscription_first" | "subscription" => Some(LlmMode::SubscriptionFirst),
            "api_first" | "api" => Some(LlmMode::ApiFirst),
            "api_only" => Some(LlmMode::ApiOnly),
            "subscription_only" => Some(LlmMode::SubscriptionOnly),
            _ => None,
        }
    }

    /// The priority list this mode is shorthand for.
    fn to_order(self) -> Vec<Backend> {
        let subs: Vec<Backend> = subscription::BACKENDS
            .iter()
            .map(Backend::Subscription)
            .collect();
        let apis: Vec<Backend> = catalog::API_PROVIDERS
            .iter()
            .map(|p| Backend::Api(p))
            .collect();
        match self {
            LlmMode::SubscriptionFirst => subs.into_iter().chain(apis).collect(),
            LlmMode::ApiFirst => apis.into_iter().chain(subs).collect(),
            LlmMode::ApiOnly => apis,
            LlmMode::SubscriptionOnly => subs,
        }
    }
}

/// The `[llm]` table of `~/.cori/config.toml`, as written by the Console.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LlmConfig {
    /// Ordered backend ids, best first. Absent means "the built-in
    /// order" (or whatever [`Self::mode`] implied, for old configs).
    #[serde(default)]
    pub priority: Option<Vec<String>>,
    /// Backends the user switched off. Never used, wherever they sit in
    /// `priority`.
    #[serde(default)]
    pub disabled: Vec<String>,
    /// `backend id → { tier → model }` overrides.
    #[serde(default)]
    pub models: BTreeMap<String, BTreeMap<String, String>>,
    /// Legacy coarse preference. Only consulted when `priority` is
    /// absent.
    #[serde(default)]
    pub mode: Option<String>,
    /// Legacy subscription-only ordering, folded into `priority`.
    #[serde(default)]
    pub order: Option<Vec<String>>,
}

/// Fully resolved policy handed to every `llm` step.
#[derive(Debug, Clone)]
pub struct LlmPolicy {
    /// Enabled backends, best first, after the deployment gate.
    order: Vec<Backend>,
    deployment: Deployment,
    /// True when the config asked for subscriptions but the deployment
    /// gate removed them — worth explaining rather than silently doing.
    subscriptions_gated_off: bool,
    model_overrides: BTreeMap<String, BTreeMap<String, String>>,
}

impl Default for LlmPolicy {
    fn default() -> Self {
        Self {
            order: Backend::all(),
            deployment: Deployment::Local,
            subscriptions_gated_off: false,
            model_overrides: BTreeMap::new(),
        }
    }
}

impl LlmPolicy {
    /// Build a policy from config, then apply the deployment gate.
    pub fn for_deployment(config: &LlmConfig, deployment: Deployment) -> Self {
        let configured = configured_order(config);

        let disabled: Vec<&str> = config.disabled.iter().map(String::as_str).collect();
        let mut wanted_subscription = false;
        let order: Vec<Backend> = configured
            .into_iter()
            .filter(|backend| !disabled.contains(&backend.id()))
            .inspect(|backend| {
                if backend.kind() == BackendKind::Subscription {
                    wanted_subscription = true;
                }
            })
            // The gate: a shared worker never spends a personal plan.
            .filter(|backend| deployment == Deployment::Local || backend.kind() == BackendKind::Api)
            .collect();

        Self {
            order,
            deployment,
            subscriptions_gated_off: deployment == Deployment::Shared && wanted_subscription,
            model_overrides: config.models.clone(),
        }
    }

    /// Enabled backends, best first. The whole preference.
    pub fn order(&self) -> &[Backend] {
        &self.order
    }

    pub fn deployment(&self) -> Deployment {
        self.deployment
    }

    /// The config wanted subscriptions but this is a shared worker.
    pub fn subscriptions_gated_off(&self) -> bool {
        self.subscriptions_gated_off
    }

    /// Subscription backends in the order, for capability discovery.
    pub fn subscription_order(&self) -> Vec<&'static BackendSpec> {
        self.order
            .iter()
            .filter_map(|backend| match backend {
                Backend::Subscription(spec) => Some(*spec),
                Backend::Api(_) => None,
            })
            .collect()
    }

    /// The model to send to a backend for a tier: the user's override if
    /// they set one, else the built-in default.
    pub fn model_for(&self, backend: &Backend, tier: ModelTier) -> Option<String> {
        if let Some(over) = self.model_override(backend.id(), tier) {
            return Some(over);
        }
        backend.default_model_for(tier).map(str::to_string)
    }

    /// The user's explicit override for one cell, if any.
    pub fn model_override(&self, backend_id: &str, tier: ModelTier) -> Option<String> {
        self.model_overrides
            .get(backend_id)
            .and_then(|m| m.get(tier.as_str()))
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    }
}

/// The order the config asks for, before disable/gate filtering.
///
/// Any backend the user didn't mention is appended in built-in order, so
/// a partial `priority` list is a "these first" statement rather than an
/// accidental allowlist — adding a new backend to Cori must not require
/// every existing config to be rewritten.
fn configured_order(config: &LlmConfig) -> Vec<Backend> {
    let named: Vec<Backend> = match (&config.priority, &config.order) {
        (Some(ids), _) => ids.iter().filter_map(|id| Backend::find(id)).collect(),
        // Legacy: `order` listed subscriptions only.
        (None, Some(ids)) => ids.iter().filter_map(|id| Backend::find(id)).collect(),
        (None, None) => Vec::new(),
    };

    let fallback = match (&config.priority, &config.order) {
        // No explicit list at all — the legacy mode decides, defaulting
        // to subscriptions first.
        (None, None) => config
            .mode
            .as_deref()
            .and_then(LlmMode::parse)
            .unwrap_or_default()
            .to_order(),
        _ => Backend::all(),
    };

    let mut out = named;
    for backend in fallback {
        if !out.iter().any(|b| b.id() == backend.id()) {
            out.push(backend);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use cori_protocol::WorkerIdentity;

    fn ids(policy: &LlmPolicy) -> Vec<&str> {
        policy.order().iter().map(|b| b.id()).collect()
    }

    fn local(config: LlmConfig) -> LlmPolicy {
        LlmPolicy::for_deployment(&config, Deployment::Local)
    }

    #[test]
    fn default_puts_subscriptions_before_api_keys() {
        let policy = local(LlmConfig::default());
        let order = ids(&policy);
        let first_api = order.iter().position(|id| *id == "openai").unwrap();
        let last_sub = order.iter().position(|id| *id == "gemini-cli").unwrap();
        assert!(last_sub < first_api, "{order:?}");
    }

    #[test]
    fn priority_is_honoured_exactly() {
        let policy = local(LlmConfig {
            priority: Some(vec!["openai".into(), "claude".into()]),
            ..Default::default()
        });
        let order = ids(&policy);
        assert_eq!(&order[..2], &["openai", "claude"]);
    }

    #[test]
    fn unnamed_backends_are_appended_not_dropped() {
        // A partial list means "these first", so adding a backend to
        // Cori doesn't silently disable it for existing users.
        let policy = local(LlmConfig {
            priority: Some(vec!["openai".into()]),
            ..Default::default()
        });
        let order = ids(&policy);
        assert_eq!(order[0], "openai");
        assert!(order.contains(&"claude"), "{order:?}");
        assert_eq!(order.len(), Backend::all().len());
    }

    #[test]
    fn disabled_backends_never_appear() {
        let policy = local(LlmConfig {
            disabled: vec!["cursor".into(), "gemini".into()],
            ..Default::default()
        });
        let order = ids(&policy);
        assert!(!order.contains(&"cursor"));
        assert!(!order.contains(&"gemini"));
        assert!(order.contains(&"openai"));
    }

    #[test]
    fn unknown_ids_in_config_are_ignored() {
        let policy = local(LlmConfig {
            priority: Some(vec!["not-a-backend".into(), "codex".into()]),
            ..Default::default()
        });
        assert_eq!(ids(&policy)[0], "codex");
    }

    #[test]
    fn shared_deployment_drops_every_subscription() {
        // The gate this module exists for: a config copied from a laptop
        // must not make a service worker spend one person's plan.
        let policy = LlmPolicy::for_deployment(
            &LlmConfig {
                priority: Some(vec!["claude".into(), "openai".into()]),
                ..Default::default()
            },
            Deployment::Shared,
        );
        assert_eq!(ids(&policy), vec!["openai", "anthropic", "gemini"]);
        assert!(policy.subscriptions_gated_off());
        assert!(policy.subscription_order().is_empty());
    }

    #[test]
    fn shared_deployment_without_subscriptions_configured_is_not_flagged() {
        let policy = LlmPolicy::for_deployment(
            &LlmConfig {
                priority: Some(vec!["openai".into()]),
                disabled: subscription::BACKENDS.iter().map(|b| b.id.into()).collect(),
                ..Default::default()
            },
            Deployment::Shared,
        );
        assert!(!policy.subscriptions_gated_off());
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
    fn legacy_mode_still_seeds_the_order() {
        let policy = local(LlmConfig {
            mode: Some("api_first".into()),
            ..Default::default()
        });
        assert_eq!(ids(&policy)[0], "openai");

        let policy = local(LlmConfig {
            mode: Some("api_only".into()),
            ..Default::default()
        });
        // `api_only` listed no subscriptions, and nothing re-adds them.
        assert_eq!(ids(&policy), vec!["openai", "anthropic", "gemini"]);
    }

    #[test]
    fn explicit_priority_beats_legacy_mode() {
        let policy = local(LlmConfig {
            mode: Some("api_only".into()),
            priority: Some(vec!["claude".into()]),
            ..Default::default()
        });
        assert_eq!(ids(&policy)[0], "claude");
    }

    #[test]
    fn model_overrides_beat_defaults() {
        let mut models = BTreeMap::new();
        models.insert(
            "codex".to_string(),
            BTreeMap::from([("deep".to_string(), "gpt-5-pro".to_string())]),
        );
        let policy = local(LlmConfig {
            models,
            ..Default::default()
        });
        let codex = Backend::find("codex").unwrap();
        assert_eq!(
            policy.model_for(&codex, ModelTier::Deep).as_deref(),
            Some("gpt-5-pro")
        );
        assert_eq!(
            policy.model_for(&codex, ModelTier::Fast).as_deref(),
            codex.default_model_for(ModelTier::Fast)
        );
        assert!(policy.model_override("codex", ModelTier::Fast).is_none());
    }

    #[test]
    fn every_backend_has_a_display_name_and_models() {
        for backend in Backend::all() {
            assert!(!backend.display_name().is_empty(), "{}", backend.id());
            assert!(!backend.model_suggestions().is_empty(), "{}", backend.id());
            for tier in [ModelTier::Fast, ModelTier::Balanced, ModelTier::Deep] {
                assert!(
                    backend.default_model_for(tier).is_some(),
                    "{} has no {tier} model",
                    backend.id()
                );
            }
        }
    }
}
