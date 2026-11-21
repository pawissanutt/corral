//! Editable model prices, in USD per million tokens.

use std::collections::BTreeMap;

pub use corral_types::pricing::{
    CATALOG_BEDROCK_SOURCE, CATALOG_DATE, CATALOG_SOURCE, Price, bedrock_catalog_prices,
    catalog_prices,
};

use crate::Config;

impl Config {
    /// Config exact ID, config alias, then catalog exact IDs. Unknown models have no price.
    pub fn price(&self, model: &str) -> Option<&Price> {
        self.price_with_source(model).map(|(price, _)| price)
    }

    /// Resolve a rate together with its reportable provenance.
    pub fn price_with_source(&self, model: &str) -> Option<(&Price, &'static str)> {
        corral_types::pricing::price_with_source(&self.pricing, model)
    }

    pub(crate) fn pricing_problems(&self) -> Vec<String> {
        corral_types::pricing::pricing_problems(&self.pricing)
            .into_iter()
            .map(|(key, message)| {
                let separator = if key.ends_with(".aliases") { ": " } else { " " };
                format!("{key}{separator}{message}")
            })
            .collect()
    }
}

pub(crate) fn starter_prices() -> BTreeMap<String, Price> {
    catalog_prices().clone()
}

#[cfg(test)]
#[path = "pricing_tests.rs"]
mod tests;
