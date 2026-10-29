use super::*;

#[test]
fn headless_is_an_explicit_worker_or_reviewer_choice_in_every_preset() {
    // Given each preset with no override.
    for preset in [Preset::Strict, Preset::Standard, Preset::Full] {
        let mut config = AgentCapabilities::default();
        config.set_preset(preset);
        // Then no preset turns headless on, and only worker and reviewer may carry it.
        for role in [AgentRole::Worker, AgentRole::Reviewer] {
            let snapshot = config.resolve(role, None);
            assert!(!snapshot.enabled("headless"));
            assert_eq!(
                snapshot.features["headless"].source,
                CapabilitySource::Preset
            );
        }
        for role in [AgentRole::Pm, AgentRole::Planner, AgentRole::PlanReviewer] {
            assert!(!config.resolve(role, None).features.contains_key("headless"));
        }
    }
    // When the owner opts the worker and reviewer in.
    let parsed: crate::config::AgentCfg = toml::from_str(
        "[capabilities.worker]\nheadless = true\n[capabilities.reviewer]\nheadless = true",
    )
    .unwrap();
    // Then both resolve it as a role override; the PM table refuses the key.
    for role in [AgentRole::Worker, AgentRole::Reviewer] {
        let snapshot = parsed.capabilities.resolve(role, None);
        assert!(snapshot.enabled("headless"));
        assert_eq!(
            snapshot.features["headless"].source,
            CapabilitySource::RoleOverride
        );
    }
    let refused = toml::from_str::<crate::config::AgentCfg>("[capabilities.pm]\nheadless = true")
        .unwrap_err();
    assert!(refused.to_string().contains("not available to the pm role"));
}
