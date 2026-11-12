use super::*;

#[test]
fn expert_is_pm_only_requires_its_model_and_is_absent_unless_enabled() {
    for preset in ["strict", "standard", "full"] {
        let config: AgentCapabilities = toml::from_str(&format!("preset = '{preset}'")).unwrap();
        let snapshot = config.resolve(AgentRole::Pm, None);
        assert!(!snapshot.enabled("expert"));
        assert!(!snapshot.features.contains_key("expert"));
    }
    for text in [
        "[pm]\nexpert = true",
        "[pm.expert]\nenabled = true",
        "[pm.expert]\nenabled = true\nmodel = ''",
        "[assistant.expert]\nenabled = true\nmodel = 'strong'",
        "[worker.expert]\nenabled = false",
    ] {
        assert!(toml::from_str::<AgentCapabilities>(text).is_err(), "{text}");
    }
    let config: AgentCapabilities =
        toml::from_str("[pm.expert]\nenabled = true\nmodel = 'strong'").unwrap();
    let snapshot = config.resolve(AgentRole::Pm, None);
    assert!(snapshot.enabled("expert"));
    let settings = &snapshot.features["expert"].settings;
    assert_eq!(settings["model"], "strong");
    assert_eq!(settings["max_turns"], 24);
    assert_eq!(settings["timeout_secs"], 600);
    assert_eq!(settings["max_parallel"], 1);
    let disabled: AgentCapabilities =
        toml::from_str("[pm.expert]\nenabled = false\nmodel = 'strong'").unwrap();
    assert_eq!(
        serde_json::to_value(disabled.resolve(AgentRole::Pm, None)).unwrap(),
        serde_json::to_value(AgentCapabilities::default().resolve(AgentRole::Pm, None)).unwrap()
    );
    assert!(
        !crate::agent::CapabilitySnapshot::legacy(AgentRole::Pm, false)
            .features
            .contains_key("expert")
    );
}

#[test]
fn expert_turn_timeout_and_parallel_bounds_are_validated_when_disabled_too() {
    for (key, maximum) in [
        ("max_turns", 64),
        ("timeout_secs", 600),
        ("max_parallel", 16),
    ] {
        for value in [0, maximum + 1] {
            let text = format!("[pm.expert]\nenabled = false\n{key} = {value}");
            assert!(
                toml::from_str::<AgentCapabilities>(&text).is_err(),
                "{text}"
            );
        }
        for value in [1, maximum] {
            let text = format!("[pm.expert]\n{key} = {value}");
            assert!(toml::from_str::<AgentCapabilities>(&text).is_ok(), "{text}");
        }
    }
}
