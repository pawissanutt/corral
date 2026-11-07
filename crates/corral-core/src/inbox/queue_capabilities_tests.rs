use super::*;
use crate::config::agent_capabilities::{AgentCapabilities, Preset};

#[test]
fn queue_control_defaults_overrides_and_host_negotiation_cover_every_role() {
    let supported = std::collections::BTreeSet::from(["queue_control".to_owned()]);
    let old_host = std::collections::BTreeSet::new();
    for role in [
        AgentRole::Worker,
        AgentRole::Reviewer,
        AgentRole::Pm,
        AgentRole::Assistant,
        AgentRole::Planner,
        AgentRole::PlanReviewer,
    ] {
        for (preset, expected) in [
            (Preset::Strict, false),
            (Preset::Standard, true),
            (Preset::Full, true),
        ] {
            let mut config = AgentCapabilities::default();
            config.set_preset(preset);
            assert_eq!(
                config
                    .resolve(role, Some(&supported))
                    .enabled("queue_control"),
                expected
            );
            assert!(
                !config
                    .resolve(role, Some(&old_host))
                    .enabled("queue_control")
            );
            config.set(role, "queue_control", !expected).unwrap();
            let explicit = config.resolve(role, Some(&supported));
            assert_eq!(explicit.enabled("queue_control"), !expected);
            assert_eq!(
                explicit.features["queue_control"].source,
                CapabilitySource::RoleOverride
            );
            config.set(role, "queue_control", true).unwrap();
            let unavailable = config.resolve(role, Some(&old_host));
            assert!(!unavailable.enabled("queue_control"));
            assert_eq!(
                unavailable.explicit_unavailable().unwrap().0,
                "queue_control"
            );
        }
    }
}

#[test]
fn catalog_nine_history_is_read_without_new_features_or_digest_rewrites() {
    let recorded = serde_json::json!({
        "catalog": 9, "role": "worker", "preset": "standard",
        "features": {"todo": {"enabled": true, "source": "preset"}},
        "digest": "immutable-catalog-nine"
    });
    let historical: CapabilitySnapshot = serde_json::from_value(recorded.clone()).unwrap();
    assert!(!historical.enabled("queue_control"));
    assert_eq!(serde_json::to_value(&historical).unwrap(), recorded);
    let mut invalid_old = historical;
    invalid_old.features.insert(
        "queue_control".into(),
        CapabilityState {
            enabled: true,
            source: CapabilitySource::Preset,
            unavailable: None,
            settings: CapabilitySettings::new(),
        },
    );
    assert!(!invalid_old.enabled("queue_control"));
    assert!(!invalid_old.enabled_keys().any(|key| key == "queue_control"));
    assert_eq!(invalid_old.digest, "immutable-catalog-nine");
}
