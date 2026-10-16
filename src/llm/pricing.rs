//! What a request cost: list prices for well-known models, overridable per model.

use serde::{Deserialize, Serialize};

use crate::config::{ModelPricing, ModelSelection, ProviderKind};

use super::types::TokenUsage;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Cost {
    /// Billed per token, in US dollars.
    #[serde(rename = "usd")]
    Usd(f64),
    /// Counted against the signed-in ChatGPT plan instead of billed per token.
    #[serde(rename = "chatgpt_plan")]
    ChatGptPlan,
}

impl Cost {
    pub fn describe(&self) -> String {
        match self {
            Cost::Usd(amount) => format_usd(*amount),
            Cost::ChatGptPlan => "covered by the ChatGPT plan".to_string(),
        }
    }
}

pub fn format_usd(amount: f64) -> String {
    if amount > 0.0 && amount < 0.01 { format!("${amount:.4}") } else { format!("${amount:.2}") }
}

impl ModelPricing {
    pub fn cost_of(&self, usage: &TokenUsage) -> f64 {
        let cached = usage.cached_input_tokens.min(usage.input_tokens);
        let uncached = usage.input_tokens - cached;
        let cached_price = self.cached_input_per_million.unwrap_or(self.input_per_million);
        (uncached as f64 * self.input_per_million + cached as f64 * cached_price + usage.output_tokens as f64 * self.output_per_million)
            / 1_000_000.0
    }
}

/// The cost of `usage` with this model, or `None` when no price is known.
pub fn cost_for(kind: ProviderKind, selection: &ModelSelection, usage: &TokenUsage) -> Option<Cost> {
    if kind == ProviderKind::ChatGpt {
        return Some(Cost::ChatGptPlan);
    }
    price_for(kind, selection).map(|pricing| Cost::Usd(pricing.cost_of(usage)))
}

pub fn price_for(kind: ProviderKind, selection: &ModelSelection) -> Option<ModelPricing> {
    selection.pricing.or_else(|| builtin_price(kind, &selection.model))
}

/// List prices in US dollars per million tokens: input, cached input, output.
const ANTHROPIC_PRICES: &[(&str, f64, f64, f64)] = &[
    ("claude-fable-5-1", 10.0, 0.25, 50.0),
    ("claude-fable-5", 10.0, 1.0, 50.0),
    ("claude-opus-5-5", 4.0, 0.20, 20.0),
    ("claude-opus-5", 5.0, 0.50, 25.0),
    ("claude-opus-4-8", 5.0, 0.50, 25.0),
    ("claude-opus-4-7", 5.0, 0.50, 25.0),
    ("claude-opus-4-6", 5.0, 0.50, 25.0),
    ("claude-sonnet-5-5", 2.0, 0.20, 10.0),
    ("claude-sonnet-5", 2.0, 0.20, 10.0),
    ("claude-sonnet-4-6", 3.0, 0.30, 15.0),
    ("claude-haiku-5-5", 0.10, 0.01, 0.50),
    ("claude-haiku-4-5", 1.0, 0.10, 5.0),
];

const OPENAI_PRICES: &[(&str, f64, f64, f64)] = &[
    ("gpt-5", 1.25, 0.125, 10.0),
    ("gpt-5.1", 1.25, 0.125, 10.0),
    ("gpt-5-mini", 0.25, 0.025, 2.0),
    ("gpt-5-nano", 0.05, 0.005, 0.40),
];

pub fn builtin_price(kind: ProviderKind, model: &str) -> Option<ModelPricing> {
    let table = match kind {
        ProviderKind::Anthropic => ANTHROPIC_PRICES,
        ProviderKind::OpenAi => OPENAI_PRICES,
        ProviderKind::OpenAiCompatible | ProviderKind::ChatGpt => return None,
    };
    let model = without_date_suffix(model.trim());
    table.iter().find(|(name, ..)| *name == model).map(|&(_, input, cached, output)| ModelPricing {
        input_per_million: input,
        cached_input_per_million: Some(cached),
        output_per_million: output,
    })
}

/// `gpt-5-2025-08-07` -> `gpt-5`, `claude-haiku-4-5-20251001` -> `claude-haiku-4-5`.
fn without_date_suffix(model: &str) -> &str {
    let is_digits = |s: &str, len: usize| s.len() == len && s.bytes().all(|b| b.is_ascii_digit());
    if let Some((base, date)) = model.rsplit_once('-')
        && is_digits(date, 8)
    {
        return base;
    }
    let parts: Vec<&str> = model.rsplitn(4, '-').collect();
    if parts.len() == 4 && is_digits(parts[0], 2) && is_digits(parts[1], 2) && is_digits(parts[2], 4) {
        return parts[3];
    }
    model
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prices_known_models_and_cached_input() {
        let usage = TokenUsage { input_tokens: 1_000_000, output_tokens: 100_000, cached_input_tokens: 500_000, reasoning_tokens: 0 };
        let price = builtin_price(ProviderKind::Anthropic, "claude-opus-5-5").unwrap();
        assert!((price.cost_of(&usage) - (2.0 + 0.10 + 2.0)).abs() < 1e-9);
        assert!(builtin_price(ProviderKind::OpenAi, "gpt-5-2025-08-07").is_some());
        assert!(builtin_price(ProviderKind::Anthropic, "claude-haiku-4-5-20251001").is_some());
        assert!(builtin_price(ProviderKind::OpenAi, "gpt-5.4").is_none());
        assert!(builtin_price(ProviderKind::OpenAiCompatible, "gpt-5").is_none());
    }

    #[test]
    fn overrides_and_plan_usage() {
        let mut selection = ModelSelection { model: "my-model".into(), ..Default::default() };
        let usage = TokenUsage { input_tokens: 2_000_000, output_tokens: 0, ..Default::default() };
        assert_eq!(cost_for(ProviderKind::OpenAiCompatible, &selection, &usage), None);
        selection.pricing = Some(ModelPricing { input_per_million: 1.5, cached_input_per_million: None, output_per_million: 3.0 });
        assert_eq!(cost_for(ProviderKind::OpenAiCompatible, &selection, &usage), Some(Cost::Usd(3.0)));
        assert_eq!(cost_for(ProviderKind::ChatGpt, &selection, &usage), Some(Cost::ChatGptPlan));
        assert_eq!(format_usd(0.004), "$0.0040");
        assert_eq!(format_usd(1.234), "$1.23");
    }
}
