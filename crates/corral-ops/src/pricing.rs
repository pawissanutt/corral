//! Pricing coverage over response evidence, including retained reviewer runs.

use std::collections::BTreeMap;

use anyhow::Result;
use corral_core::pricing::{CATALOG_DATE, catalog_prices};
use corral_core::{Price, Store};
use jiff::Timestamp;
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct UnpricedModel {
    pub model: Option<String>,
    pub entries: u64,
    pub tokens: u64,
}

#[derive(Debug, Serialize)]
pub struct CacheRateFallback {
    pub model: Option<String>,
    pub price_source: String,
    pub entries: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
}

#[derive(Debug, Serialize)]
pub struct PriceShadow {
    pub model: String,
    pub config_model: String,
    pub config_price: Price,
    pub catalog_price: Price,
}

#[derive(Debug, Serialize)]
pub struct PricingCheck {
    pub catalog_date: &'static str,
    pub unpriced_tokens: u64,
    pub unpriced_models: Vec<UnpricedModel>,
    pub cache_rate_fallbacks: Vec<CacheRateFallback>,
    pub shadows: Vec<PriceShadow>,
}

/// Refresh available transcript evidence and check all retained usage.
pub fn pricing_check(store: &Store) -> Result<PricingCheck> {
    crate::usage::collect_missing(store, None)?;
    pricing_coverage(store, None)
}

/// Read-only coverage, repriced in memory. The cutoff selects each run by start time.
/// This permits doctor/startup checks without scanning transcripts or changing task files.
pub fn pricing_coverage(store: &Store, since: Option<Timestamp>) -> Result<PricingCheck> {
    let mut unpriced = BTreeMap::new();
    let mut fallbacks = BTreeMap::new();
    for task in store.list()? {
        let mut runs: Vec<_> = task.runs.iter().collect();
        while let Some(run) = runs.pop() {
            runs.extend(&run.review_runs);
            if since.is_some_and(|since| run.started < since) {
                continue;
            }
            let Some(mut usage) = run.usage.clone() else {
                continue;
            };
            usage.reprice(&store.config, run.model.as_deref());
            for entry in usage.entries {
                if entry.estimated_usd.is_none() {
                    let row = unpriced
                        .entry(entry.model.clone())
                        .or_insert(UnpricedModel {
                            model: entry.model.clone(),
                            entries: 0,
                            tokens: 0,
                        });
                    row.entries = row.entries.saturating_add(1);
                    row.tokens = row.tokens.saturating_add(entry.total_tokens());
                } else if let Some((price, source)) = entry
                    .model
                    .as_deref()
                    .and_then(|model| store.config.price_with_source(model))
                {
                    let reads = if price.cache_read.is_none() {
                        entry.cache_read
                    } else {
                        0
                    };
                    let writes = if price.cache_write.is_none() {
                        entry.cache_write
                    } else {
                        0
                    };
                    if reads > 0 || writes > 0 {
                        let row =
                            fallbacks
                                .entry(entry.model.clone())
                                .or_insert(CacheRateFallback {
                                    model: entry.model,
                                    price_source: source.to_owned(),
                                    entries: 0,
                                    cache_read_tokens: 0,
                                    cache_write_tokens: 0,
                                });
                        row.entries = row.entries.saturating_add(1);
                        row.cache_read_tokens = row.cache_read_tokens.saturating_add(reads);
                        row.cache_write_tokens = row.cache_write_tokens.saturating_add(writes);
                    }
                }
            }
        }
    }
    let shadows = catalog_prices()
        .iter()
        .filter_map(|(model, catalog)| {
            let (config_model, configured) =
                store.config.pricing.get_key_value(model).or_else(|| {
                    store
                        .config
                        .pricing
                        .iter()
                        .find(|(_, price)| price.aliases.iter().any(|alias| alias == model))
                })?;
            let rates = |price: &Price| {
                [
                    price.input,
                    price.output,
                    price.cache_read(),
                    price.cache_write(),
                ]
            };
            (rates(configured) != rates(catalog)).then(|| PriceShadow {
                model: model.clone(),
                config_model: config_model.clone(),
                config_price: configured.clone(),
                catalog_price: catalog.clone(),
            })
        })
        .collect();
    Ok(PricingCheck {
        catalog_date: CATALOG_DATE,
        unpriced_tokens: unpriced.values().fold(0_u64, |sum, row: &UnpricedModel| {
            sum.saturating_add(row.tokens)
        }),
        unpriced_models: unpriced.into_values().collect(),
        cache_rate_fallbacks: fallbacks.into_values().collect(),
        shadows,
    })
}
