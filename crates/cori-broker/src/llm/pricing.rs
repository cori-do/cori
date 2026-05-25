//! Per-model pricing table.
//!
//! Loaded from `providers.toml` embedded at compile time. Prices are in
//! EUR per 1M tokens; we don't bother with USD<->EUR conversion in v1 —
//! the embedded numbers are already converted at a recent rate and the
//! file is regenerated when prices drift.

use std::collections::HashMap;
use std::sync::OnceLock;

use serde::Deserialize;

const PROVIDERS_TOML: &str = include_str!("../../providers.toml");

#[derive(Debug, Clone, Deserialize)]
pub struct ModelPricing {
    /// EUR per 1M input tokens.
    pub input_per_mtok_eur: f64,
    /// EUR per 1M output tokens.
    pub output_per_mtok_eur: f64,
}

#[derive(Debug, Deserialize)]
struct PricingFile {
    #[serde(default)]
    models: HashMap<String, ModelPricing>,
}

fn table() -> &'static HashMap<String, ModelPricing> {
    static CELL: OnceLock<HashMap<String, ModelPricing>> = OnceLock::new();
    CELL.get_or_init(|| {
        toml::from_str::<PricingFile>(PROVIDERS_TOML)
            .map(|f| f.models)
            .unwrap_or_default()
    })
}

/// Look up pricing for a model. Returns `None` for unknown models — we
/// still let the call go through, we just can't report a cost.
pub fn lookup(model: &str) -> Option<&'static ModelPricing> {
    table().get(model)
}

/// Compute the cost of a call in EUR.
pub fn cost_eur(model: &str, input_tokens: u64, output_tokens: u64) -> Option<f64> {
    let p = lookup(model)?;
    let inp = (input_tokens as f64) * p.input_per_mtok_eur / 1_000_000.0;
    let out = (output_tokens as f64) * p.output_per_mtok_eur / 1_000_000.0;
    Some(inp + out)
}
