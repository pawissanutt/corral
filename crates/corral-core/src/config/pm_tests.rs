use super::*;

#[test]
fn cache_key_defaults_to_session_and_rejects_unknown_modes_or_fields() {
    let omitted: PmCacheKeyCfg = toml::from_str("").unwrap();
    assert_eq!(omitted.mode, PmCacheKeyMode::Session);
    for mode in ["session", "stable"] {
        let parsed: PmCacheKeyCfg = toml::from_str(&format!("mode = \"{mode}\"\n")).unwrap();
        assert_eq!(serde_json::to_value(parsed).unwrap()["mode"], mode);
    }
    assert!(toml::from_str::<PmCacheKeyCfg>("mode = \"automatic\"").is_err());
    assert!(toml::from_str::<PmCacheKeyCfg>("mode = \"stable\"\nsecret = \"no\"").is_err());
}
