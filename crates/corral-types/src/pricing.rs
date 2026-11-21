//! Editable model prices, in USD per million tokens.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

/// Verification date of the built-in API-equivalent rates.
pub const CATALOG_DATE: &str = "2026-10-15";
pub const CATALOG_SOURCE: &str = "catalog@2026-10-15";
/// Newer Pi supplemental model rates, separate from the older frozen catalog.
pub const CATALOG_SUPPLEMENTAL_SOURCE: &str = "catalog@pi-supplemental";
/// Provenance of the frozen Amazon Bedrock rates, generated from pi-ai 1.5.2.
pub const CATALOG_BEDROCK_SOURCE: &str = "catalog-bedrock@pi-ai-1.5.2";

/// Frozen catalog, independent of the editable configuration and installed harnesses.
pub fn catalog_prices() -> &'static BTreeMap<String, Price> {
    static CATALOG: OnceLock<BTreeMap<String, Price>> = OnceLock::new();
    CATALOG.get_or_init(starter_prices)
}

/// Frozen Bedrock catalog keyed by the bare Bedrock model ID that Pi records.
pub fn bedrock_catalog_prices() -> &'static BTreeMap<String, Price> {
    static CATALOG: OnceLock<BTreeMap<String, Price>> = OnceLock::new();
    CATALOG.get_or_init(|| {
        serde_json::from_str(include_str!("pricing_bedrock.json")).unwrap_or_default()
    })
}

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

    /// Quote disjoint token categories at API rates, not the request's bill.
    pub fn cost(&self, input: u64, output: u64, cache_read: u64, cache_write: u64) -> Option<f64> {
        let categories = [
            (input, self.input),
            (output, self.output),
            (cache_read, self.cache_read()),
            (cache_write, self.cache_write()),
        ];
        let mut cost = 0.0;
        for (tokens, rate) in categories {
            if !rate.is_finite() || rate < 0.0 {
                return None;
            }
            cost += serde_json::Number::from(tokens).as_f64()? * rate;
        }
        let usd = cost / 1_000_000.0;
        usd.is_finite().then_some(usd)
    }
}

/// Exact configured model IDs take precedence over configured aliases.
fn configured_price<'a>(pricing: &'a BTreeMap<String, Price>, model: &str) -> Option<&'a Price> {
    pricing.get(model).or_else(|| {
        pricing
            .values()
            .find(|price| price.aliases.iter().any(|alias| alias == model))
    })
}

/// Resolve an exact model ID or a configured alias, with the rate's provenance.
pub fn price_with_source<'a>(
    pricing: &'a BTreeMap<String, Price>,
    model: &str,
) -> Option<(&'a Price, &'static str)> {
    configured_price(pricing, model)
        .map(|price| (price, "config"))
        .or_else(|| {
            catalog_prices()
                .get(model)
                .map(|price| (price, catalog_source(model)))
        })
        .or_else(|| {
            bedrock_catalog_prices()
                .get(model)
                .map(|price| (price, CATALOG_BEDROCK_SOURCE))
        })
}

fn catalog_source(model: &str) -> &'static str {
    match model {
        "gpt-6.2-astra" | "gpt-6.2-sol" => CATALOG_SUPPLEMENTAL_SOURCE,
        _ => CATALOG_SOURCE,
    }
}

/// Live Pi reports include a provider namespace. Only explicit built-in catalog
/// equivalences are translated. A different (e.g. local) provider's same model
/// string does not inherit an API price, and an unknown model is never guessed.
pub fn request_price_with_source<'a>(
    pricing: &'a BTreeMap<String, Price>,
    model: &str,
) -> Option<(&'a Price, &'static str)> {
    if let Some(price) = price_with_source(pricing, model) {
        return Some(price);
    }
    let (provider, id) = model.split_once('/')?;
    let (price, source) = if provider == "amazon-bedrock" {
        (bedrock_catalog_prices().get(id)?, CATALOG_BEDROCK_SOURCE)
    } else {
        if !CATALOG_ROWS
            .iter()
            .any(|(key, providers, ..)| *key == id && providers.contains(&provider))
        {
            return None;
        }
        (catalog_prices().get(id)?, catalog_source(id))
    };
    // A local override of the same verified bare model applies unless a more
    // specific qualified key or alias above matched the original report.
    Some(configured_price(pricing, id).map_or((price, source), |price| (price, "config")))
}

/// Collect invalid rates and conflicting aliases for config validators.
pub fn pricing_problems(pricing: &BTreeMap<String, Price>) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut aliases = BTreeMap::new();
    for (model, price) in pricing {
        for (field, rate) in [
            ("input", Some(price.input)),
            ("output", Some(price.output)),
            ("cache_read", price.cache_read),
            ("cache_write", price.cache_write),
        ] {
            if rate.is_some_and(|rate| !rate.is_finite() || rate < 0.0) {
                out.push((
                    format!("pricing.{model}.{field}"),
                    "must be finite and nonnegative".into(),
                ));
            }
        }
        for alias in &price.aliases {
            if let Some(owner) = aliases.insert(alias, model)
                && owner != model
            {
                out.push((
                    format!("pricing.{model}.aliases"),
                    format!("ambiguous alias {alias:?}, also used by {owner}"),
                ));
            }
            if alias != model && pricing.contains_key(alias) {
                out.push((
                    format!("pricing.{model}.aliases"),
                    format!("alias {alias:?} conflicts with an exact model ID"),
                ));
            }
        }
    }
    out
}

const ANTHROPIC: &[&str] = &["anthropic", "anthropic-api"];
const OPENAI: &[&str] = &["openai"];
const OPENAI_AND_CODEX: &[&str] = &["openai", "openai-codex"];

type CatalogRow = (&'static str, &'static [&'static str], f64, f64, f64, f64);

/// Frozen subset of installed omp and OmO catalogs inspected on 2026-10-15.
/// The GPT-6.2 rates come from Pi's supplemental model metadata. Sol Fast
/// rates include omp's 2x API priority premium, not subscription usage.
/// See `docs/pricing.md` for source paths, exact-ID equivalence, and tier limitations.
const CATALOG_ROWS: [CatalogRow; 24] = [
    ("claude-haiku-5-5", ANTHROPIC, 0.1, 0.5, 0.01, 0.125),
    ("claude-opus-4-6", ANTHROPIC, 5.0, 25.0, 0.5, 6.25),
    ("claude-opus-4-7", ANTHROPIC, 5.0, 25.0, 0.5, 6.25),
    ("claude-opus-4-8", ANTHROPIC, 5.0, 25.0, 0.5, 6.25),
    ("claude-opus-5-5", ANTHROPIC, 4.0, 20.0, 0.2, 5.0),
    ("claude-sonnet-4-6", ANTHROPIC, 3.0, 15.0, 0.3, 3.75),
    ("claude-sonnet-5", ANTHROPIC, 2.0, 10.0, 0.2, 2.5),
    ("claude-sonnet-5-5", ANTHROPIC, 2.0, 10.0, 0.1, 2.5),
    ("gpt-5.6", OPENAI, 4.0, 20.0, 0.4, 5.0),
    ("gpt-5.6-luna", OPENAI_AND_CODEX, 0.2, 1.2, 0.02, 0.25),
    ("gpt-5.6-luna-fast", OPENAI_AND_CODEX, 0.2, 1.2, 0.02, 0.25),
    ("gpt-5.6-sol", OPENAI_AND_CODEX, 4.0, 20.0, 0.4, 5.0),
    ("gpt-5.6-sol-fast", OPENAI_AND_CODEX, 8.0, 40.0, 0.8, 10.0),
    ("gpt-5.6-terra", OPENAI_AND_CODEX, 2.0, 12.0, 0.2, 2.5),
    ("gpt-6-astra", OPENAI_AND_CODEX, 10.0, 50.0, 1.0, 12.5),
    ("gpt-6-astra-fast", OPENAI_AND_CODEX, 10.0, 50.0, 1.0, 12.5),
    ("gpt-6-luna", OPENAI_AND_CODEX, 0.1, 0.5, 0.01, 0.125),
    ("gpt-6-luna-fast", OPENAI_AND_CODEX, 0.1, 0.5, 0.01, 0.125),
    ("gpt-6-sol", OPENAI_AND_CODEX, 2.0, 10.0, 0.2, 2.5),
    ("gpt-6-sol-fast", OPENAI_AND_CODEX, 4.0, 20.0, 0.4, 5.0),
    ("gpt-6.1-sol", OPENAI_AND_CODEX, 2.0, 10.0, 0.1, 2.5),
    ("gpt-6.1-sol-fast", OPENAI_AND_CODEX, 4.0, 20.0, 0.2, 5.0),
    ("gpt-6.2-astra", OPENAI_AND_CODEX, 8.0, 40.0, 0.4, 10.0),
    ("gpt-6.2-sol", OPENAI_AND_CODEX, 2.0, 10.0, 0.1, 2.5),
];

/// Build the editable starter table without adding any configured aliases.
fn starter_prices() -> BTreeMap<String, Price> {
    CATALOG_ROWS
        .into_iter()
        .map(|(model, _, input, output, cache_read, cache_write)| {
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
