use super::*;

#[test]
fn shadow_is_pm_only_explicitly_modeled_and_off_in_every_preset() {
    for preset in ["strict", "standard", "full"] {
        let config: AgentCapabilities = toml::from_str(&format!("preset = '{preset}'")).unwrap();
        assert!(!config.resolve(AgentRole::Pm, None).enabled("shadow"));
        assert!(
            !config
                .resolve(AgentRole::Pm, None)
                .features
                .contains_key("shadow")
        );
    }
    for invalid in [
        "[pm]\nshadow = true",
        "[pm.shadow]\nenabled = true",
        "[pm.shadow]\nenabled = true\nmodel = ''",
        "[worker.shadow]\nenabled = true\nmodel = 'cheap'",
    ] {
        assert!(
            toml::from_str::<AgentCapabilities>(invalid).is_err(),
            "{invalid}"
        );
    }
    let config: AgentCapabilities =
        toml::from_str("[pm.shadow]\nenabled = true\nmodel = 'cheap'").unwrap();
    let snapshot = config.resolve(AgentRole::Pm, None);
    assert!(snapshot.enabled("shadow"));
    assert_eq!(snapshot.features["shadow"].settings["model"], "cheap");
    assert_eq!(snapshot.features["shadow"].settings["timeout_secs"], 60);
    assert_eq!(snapshot.features["shadow"].settings["max_per_hour"], 30);
    let disabled: AgentCapabilities =
        toml::from_str("[pm.shadow]\nenabled = false\nmodel = 'cheap'").unwrap();
    let defaults = AgentCapabilities::default().resolve(AgentRole::Pm, None);
    let disabled = disabled.resolve(AgentRole::Pm, None);
    assert_eq!(
        serde_json::to_value(&disabled).unwrap(),
        serde_json::to_value(&defaults).unwrap()
    );
    assert_eq!(disabled.digest, defaults.digest);
    assert!(
        !crate::agent::CapabilitySnapshot::legacy(AgentRole::Pm, false)
            .features
            .contains_key("shadow")
    );
}

#[test]
fn shadow_timeout_and_hourly_bounds_are_validated_even_when_disabled() {
    for key in ["timeout_secs", "max_per_hour"] {
        for value in [0, 601] {
            let text = format!("[pm.shadow]\nenabled = false\n{key} = {value}");
            assert!(
                toml::from_str::<AgentCapabilities>(&text).is_err(),
                "{text}"
            );
        }
        for value in [1, 600] {
            let text = format!("[pm.shadow]\n{key} = {value}");
            assert!(toml::from_str::<AgentCapabilities>(&text).is_ok(), "{text}");
        }
    }
}
