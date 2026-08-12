//! Capability tiers, and how a step's declared `model` maps onto them.
//!
//! A workflow declares *what kind of model it needs*, not which vendor
//! serves it. Three tiers cover the useful spread:
//!
//! | Tier       | For | Typical |
//! |---|---|---|
//! | `fast`     | classification, extraction, short rewrites | gpt-4o-mini, haiku, flash |
//! | `balanced` | the default — most steps | gpt-4o, sonnet, pro |
//! | `deep`     | multi-constraint reasoning, long synthesis | o3, opus |
//!
//! `model` is optional in the SDK. What a step declares becomes a
//! [`ModelRequest`]:
//!
//! - absent            → [`ModelRequest::Tier`] at [`ModelTier::Balanced`]
//! - `"fast"` etc.     → [`ModelRequest::Tier`]
//! - `"gpt-4o-mini"`   → [`ModelRequest::Exact`], carrying the tier it
//!   falls into so an unavailable exact model degrades to a peer rather
//!   than to whatever happens to be installed.
//!
//! An exact name is a *preference*, not a pin: [`super::resolve`] uses
//! the owning provider when that backend is usable and otherwise serves
//! the request's tier from another backend, recording both the requested
//! and the resolved model in the run trace.

use std::fmt;

/// How much model a step needs. Ordered `Fast < Balanced < Deep`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ModelTier {
    Fast,
    Balanced,
    Deep,
}

/// The tier used when a step declares no model at all.
pub const DEFAULT_TIER: ModelTier = ModelTier::Balanced;

impl ModelTier {
    pub fn as_str(self) -> &'static str {
        match self {
            ModelTier::Fast => "fast",
            ModelTier::Balanced => "balanced",
            ModelTier::Deep => "deep",
        }
    }

    /// Parse a tier alias. Accepts a few obvious synonyms so authors
    /// don't have to memorise the exact word.
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "fast" | "cheap" | "small" | "mini" => Some(ModelTier::Fast),
            "balanced" | "default" | "standard" | "medium" => Some(ModelTier::Balanced),
            "deep" | "reasoning" | "smart" | "large" | "best" => Some(ModelTier::Deep),
            _ => None,
        }
    }

    /// Tiers to try when this one cannot be served, nearest first.
    /// Adjacent capability beats "whatever is left": a `deep` step
    /// degrades to `balanced` before it degrades to `fast`.
    pub fn degradation_path(self) -> &'static [ModelTier] {
        match self {
            ModelTier::Fast => &[ModelTier::Fast, ModelTier::Balanced, ModelTier::Deep],
            ModelTier::Balanced => &[ModelTier::Balanced, ModelTier::Deep, ModelTier::Fast],
            ModelTier::Deep => &[ModelTier::Deep, ModelTier::Balanced, ModelTier::Fast],
        }
    }
}

impl fmt::Display for ModelTier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What a step asked for, after parsing its declared `model`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelRequest {
    /// A tier alias, or nothing at all.
    Tier(ModelTier),
    /// A concrete vendor model name. Treated as a preference.
    Exact {
        name: String,
        /// Provider that natively serves this name, when recognised.
        provider: Option<&'static str>,
        /// Tier to fall back to when `name` cannot be served.
        tier: ModelTier,
    },
}

impl ModelRequest {
    /// The tier this request resolves at, exact or not.
    pub fn tier(&self) -> ModelTier {
        match self {
            ModelRequest::Tier(t) => *t,
            ModelRequest::Exact { tier, .. } => *tier,
        }
    }

    /// What the step declared, for traces and error messages.
    pub fn requested(&self) -> &str {
        match self {
            ModelRequest::Tier(t) => t.as_str(),
            ModelRequest::Exact { name, .. } => name,
        }
    }
}

/// Parse a step's declared model. `None` (no `model:` field) yields the
/// host default tier.
pub fn parse_request(model: Option<&str>) -> ModelRequest {
    let Some(raw) = model.map(str::trim).filter(|m| !m.is_empty()) else {
        return ModelRequest::Tier(DEFAULT_TIER);
    };
    if let Some(tier) = ModelTier::parse(raw) {
        return ModelRequest::Tier(tier);
    }
    ModelRequest::Exact {
        name: raw.to_string(),
        provider: api_provider_for_model(raw),
        tier: tier_for_model(raw),
    }
}

/// Map a concrete model name to the HTTP API provider that serves it.
/// `None` for names no known vendor prefix claims — those are still
/// runnable, they just can't pick a provider on their own.
pub fn api_provider_for_model(model: &str) -> Option<&'static str> {
    let m = model.to_ascii_lowercase();
    if m.starts_with("gpt-") || m.starts_with("o1") || m.starts_with("o3") || m.starts_with("o4") {
        Some("openai")
    } else if m.starts_with("claude-") {
        Some("anthropic")
    } else if m.starts_with("gemini-") {
        Some("gemini")
    } else {
        None
    }
}

/// Classify a concrete model name into the tier it belongs to, so an
/// unavailable exact model degrades to a peer of similar capability.
///
/// Substring matching on vendor naming conventions: every vendor marks
/// its small models (`mini`, `haiku`, `flash`, `nano`, `lite`) and its
/// reasoning models (`o1`/`o3`, `opus`, `-thinking`) in the name itself.
/// Unrecognised names land on [`DEFAULT_TIER`].
pub fn tier_for_model(model: &str) -> ModelTier {
    let m = model.to_ascii_lowercase();

    // Reasoning / frontier markers win over size markers: `o3-mini` is a
    // reasoning model that happens to be small, and callers who asked
    // for it want the reasoning.
    if m.starts_with("o1") || m.starts_with("o3") || m.starts_with("o4") {
        return ModelTier::Deep;
    }
    if m.contains("opus") || m.contains("thinking") || m.contains("ultra") {
        return ModelTier::Deep;
    }
    if m.contains("mini")
        || m.contains("nano")
        || m.contains("haiku")
        || m.contains("flash")
        || m.contains("lite")
        || m.contains("small")
    {
        return ModelTier::Fast;
    }
    if m.contains("sonnet") || m.contains("gpt-4o") || m.contains("gpt-5") || m.contains("pro") {
        return ModelTier::Balanced;
    }
    DEFAULT_TIER
}

/// The concrete model an HTTP API provider uses for a tier. These are
/// the names Cori sends when a step asked for a tier rather than a
/// model, and the landing spot when an exact model degrades.
pub fn api_model_for_tier(provider: &str, tier: ModelTier) -> Option<&'static str> {
    Some(match (provider, tier) {
        ("openai", ModelTier::Fast) => "gpt-4o-mini",
        ("openai", ModelTier::Balanced) => "gpt-4o",
        ("openai", ModelTier::Deep) => "o3",
        ("anthropic", ModelTier::Fast) => "claude-3-5-haiku-latest",
        ("anthropic", ModelTier::Balanced) => "claude-sonnet-4-5",
        ("anthropic", ModelTier::Deep) => "claude-opus-4-1",
        ("gemini", ModelTier::Fast) => "gemini-2.0-flash",
        ("gemini", ModelTier::Balanced) => "gemini-2.5-pro",
        ("gemini", ModelTier::Deep) => "gemini-2.5-pro",
        _ => return None,
    })
}

/// Every HTTP API provider Cori can talk to, in preference order.
pub const API_PROVIDERS: [&str; 3] = ["openai", "anthropic", "gemini"];

/// Human-facing name for an API provider.
pub fn api_display_name(provider: &str) -> &'static str {
    match provider {
        "openai" => "OpenAI",
        "anthropic" => "Anthropic",
        "gemini" => "Google Gemini",
        _ => "Unknown provider",
    }
}

/// Models worth offering in the Console's per-tier pickers.
///
/// Suggestions, not a closed set: the pickers accept any string, because
/// vendors ship models faster than Cori ships releases and a user who
/// knows the new name should not have to wait for us.
pub fn api_model_suggestions(provider: &str) -> &'static [&'static str] {
    match provider {
        "openai" => &[
            "gpt-4o-mini",
            "gpt-4o",
            "gpt-4.1-mini",
            "gpt-4.1",
            "gpt-5-mini",
            "gpt-5",
            "o3-mini",
            "o3",
        ],
        "anthropic" => &[
            "claude-3-5-haiku-latest",
            "claude-haiku-4-5",
            "claude-sonnet-4-5",
            "claude-opus-4-1",
        ],
        "gemini" => &["gemini-2.0-flash", "gemini-2.5-flash", "gemini-2.5-pro"],
        _ => &[],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_model_is_the_default_tier() {
        assert_eq!(parse_request(None), ModelRequest::Tier(DEFAULT_TIER));
        assert_eq!(parse_request(Some("  ")), ModelRequest::Tier(DEFAULT_TIER));
    }

    #[test]
    fn tier_aliases_parse() {
        assert_eq!(
            parse_request(Some("fast")),
            ModelRequest::Tier(ModelTier::Fast)
        );
        assert_eq!(
            parse_request(Some("reasoning")),
            ModelRequest::Tier(ModelTier::Deep)
        );
        assert_eq!(
            parse_request(Some("BALANCED")),
            ModelRequest::Tier(ModelTier::Balanced)
        );
    }

    #[test]
    fn exact_models_carry_provider_and_fallback_tier() {
        let req = parse_request(Some("gpt-4o-mini"));
        assert_eq!(
            req,
            ModelRequest::Exact {
                name: "gpt-4o-mini".to_string(),
                provider: Some("openai"),
                tier: ModelTier::Fast,
            }
        );
        assert_eq!(req.tier(), ModelTier::Fast);
        assert_eq!(req.requested(), "gpt-4o-mini");
    }

    #[test]
    fn reasoning_markers_outrank_size_markers() {
        // o3-mini is small *and* a reasoning model — asking for it means
        // asking for the reasoning, so it must not degrade to `fast`.
        assert_eq!(tier_for_model("o3-mini"), ModelTier::Deep);
        assert_eq!(tier_for_model("claude-3-5-haiku-latest"), ModelTier::Fast);
        assert_eq!(tier_for_model("claude-opus-4-1"), ModelTier::Deep);
        assert_eq!(tier_for_model("gemini-2.0-flash"), ModelTier::Fast);
        assert_eq!(tier_for_model("claude-sonnet-4-5"), ModelTier::Balanced);
    }

    #[test]
    fn unknown_models_are_usable_at_the_default_tier() {
        let req = parse_request(Some("llama-3.1-70b"));
        assert_eq!(req.tier(), DEFAULT_TIER);
        assert!(matches!(req, ModelRequest::Exact { provider: None, .. }));
    }

    #[test]
    fn degradation_prefers_adjacent_capability() {
        assert_eq!(
            ModelTier::Deep.degradation_path(),
            &[ModelTier::Deep, ModelTier::Balanced, ModelTier::Fast]
        );
        assert_eq!(
            ModelTier::Fast.degradation_path(),
            &[ModelTier::Fast, ModelTier::Balanced, ModelTier::Deep]
        );
    }

    #[test]
    fn every_api_provider_serves_every_tier() {
        for provider in API_PROVIDERS {
            for tier in [ModelTier::Fast, ModelTier::Balanced, ModelTier::Deep] {
                assert!(
                    api_model_for_tier(provider, tier).is_some(),
                    "{provider} has no model for {tier}"
                );
            }
        }
    }
}
