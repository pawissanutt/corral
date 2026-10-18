use super::*;
use crate::{ConfigErrors, Store};
use tempfile::tempdir;

fn price() -> Price {
    Price {
        input: 2.0,
        output: 10.0,
        cache_read: None,
        cache_write: None,
        aliases: vec!["alternate".into()],
    }
}

#[test]
fn old_config_uses_catalog_without_installing_config_entries() {
    // Given / When: a legacy config without pricing is parsed.
    let config: Config = toml::from_str("").unwrap();
    // Then: catalog prices resolve without changing the editable table.
    assert!(config.pricing.is_empty());
    let (price, source) = config.price_with_source("gpt-6-astra").unwrap();
    assert!((price.input - 10.0).abs() < f64::EPSILON);
    assert_eq!(source, "catalog@2026-10-15");
    for unknown in [
        "unknown",
        "openai/gpt-6-astra",
        "GPT-6-astra",
        "gpt-6-astra-extra",
    ] {
        assert!(config.price(unknown).is_none());
    }
}

#[test]
fn config_exact_and_alias_both_override_catalog_rates() {
    // Given: two distinct overrides, one exact and one explicit alias.
    let mut config: Config = toml::from_str("").unwrap();
    let mut aliased = price();
    aliased.input = 7.0;
    aliased.output = 19.0;
    aliased.aliases = vec!["gpt-6-astra".into(), "claude-sonnet-5-5".into()];
    config.pricing.insert("custom".into(), aliased);
    let mut exact = price();
    exact.input = 99.0;
    exact.aliases.clear();
    config.pricing.insert("gpt-6-astra".into(), exact);
    // When / Then: exact config beats alias, and alias beats catalog.
    let (exact, source) = config.price_with_source("gpt-6-astra").unwrap();
    assert!((exact.input - 99.0).abs() < f64::EPSILON);
    assert_eq!(source, "config");
    let (alias, source) = config.price_with_source("claude-sonnet-5-5").unwrap();
    assert!((alias.input - 7.0).abs() < f64::EPSILON);
    assert!((alias.output - 19.0).abs() < f64::EPSILON);
    assert_eq!(source, "config");
}

#[test]
fn lookup_prefers_exact_keys_then_explicit_aliases() {
    // Given: an alias and an exact key with distinct prices.
    let dir = tempdir().unwrap();
    let mut config = Config::default_for(dir.path());
    config.pricing.insert("canonical".into(), price());
    let mut exact = price();
    exact.output = 20.0;
    exact.aliases.clear();
    config.pricing.insert("alternate".into(), exact);
    // When / Then: exact matches win even in an unvalidated config.
    assert!(std::ptr::eq(
        config.price("alternate").unwrap(),
        &config.pricing["alternate"]
    ));
    assert!(std::ptr::eq(
        config.price("canonical").unwrap(),
        &config.pricing["canonical"]
    ));
    assert!(config.price("Canonical").is_none());
    assert!(config.price("provider/canonical").is_none());
}

#[test]
fn alias_lookup_returns_the_canonical_price() {
    // Given: an explicit alias.
    let dir = tempdir().unwrap();
    let mut config = Config::default_for(dir.path());
    config.pricing.insert("canonical".into(), price());
    // When / Then: the alias borrows that entry.
    assert!(std::ptr::eq(
        config.price("alternate").unwrap(),
        &config.pricing["canonical"]
    ));
    assert!(config.price("unknown").is_none());
}

#[test]
fn absent_cache_rates_fall_back_independently_but_zero_does_not() {
    // Given: one absent and one explicitly zero cache rate.
    let mut configured = price();
    configured.cache_write = Some(0.0);
    // When / Then: absent uses input, explicit zero stays zero.
    assert!((configured.cache_read() - 2.0).abs() < f64::EPSILON);
    assert!(configured.cache_write().abs() < f64::EPSILON);
    let default = price();
    assert!((default.cache_write() - 2.0).abs() < f64::EPSILON);
}

#[test]
fn pricing_round_trips_optional_rates_and_aliases() {
    // Given: a price with an explicit cache read and an omitted cache write.
    let dir = tempdir().unwrap();
    let mut config = Config::default_for(dir.path());
    let mut configured = price();
    configured.cache_read = Some(0.1);
    config.pricing.insert("gpt-6.1-sol".into(), configured);
    // When: serialize and parse the config.
    let parsed: Config = toml::from_str(&config.to_toml()).unwrap();
    // Then: all pricing fields survive.
    assert_eq!(parsed.pricing, config.pricing);
    assert!(parsed.validate(dir.path()).is_ok());
}

#[test]
fn validation_rejects_each_invalid_rate() {
    // Given: each numeric field independently receives every invalid class.
    let dir = tempdir().unwrap();
    for rate in [-1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        for field in ["input", "output", "cache_read", "cache_write"] {
            let mut config = Config::default_for(dir.path());
            let mut configured = price();
            match field {
                "input" => configured.input = rate,
                "output" => configured.output = rate,
                "cache_read" => configured.cache_read = Some(rate),
                "cache_write" => configured.cache_write = Some(rate),
                _ => unreachable!(),
            }
            config.pricing.insert("model".into(), configured);
            // When: validate.
            let ConfigErrors(problems) = config.validate(dir.path()).unwrap_err();
            // Then: the precise invalid field is reported.
            assert_eq!(problems.len(), 1);
            assert!(problems[0].contains(&format!("pricing.model.{field}")));
        }
    }
}

#[test]
fn validation_accepts_explicit_zero_rates() {
    // Given: a genuinely free, explicitly configured model.
    let dir = tempdir().unwrap();
    let mut config = Config::default_for(dir.path());
    config.pricing.insert(
        "free".into(),
        Price {
            input: 0.0,
            output: 0.0,
            cache_read: Some(0.0),
            cache_write: Some(0.0),
            aliases: Vec::new(),
        },
    );
    // When / Then: zero is valid rather than unknown.
    assert!(config.validate(dir.path()).is_ok());
}

#[test]
fn validation_rejects_ambiguous_aliases_and_exact_key_collisions() {
    // Given: two models claim the same alias and one claims another model's key.
    let dir = tempdir().unwrap();
    let mut config = Config::default_for(dir.path());
    config.pricing.insert("first".into(), price());
    let mut second = price();
    second.aliases.push("first".into());
    config.pricing.insert("second".into(), second);
    // When: validate.
    let ConfigErrors(problems) = config.validate(dir.path()).unwrap_err();
    // Then: both ambiguities are rejected.
    assert_eq!(problems.len(), 2);
}

#[test]
fn new_init_seeds_verified_prices_and_validates() {
    // Given: a new workspace.
    let dir = tempdir().unwrap();
    // When: initialize it.
    let store = Store::init(dir.path()).unwrap();
    // Then: on-disk config contains the shipped API rates and validates.
    let loaded = Config::load(&store.config_path()).unwrap();
    assert_eq!(loaded.pricing, starter_prices());
    assert!(loaded.validate(dir.path()).is_ok());
    let opus = loaded.price("claude-opus-5-5").unwrap();
    assert!((opus.input - 4.0).abs() < f64::EPSILON);
    let sol = loaded.price("gpt-6.1-sol").unwrap();
    assert!((sol.input - 2.0).abs() < f64::EPSILON);
    assert!((sol.cache_write() - 2.5).abs() < f64::EPSILON);
    assert!(loaded.pricing.values().all(|p| p.input > 0.0));
}

#[test]
fn new_init_prices_sol_fast_at_api_priority_rates() {
    // Given: a newly initialized workspace, not an existing user-edited table.
    let dir = tempdir().unwrap();
    let store = Store::init(dir.path()).unwrap();
    // When: load the starter prices saved on disk.
    let loaded = Config::load(&store.config_path()).unwrap();
    // Then: every Sol Fast rate includes the verified 2x API priority premium.
    for (model, input, output, cache_read, cache_write) in [
        ("gpt-5.6-sol-fast", 8.0, 40.0, 0.8, 10.0),
        ("gpt-6-sol-fast", 4.0, 20.0, 0.4, 5.0),
        ("gpt-6.1-sol-fast", 4.0, 20.0, 0.2, 5.0),
    ] {
        let price = loaded.price(model).unwrap();
        for (actual, expected) in [
            (price.input, input),
            (price.output, output),
            (price.cache_read(), cache_read),
            (price.cache_write(), cache_write),
        ] {
            assert!((actual - expected).abs() < f64::EPSILON, "{model}");
        }
    }
}

#[test]
fn repeated_init_preserves_existing_config_bytes_without_seeding() {
    // Given: an existing config with no pricing.
    let dir = tempdir().unwrap();
    std::fs::create_dir(dir.path().join(".corral")).unwrap();
    let path = dir.path().join(".corral/corral.toml");
    let original = Config::default_for(dir.path()).to_toml();
    std::fs::write(&path, &original).unwrap();
    // When: initialize.
    let store = Store::init(dir.path()).unwrap();
    // Then: no defaults or comments were inserted.
    assert!(store.config.pricing.is_empty());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
}

#[test]
fn repeated_init_preserves_user_prices() {
    // Given: an edited existing rate.
    let dir = tempdir().unwrap();
    let mut store = Store::init(dir.path()).unwrap();
    store.config.pricing.get_mut("gpt-6.1-sol").unwrap().input = 7.0;
    store.save_config().unwrap();
    let original = std::fs::read_to_string(store.config_path()).unwrap();
    // When: initialize again.
    let reopened = Store::init(dir.path()).unwrap();
    // Then: user edits survive byte for byte.
    assert_eq!(
        std::fs::read_to_string(reopened.config_path()).unwrap(),
        original
    );
    assert!((reopened.config.price("gpt-6.1-sol").unwrap().input - 7.0).abs() < f64::EPSILON);
}

#[test]
fn invalid_pricing_edits_do_not_write_config() {
    // Given: a valid workspace.
    let dir = tempdir().unwrap();
    let store = Store::init(dir.path()).unwrap();
    let original = std::fs::read_to_string(store.config_path()).unwrap();
    // When: attempt to save a negative price through the real edit boundary.
    let result = store.edit_config(|doc| doc.set_str("pricing.\"gpt-6.1-sol\".input", "-1.0"));
    // Then: validation rejects the write without changing the file.
    assert!(result.is_err());
    assert_eq!(
        std::fs::read_to_string(store.config_path()).unwrap(),
        original
    );
}
