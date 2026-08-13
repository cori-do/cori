//! Portable workflow levels and provider-specific model defaults.
//!
//! Workflows never select a vendor model. They declare `low`, `medium`, or
//! `high`; the one active backend maps that level to a concrete model. Model
//! names remain a machine-owned advanced setting.

use std::fmt;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LlmLevel {
    Low,
    Medium,
    High,
}

pub const DEFAULT_LEVEL: LlmLevel = LlmLevel::Medium;

impl LlmLevel {
    pub const ALL: [Self; 3] = [Self::Low, Self::Medium, Self::High];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }

    /// Parse the strict workflow/config vocabulary.
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "low" => Some(Self::Low),
            "medium" => Some(Self::Medium),
            "high" => Some(Self::High),
            _ => None,
        }
    }

    /// Runtime-only translation for Temporal activities started before
    /// workflows moved from `model` to `level`.
    pub fn from_legacy_model(model: &str) -> Self {
        let value = model.trim().to_ascii_lowercase();
        match value.as_str() {
            "fast" | "cheap" | "small" | "mini" => return Self::Low,
            "balanced" | "default" | "standard" | "medium" => return Self::Medium,
            "deep" | "reasoning" | "smart" | "large" | "best" => return Self::High,
            _ => {}
        }
        if value.starts_with("o1")
            || value.starts_with("o3")
            || value.starts_with("o4")
            || value.contains("opus")
            || value.contains("thinking")
            || value.contains("ultra")
        {
            Self::High
        } else if value.contains("mini")
            || value.contains("nano")
            || value.contains("haiku")
            || value.contains("flash")
            || value.contains("lite")
            || value.contains("small")
        {
            Self::Low
        } else {
            Self::Medium
        }
    }
}

impl fmt::Display for LlmLevel {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// The concrete model an HTTP API provider uses for a level.
pub fn api_model_for_level(provider: &str, level: LlmLevel) -> Option<&'static str> {
    Some(match (provider, level) {
        ("openai", LlmLevel::Low) => "gpt-4o-mini",
        ("openai", LlmLevel::Medium) => "gpt-4o",
        ("openai", LlmLevel::High) => "o3",
        ("anthropic", LlmLevel::Low) => "claude-3-5-haiku-latest",
        ("anthropic", LlmLevel::Medium) => "claude-sonnet-4-5",
        ("anthropic", LlmLevel::High) => "claude-opus-4-1",
        ("gemini", LlmLevel::Low) => "gemini-2.0-flash",
        ("gemini", LlmLevel::Medium) => "gemini-2.5-pro",
        ("gemini", LlmLevel::High) => "gemini-2.5-pro",
        _ => return None,
    })
}

pub const API_PROVIDERS: [&str; 3] = ["openai", "anthropic", "gemini"];

pub fn api_display_name(provider: &str) -> &'static str {
    match provider {
        "openai" => "OpenAI",
        "anthropic" => "Anthropic",
        "gemini" => "Google Gemini",
        _ => "Unknown provider",
    }
}

/// Suggestions only: advanced settings accept any model name.
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
    fn strict_levels_parse() {
        assert_eq!(LlmLevel::parse("low"), Some(LlmLevel::Low));
        assert_eq!(LlmLevel::parse("medium"), Some(LlmLevel::Medium));
        assert_eq!(LlmLevel::parse("high"), Some(LlmLevel::High));
        assert_eq!(LlmLevel::parse("fast"), None);
        assert_eq!(LlmLevel::parse("MEDIUM"), None);
    }

    #[test]
    fn legacy_models_map_only_for_history_resume() {
        assert_eq!(LlmLevel::from_legacy_model("fast"), LlmLevel::Low);
        assert_eq!(LlmLevel::from_legacy_model("gpt-4o-mini"), LlmLevel::Low);
        assert_eq!(LlmLevel::from_legacy_model("o3-mini"), LlmLevel::High);
        assert_eq!(LlmLevel::from_legacy_model("gpt-5"), LlmLevel::Medium);
    }

    #[test]
    fn every_api_provider_serves_every_level() {
        for provider in API_PROVIDERS {
            for level in LlmLevel::ALL {
                assert!(
                    api_model_for_level(provider, level).is_some(),
                    "{provider} has no model for {level}"
                );
            }
        }
    }
}
