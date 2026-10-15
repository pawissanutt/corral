//! Editable model prices, in USD per million tokens.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::Config;

/// Base API rates in USD per million tokens, not a subscription invoice.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Price {
    /// Uncached input rate.
    pub input: f64,
    /// Output rate.
    pub output: f64,
    /// Cache read rate; absent means use `input`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read: Option<f64>,
    /// Cache write rate; absent means use `input`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write: Option<f64>,
    /// Explicit alternate model IDs; no fuzzy matching or provider stripping.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
}

impl Price {
    /// Effective cache read rate, falling back to uncached input.
    pub fn cache_read(&self) -> f64 {
        self.cache_read.unwrap_or(self.input)
    }

    /// Effective cache write rate, falling back to uncached input.
    pub fn cache_write(&self) -> f64 {
        self.cache_write.unwrap_or(self.input)
    }
}

impl Config {
    /// Looks up an exact model ID first, then an explicit alias. Unknown models have no price.
    pub fn price(&self, model: &str) -> Option<&Price> {
        self.pricing.get(model).or_else(|| {
            self.pricing
                .values()
                .find(|price| price.aliases.iter().any(|alias| alias == model))
        })
    }

    pub(crate) fn pricing_problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        let mut aliases = BTreeMap::new();
        for (model, price) in &self.pricing {
            for (field, rate) in [
                ("input", Some(price.input)),
                ("output", Some(price.output)),
                ("cache_read", price.cache_read),
                ("cache_write", price.cache_write),
            ] {
                if rate.is_some_and(|rate| !rate.is_finite() || rate < 0.0) {
                    out.push(format!(
                        "pricing.{model}.{field} must be finite and nonnegative"
                    ));
                }
            }
            for alias in &price.aliases {
                if let Some(owner) = aliases.insert(alias, model)
                    && owner != model
                {
                    out.push(format!(
                        "pricing.{model}.aliases: ambiguous alias {alias:?}, also used by {owner}"
                    ));
                }
                if alias != model && self.pricing.contains_key(alias) {
                    out.push(format!(
                        "pricing.{model}.aliases: alias {alias:?} conflicts with an exact model ID"
                    ));
                }
            }
        }
        out
    }
}

/// Frozen subset of installed omp and OmO catalogs inspected on 2026-10-15.
/// Sol Fast rates include omp's 2x API priority premium, not subscription usage.
/// See `docs/pricing.md` for source paths, exact-ID equivalence, and tier limitations.
pub(crate) fn starter_prices() -> BTreeMap<String, Price> {
    [
        ("claude-haiku-5-5", 0.1, 0.5, 0.01, 0.125),
        ("claude-opus-4-6", 5.0, 25.0, 0.5, 6.25),
        ("claude-opus-4-7", 5.0, 25.0, 0.5, 6.25),
        ("claude-opus-4-8", 5.0, 25.0, 0.5, 6.25),
        ("claude-opus-5-5", 4.0, 20.0, 0.2, 5.0),
        ("claude-sonnet-4-6", 3.0, 15.0, 0.3, 3.75),
        ("claude-sonnet-5", 2.0, 10.0, 0.2, 2.5),
        ("claude-sonnet-5-5", 2.0, 10.0, 0.1, 2.5),
        ("gpt-5.6", 4.0, 20.0, 0.4, 5.0),
        ("gpt-5.6-luna", 0.2, 1.2, 0.02, 0.25),
        ("gpt-5.6-luna-fast", 0.2, 1.2, 0.02, 0.25),
        ("gpt-5.6-sol", 4.0, 20.0, 0.4, 5.0),
        ("gpt-5.6-sol-fast", 8.0, 40.0, 0.8, 10.0),
        ("gpt-5.6-terra", 2.0, 12.0, 0.2, 2.5),
        ("gpt-6-astra", 10.0, 50.0, 1.0, 12.5),
        ("gpt-6-astra-fast", 10.0, 50.0, 1.0, 12.5),
        ("gpt-6-luna", 0.1, 0.5, 0.01, 0.125),
        ("gpt-6-luna-fast", 0.1, 0.5, 0.01, 0.125),
        ("gpt-6-sol", 2.0, 10.0, 0.2, 2.5),
        ("gpt-6-sol-fast", 4.0, 20.0, 0.4, 5.0),
        ("gpt-6.1-sol", 2.0, 10.0, 0.1, 2.5),
        ("gpt-6.1-sol-fast", 4.0, 20.0, 0.2, 5.0),
    ]
    .into_iter()
    .map(|(model, input, output, cache_read, cache_write)| {
        (
            model.to_string(),
            Price {
                input,
                output,
                cache_read: Some(cache_read),
                cache_write: Some(cache_write),
                aliases: Vec::new(),
            },
        )
    })
    .collect()
}

#[cfg(test)]
#[path = "pricing_tests.rs"]
mod tests;
