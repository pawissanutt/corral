use corral_core::{Profile, ProfileChoice, RouteSource, Status, Store};
use corral_ops::{Ctx, route};

#[test]
fn routing_uses_enabled_authenticated_models_without_probing_harnesses() {
    // Given matching OMO rules and enabled models from two providers, only one authenticated.
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::init(dir.path()).unwrap();
    let agent = dir.path().join("agent");
    std::fs::create_dir(&agent).unwrap();
    std::fs::write(
        agent.join("settings.json"),
        r#"{"enabledModels":["anthropic-subscription/opus","chatgpt-subscription/sol"]}"#,
    )
    .unwrap();
    std::fs::write(
        agent.join("auth.json"),
        r#"{"chatgpt-subscription":{"type":"oauth","access":"fixture"}}"#,
    )
    .unwrap();
    store.config.sync.omo_agent_dir = Some(agent);
    for (name, model) in [
        ("opus", "anthropic-subscription/opus"),
        ("sol", "chatgpt-subscription/sol"),
    ] {
        store.config.profiles.insert(
            name.into(),
            Profile {
                harness: "omo".into(),
                model: Some(model.into()),
                effort: Some("high".into()),
                args: Vec::new(),
            },
        );
        store.config.routing.rules.push(corral_core::RoutingRule {
            name: None,
            project: Vec::new(),
            kind: Vec::new(),
            size: Vec::new(),
            title_regex: None,
            body_regex: None,
            profile: name.into(),
        });
    }
    let task = store
        .create("Automatic model", None, Status::Ready, "")
        .unwrap();
    let ctx = Ctx::from_store(store);

    // When routing without explicit overrides.
    let decision = route(&ctx, &task.id, &ProfileChoice::default()).unwrap();

    // Then the authenticated enabled model wins and rejected rules remain visible.
    assert_eq!(decision.profile, "sol");
    assert_eq!(
        decision.route,
        RouteSource::Rule {
            index: 2,
            name: None
        }
    );
    assert_eq!(decision.warnings.len(), 1);
    assert!(
        !ctx.store
            .state_dir()
            .join(corral_ops::HARNESS_CACHE_FILE)
            .exists()
    );
}
