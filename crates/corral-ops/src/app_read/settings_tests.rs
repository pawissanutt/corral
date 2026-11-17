use super::*;

#[test]
fn model_choices_keep_identical_models_on_distinct_billing_routes() {
    let mut config = Config::default_for(std::path::Path::new("/offline-fixture"));
    config.profiles.clear();
    config.assistant.agent = Some("api".into());
    for (name, connection) in [("api", Some("anthropic-api")), ("subscription", None)] {
        let profile = serde_json::from_value(serde_json::json!({
            "harness": "corral-pi", "model": "claude-sonnet-4-6",
            "vendor": "anthropic", "connection": connection,
        }))
        .unwrap();
        config.profiles.insert(name.into(), profile);
    }
    let choices = models(&config);
    assert_eq!(choices.len(), 2);
    assert_eq!(choices[0].name, "api");
    assert_eq!(choices[1].name, "subscription");
    assert_eq!(choices[0].model, choices[1].model);
    assert_eq!(
        choices[0].note.as_deref(),
        Some("Anthropic · API (metered)")
    );
    assert_eq!(choices[1].note.as_deref(), Some("Anthropic · subscription"));
    // Listing a profile without an approval is descriptive, not launch authorization.
    assert!(config.profiles["api"].metadata.is_none());
}
