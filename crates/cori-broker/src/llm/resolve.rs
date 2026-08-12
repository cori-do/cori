//! Pick the backend that serves one `llm` step.
//!
//! Inputs: what the step asked for ([`ModelRequest`]), the user's
//! ordered preference ([`LlmPolicy`]), and what is actually usable on
//! this machine right now (signed-in subscriptions, API keys present).
//!
//! The rules, in order:
//!
//! 1. Walk the user's priority list and keep the backends that can run
//!    *right now* — subscriptions must be signed in, API providers must
//!    have a key.
//! 2. A step that asked for a **tier** takes the first survivor.
//! 3. A step that asked for an **exact model** prefers, among survivors,
//!    one that natively serves that model's vendor — the OpenAI API or
//!    Codex for `gpt-4o`, Claude Code or the Anthropic API for
//!    `claude-*`. That backend gets the exact name. Any other backend
//!    serves the model's *tier* instead, so `o3` degrades to another
//!    `deep` model rather than to whatever happened to be installed.
//! 4. Nothing usable → an error naming the ways to fix it.
//!
//! [`select`] is the single implementation of those rules. [`resolve`]
//! builds a live provider from it; [`preview`] only describes it, which
//! is what the Console's "this step will run on…" tooltip renders. They
//! cannot disagree about which backend wins, because there is one
//! decision function and they both call it.

use super::catalog::{self, ModelRequest, ModelTier};
use super::credentials::LlmCredentials;
use super::policy::{Backend, BackendKind, LlmPolicy};
use super::providers::{self, LlmProvider};
use super::subscription::{self, SubscriptionProvider};
use crate::{BrokerError, Result};

/// The decision: which backend, serving which model, and how that
/// relates to what the step asked for.
#[derive(Debug, Clone)]
pub struct Selection {
    pub backend: Backend,
    /// What the step declared (`"gpt-4o-mini"`, `"fast"`, or
    /// `"balanced"` when it declared nothing).
    pub requested: String,
    /// The model actually sent. `None` means the backend's own
    /// configured default is used.
    pub resolved_model: Option<String>,
    /// The step named an exact model this backend does not serve; it was
    /// served at the same tier instead.
    pub degraded: bool,
    pub tier: ModelTier,
}

impl Selection {
    pub fn backend_id(&self) -> &'static str {
        self.backend.id()
    }

    pub fn kind(&self) -> BackendKind {
        self.backend.kind()
    }

    /// The line recorded in the run trace, and shown in the Console.
    /// Always states the backend; calls out a substitution explicitly
    /// when one happened.
    pub fn trace_note(&self) -> String {
        let served = self
            .resolved_model
            .as_deref()
            .unwrap_or("(backend default)");
        if self.degraded {
            format!(
                "llm: requested `{}` → served `{}` at tier `{}` by {} ({})",
                self.requested,
                served,
                self.tier,
                self.backend_id(),
                self.kind().as_str()
            )
        } else {
            format!(
                "llm: `{}` served by {} ({})",
                served,
                self.backend_id(),
                self.kind().as_str()
            )
        }
    }

    /// Model name for cost lookup. Subscription calls are already paid
    /// for, so they are priced at zero rather than at API rates —
    /// charging a run for tokens the user's flat fee already covers
    /// would make the cost ledger wrong in the expensive direction.
    pub fn cost_model(&self) -> Option<&str> {
        match self.kind() {
            BackendKind::Subscription => None,
            BackendKind::Api => self.resolved_model.as_deref(),
        }
    }
}

/// A [`Selection`] plus the live provider that will serve it.
pub struct Resolution {
    pub provider: Box<dyn LlmProvider>,
    pub selection: Selection,
}

impl std::fmt::Debug for Resolution {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The boxed provider isn't `Debug`, and printing it would be
        // noise anyway — the decision is what's interesting.
        f.debug_struct("Resolution")
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

/// Decide which backend serves this request. The one decision function.
pub fn select(
    request: &ModelRequest,
    policy: &LlmPolicy,
    creds: &LlmCredentials,
) -> Result<Selection> {
    let usable: Vec<Backend> = policy
        .order()
        .iter()
        .copied()
        .filter(|backend| is_usable(backend, creds))
        .collect();

    if usable.is_empty() {
        return Err(no_backend_error(request, policy, creds));
    }

    let tier = request.tier();

    // An exact model prefers a backend of its own vendor, wherever that
    // backend sits in the ordering.
    if let ModelRequest::Exact {
        name,
        provider: Some(owner),
        ..
    } = request
        && let Some(native) = usable.iter().find(|b| b.serves_api_provider(owner))
    {
        return Ok(Selection {
            backend: *native,
            requested: request.requested().to_string(),
            resolved_model: Some(name.clone()),
            degraded: false,
            tier,
        });
    }

    // Otherwise the highest-priority usable backend serves the tier.
    let chosen = usable[0];
    Ok(Selection {
        backend: chosen,
        requested: request.requested().to_string(),
        resolved_model: model_for_tier(&chosen, tier, policy),
        degraded: matches!(request, ModelRequest::Exact { .. }),
        tier,
    })
}

/// Resolve a request into a live backend ready to be called.
pub fn resolve(
    request: &ModelRequest,
    policy: &LlmPolicy,
    creds: &LlmCredentials,
) -> Result<Resolution> {
    let selection = select(request, policy, creds)?;
    let provider = build_provider(&selection, creds);
    Ok(Resolution {
        provider,
        selection,
    })
}

/// Describe what *would* run, without building a provider or making a
/// call. Backs the Console's per-step tooltip.
pub fn preview(
    request: &ModelRequest,
    policy: &LlmPolicy,
    creds: &LlmCredentials,
) -> Result<Selection> {
    select(request, policy, creds)
}

/// Can this backend run right now?
fn is_usable(backend: &Backend, creds: &LlmCredentials) -> bool {
    match backend {
        Backend::Subscription(spec) => subscription::check(spec).is_ready(),
        Backend::Api(id) => creds.key_for_str(id).is_some(),
    }
}

fn build_provider(selection: &Selection, creds: &LlmCredentials) -> Box<dyn LlmProvider> {
    match selection.backend {
        Backend::Subscription(spec) => Box::new(SubscriptionProvider::new(
            spec,
            selection.resolved_model.clone(),
        )),
        Backend::Api(id) => {
            // Presence was checked before the backend was admitted.
            let key = creds.key_for_str(id).unwrap_or_default().to_string();
            match id {
                "openai" => Box::new(providers::OpenAiProvider::new(key)),
                "anthropic" => Box::new(providers::AnthropicProvider::new(key)),
                _ => Box::new(providers::GeminiProvider::new(key)),
            }
        }
    }
}

/// The model this backend uses for a tier, walking the degradation path
/// if it cannot serve that tier at all.
fn model_for_tier(backend: &Backend, tier: ModelTier, policy: &LlmPolicy) -> Option<String> {
    tier.degradation_path()
        .iter()
        .find_map(|step| policy.model_for(backend, *step))
}

/// Nothing can serve this step. Explain the routes, and say which ones
/// the current configuration actually permits.
fn no_backend_error(
    request: &ModelRequest,
    policy: &LlmPolicy,
    creds: &LlmCredentials,
) -> BrokerError {
    let mut lines = Vec::new();

    if policy.subscriptions_gated_off() {
        lines.push(
            "This is a shared worker, so personal subscriptions are not available here — \
             it needs an API key."
                .to_string(),
        );
    }

    if policy.order().is_empty() {
        lines.push(
            "Every LLM backend is disabled in Settings → AI Providers. Enable at least one."
                .to_string(),
        );
        return BrokerError::LlmNoBackend {
            requested: request.requested().to_string(),
            detail: lines.join("\n"),
        };
    }

    for backend in policy.order() {
        match backend {
            Backend::Subscription(spec) => match subscription::check(spec) {
                subscription::BackendState::SignedOut { hint } => {
                    lines.push(format!("  · {} — signed out; {hint}", spec.display_name));
                }
                subscription::BackendState::NotInstalled => {
                    lines.push(format!(
                        "  · {} — `{}` is not installed",
                        spec.display_name, spec.binary
                    ));
                }
                subscription::BackendState::Ready => {}
            },
            Backend::Api(id) => {
                if creds.key_for_str(id).is_none() {
                    lines.push(format!(
                        "  · {} — no API key; run `cori login {id}`",
                        catalog::api_display_name(id)
                    ));
                }
            }
        }
    }

    lines.insert(
        if policy.subscriptions_gated_off() {
            1
        } else {
            0
        },
        "No configured LLM backend is ready:".to_string(),
    );

    BrokerError::LlmNoBackend {
        requested: request.requested().to_string(),
        detail: lines.join("\n"),
    }
}

#[cfg(test)]
mod tests {
    use super::super::policy::{Deployment, LlmConfig};
    use super::*;

    fn creds_with(provider: &str) -> LlmCredentials {
        let mut c = LlmCredentials::empty();
        match provider {
            "openai" => c.openai_api_key = Some("sk-test".into()),
            "anthropic" => c.anthropic_api_key = Some("sk-ant-test".into()),
            "gemini" => c.gemini_api_key = Some("AIza-test".into()),
            _ => {}
        }
        c
    }

    /// A policy listing only API providers, so the candidate set is
    /// deterministic in CI regardless of what is installed on the host.
    fn api_policy(order: &[&str]) -> LlmPolicy {
        LlmPolicy::for_deployment(
            &LlmConfig {
                priority: Some(order.iter().map(|s| (*s).to_string()).collect()),
                disabled: subscription::BACKENDS.iter().map(|b| b.id.into()).collect(),
                ..Default::default()
            },
            Deployment::Local,
        )
    }

    #[test]
    fn tier_request_uses_the_api_tier_model() {
        let req = catalog::parse_request(Some("fast"));
        let s = select(&req, &api_policy(&["openai"]), &creds_with("openai")).expect("resolves");
        assert_eq!(s.backend_id(), "openai");
        assert_eq!(s.kind(), BackendKind::Api);
        assert_eq!(s.resolved_model.as_deref(), Some("gpt-4o-mini"));
        assert!(!s.degraded);
    }

    #[test]
    fn absent_model_resolves_at_the_default_tier() {
        let req = catalog::parse_request(None);
        let s = select(&req, &api_policy(&["openai"]), &creds_with("openai")).expect("resolves");
        assert_eq!(s.tier, catalog::DEFAULT_TIER);
        assert_eq!(s.resolved_model.as_deref(), Some("gpt-4o"));
    }

    #[test]
    fn priority_order_decides_which_backend_wins() {
        // Two usable API keys; the list is what breaks the tie.
        let mut creds = creds_with("openai");
        creds.anthropic_api_key = Some("sk-ant-test".into());

        let s = select(
            &catalog::parse_request(Some("fast")),
            &api_policy(&["anthropic", "openai"]),
            &creds,
        )
        .expect("resolves");
        assert_eq!(s.backend_id(), "anthropic");

        let s = select(
            &catalog::parse_request(Some("fast")),
            &api_policy(&["openai", "anthropic"]),
            &creds,
        )
        .expect("resolves");
        assert_eq!(s.backend_id(), "openai");
    }

    #[test]
    fn a_backend_without_credentials_is_skipped_not_chosen() {
        // Anthropic ranks first but has no key, so OpenAI serves it.
        let s = select(
            &catalog::parse_request(None),
            &api_policy(&["anthropic", "openai"]),
            &creds_with("openai"),
        )
        .expect("resolves");
        assert_eq!(s.backend_id(), "openai");
    }

    #[test]
    fn exact_model_goes_to_its_own_provider_verbatim() {
        let mut creds = creds_with("openai");
        creds.anthropic_api_key = Some("sk-ant-test".into());
        // Anthropic ranks first, but the step named an OpenAI model.
        let s = select(
            &catalog::parse_request(Some("gpt-4o-mini")),
            &api_policy(&["anthropic", "openai"]),
            &creds,
        )
        .expect("resolves");
        assert_eq!(s.backend_id(), "openai");
        assert_eq!(s.resolved_model.as_deref(), Some("gpt-4o-mini"));
        assert!(!s.degraded, "its own provider is not a degradation");
    }

    #[test]
    fn exact_model_degrades_within_its_tier_on_another_provider() {
        let s = select(
            &catalog::parse_request(Some("gpt-4o-mini")),
            &api_policy(&["anthropic"]),
            &creds_with("anthropic"),
        )
        .expect("resolves");
        assert_eq!(s.backend_id(), "anthropic");
        assert!(s.degraded);
        assert_eq!(s.tier, ModelTier::Fast);
        // Fast in, fast out — never silently upgraded or downgraded.
        assert_eq!(s.resolved_model.as_deref(), Some("claude-3-5-haiku-latest"));
    }

    #[test]
    fn degradation_is_always_visible_in_the_trace() {
        let s = select(
            &catalog::parse_request(Some("o3")),
            &api_policy(&["anthropic"]),
            &creds_with("anthropic"),
        )
        .expect("resolves");
        let note = s.trace_note();
        assert!(note.contains("requested `o3`"), "{note}");
        assert!(note.contains("claude-opus-4-1"), "{note}");
    }

    #[test]
    fn user_model_override_is_what_gets_sent() {
        use std::collections::BTreeMap;
        let policy = LlmPolicy::for_deployment(
            &LlmConfig {
                priority: Some(vec!["openai".into()]),
                disabled: subscription::BACKENDS.iter().map(|b| b.id.into()).collect(),
                models: BTreeMap::from([(
                    "openai".to_string(),
                    BTreeMap::from([("balanced".to_string(), "gpt-4.1".to_string())]),
                )]),
                ..Default::default()
            },
            Deployment::Local,
        );
        let s = select(
            &catalog::parse_request(Some("balanced")),
            &policy,
            &creds_with("openai"),
        )
        .expect("resolves");
        assert_eq!(s.resolved_model.as_deref(), Some("gpt-4.1"));
    }

    #[test]
    fn preview_matches_what_resolve_would_run() {
        // The tooltip must not be able to disagree with execution.
        let req = catalog::parse_request(Some("deep"));
        let policy = api_policy(&["openai"]);
        let creds = creds_with("openai");
        let previewed = preview(&req, &policy, &creds).expect("preview");
        let resolved = resolve(&req, &policy, &creds).expect("resolve");
        assert_eq!(previewed.backend_id(), resolved.selection.backend_id());
        assert_eq!(previewed.resolved_model, resolved.selection.resolved_model);
    }

    #[test]
    fn subscription_calls_are_not_priced_at_api_rates() {
        let s = select(
            &catalog::parse_request(Some("fast")),
            &api_policy(&["openai"]),
            &creds_with("openai"),
        )
        .expect("resolves");
        assert_eq!(s.cost_model(), Some("gpt-4o-mini"));

        let sub = Selection {
            backend: Backend::find("claude").unwrap(),
            requested: "balanced".into(),
            resolved_model: Some("sonnet".into()),
            degraded: false,
            tier: ModelTier::Balanced,
        };
        assert_eq!(
            sub.cost_model(),
            None,
            "already paid for by the subscription"
        );
    }

    #[test]
    fn no_usable_backend_lists_each_one_and_its_fix() {
        let err = select(
            &catalog::parse_request(None),
            &api_policy(&["openai", "anthropic"]),
            &LlmCredentials::empty(),
        )
        .expect_err("nothing usable");
        let msg = err.to_string();
        assert!(msg.contains("cori login openai"), "{msg}");
        assert!(msg.contains("cori login anthropic"), "{msg}");
    }

    #[test]
    fn everything_disabled_says_so_plainly() {
        let policy = LlmPolicy::for_deployment(
            &LlmConfig {
                disabled: Backend::all().iter().map(|b| b.id().to_string()).collect(),
                ..Default::default()
            },
            Deployment::Local,
        );
        let err = select(
            &catalog::parse_request(None),
            &policy,
            &LlmCredentials::empty(),
        )
        .expect_err("nothing enabled");
        assert!(err.to_string().contains("disabled"), "{err}");
    }

    #[test]
    fn shared_worker_error_says_why_subscriptions_are_off() {
        let gated = LlmPolicy::for_deployment(&LlmConfig::default(), Deployment::Shared);
        let err = select(
            &catalog::parse_request(None),
            &gated,
            &LlmCredentials::empty(),
        )
        .expect_err("nothing usable");
        assert!(err.to_string().contains("shared worker"), "{err}");
    }
}
