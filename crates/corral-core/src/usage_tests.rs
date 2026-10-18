use super::*;
use crate::Price;
use std::path::Path;

fn config() -> Config {
    let mut config = Config::default_for(Path::new("/fixture"));
    config.pricing.clear();
    config.pricing.insert(
        "priced".into(),
        Price {
            input: 2.0,
            output: 10.0,
            cache_read: Some(0.5),
            cache_write: Some(3.0),
            aliases: vec!["alias".into()],
        },
    );
    config
}

#[test]
fn estimate_uses_disjoint_cache_categories_and_fallbacks() {
    // Given independent token categories and explicit cache prices.
    let config = config();
    let mut entry = UsageEntry {
        model: Some("alias".into()),
        input: 1_000_000,
        output: 100_000,
        cache_read: 200_000,
        cache_write: 300_000,
        ..UsageEntry::default()
    };
    // When repriced.
    entry.reprice(&config);
    // Then 2 + 1 + .1 + .9 = 4 USD, with no double-counting.
    assert!((entry.estimated_usd.unwrap() - 4.0).abs() < 1e-10);
    assert_eq!(entry.unpriced_tokens, 0);
    assert_eq!(entry.price_source.as_deref(), Some("config"));
}

#[test]
fn catalog_estimates_label_sources_and_unknown_models_clear_stale_sources() {
    // Given: a legacy config and response counters, with a stale source on an unknown model.
    let config: Config = toml::from_str("").unwrap();
    let mut entries = vec![
        UsageEntry {
            model: Some("gpt-6-astra".into()),
            input: 1_000_000,
            output: 100_000,
            cache_read: 200_000,
            cache_write: 300_000,
            ..UsageEntry::default()
        },
        UsageEntry {
            model: Some("unlisted".into()),
            input: 17,
            estimated_usd: Some(3.0),
            price_source: Some("config".into()),
            ..UsageEntry::default()
        },
    ];
    // When: recomputing using the catalog's four independent rates.
    for entry in &mut entries {
        entry.reprice(&config);
    }
    let usage = Usage::from_entries(entries);
    // Then: 10 + 5 + .2 + 3.75, and only the unknown model's tokens stay unpriced.
    assert!((usage.estimated_usd.unwrap() - 18.95).abs() < 1e-10);
    let json = serde_json::to_value(&usage).unwrap();
    assert_eq!(json["entries"][0]["price_source"], "catalog@2026-10-15");
    assert_eq!(json["unpriced_tokens"], 17);
    assert!(json["entries"][1]["estimated_usd"].is_null());
    assert!(json["entries"][1].get("price_source").is_none());
}

#[test]
fn omitted_cache_prices_fall_back_to_input() {
    // Given a model without cache overrides.
    let mut config = config();
    config.pricing.get_mut("priced").unwrap().cache_read = None;
    config.pricing.get_mut("priced").unwrap().cache_write = None;
    let mut usage = Usage {
        cache_read: 200_000,
        cache_write: 300_000,
        ..Usage::default()
    };
    // When legacy breakdowns are priced using the recorded run model.
    usage.reprice(&config, Some("priced"));
    // Then both cache categories use the input rate.
    assert!((usage.estimated_usd.unwrap() - 1.0).abs() < 1e-10);
}

#[test]
fn partial_pricing_retains_priced_sum_and_counts_unpriced_tokens() {
    // Given one priced response and one unknown model.
    let mut entries = vec![
        UsageEntry {
            model: Some("priced".into()),
            input: 1_000_000,
            reported_usd: Some(7.0),
            ..UsageEntry::default()
        },
        UsageEntry {
            model: Some("unknown".into()),
            input: 42,
            output: 8,
            ..UsageEntry::default()
        },
    ];
    // When each response is priced and the run is summed.
    for entry in &mut entries {
        entry.reprice(&config());
    }
    let usage = Usage::from_entries(entries);
    // Then missing prices don't destroy the priced estimate or imply a full reported bill.
    assert!((usage.estimated_usd.unwrap() - 2.0).abs() < 1e-10);
    assert_eq!(usage.unpriced_tokens, 50);
    assert_eq!(usage.reported_usd, None);
    assert_eq!(usage.cost_basis, Some(CostBasis::Mixed));
}

#[test]
fn task_total_includes_retries_and_every_reviewer_run() {
    // Given two dispatches and two reviewer sessions, one subscription-only.
    let make_run = |id: &str, reported: Option<f64>| {
        let mut run: Run = serde_json::from_value(serde_json::json!({
            "id":id, "agent":"omo", "worktree":"/fixture", "branch":"branch",
            "base":"main", "base_sha":"abc", "started":"2026-10-01T00:00:00Z"
        }))
        .unwrap();
        let mut usage = Usage {
            input: 1_000_000,
            cost_usd: reported,
            ..Usage::default()
        };
        usage.reprice(&config(), Some("priced"));
        run.usage = Some(usage);
        run
    };
    let mut first = make_run("R1", Some(3.0));
    first.review_runs = vec![
        make_run("R1-review1", Some(4.0)),
        make_run("R1-review2", None),
    ];
    let tmp = tempfile::tempdir().unwrap();
    let store = crate::Store::init(tmp.path()).unwrap();
    let mut task = store
        .create("Total", None, crate::Status::Done, "")
        .unwrap();
    task.runs = vec![first, make_run("R2", Some(5.0))];
    // When aggregating the task.
    let usage = task.total_usage().unwrap();
    // Then four sessions contribute exactly once and the basis is mixed.
    assert_eq!(usage.total_tokens(), 4_000_000);
    assert!((usage.estimated_usd.unwrap() - 8.0).abs() < 1e-10);
    assert_eq!(usage.reported_usd, None);
    assert_eq!(usage.cost_basis, Some(CostBasis::Mixed));
}
