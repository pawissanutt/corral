//! Incoming-main native plan-review launch must not silently become a legacy pane command.
use super::*;
use corral_core::agent::{AgentRole, ContainerRef, paths};
use corral_core::inbox::DeliveryMode;

#[test]
fn real_plan_review_service_binds_native_generation_and_bootstrap_before_submit() {
    let (tmp, mut ctx, id) = preview_fixture();
    let assets = tmp.path().join("native-review-assets");
    for (relative, text) in [
        ("package.json".to_owned(), "{\"name\":\"corral-pi\"}"),
        ("bun.lock".into(), "{}"),
        (
            "compat.json".into(),
            "{\"version\":1,\"verdict\":\"GO\",\"bun\":{\"range\":\"1.4.2\"},\"capabilities\":[\"role_enabled\"]}",
        ),
        ("fixture.json".into(), "{\"fixture\":true}"),
        (
            crate::agent::launch::HOST_ENTRY.into(),
            "// inert fixture host",
        ),
        (
            format!(
                "src/roles/{}/index.ts",
                AgentRole::PlanReviewer.module_dir()
            ),
            "export default {};",
        ),
        ("src/delivery/index.ts".into(), "export default {};"),
        ("src/auth/index.ts".into(), "export default {};"),
    ] {
        let path = assets.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    ctx.store.config.agent.assets_dir = Some(assets.clone());
    ctx.store.config.review.reviewer = Some("gpt-review".into());
    let profile = ctx.store.config.profiles.get_mut("gpt-review").unwrap();
    profile.harness = corral_core::CORRAL_PI_HARNESS.into();
    profile.model = Some("gpt-6.1-sol".into());
    profile.vendor = Some("openai".into());
    ctx.store.save_config().unwrap();
    let plan = ctx.store.get(&id).unwrap().plans.last().unwrap().clone();
    // This fake engine's only pane lives entirely in the disposable fixture.
    std::fs::write(tmp.path().join("engine/planner-live"), "").unwrap();
    let review = crate::plan_review(&ctx, &id, Some("gpt-review")).unwrap();
    assert_eq!(review.delivery, DeliveryMode::Inbox);
    let state = review
        .agent_host
        .as_ref()
        .expect("real native state in the complete return");
    assert_eq!(state.binding.role, AgentRole::PlanReviewer);
    assert_eq!(state.binding.generation, 1);
    assert!(state.current_generation().is_some());
    let container = ContainerRef::PlanReviewer {
        task: id.clone(),
        plan: plan.id.clone(),
        review: review.id.clone(),
    };
    let stored = ctx.store.head(&id).unwrap();
    let actual = stored.agent_generation(&container, 1).unwrap().unwrap();
    assert_eq!(actual.pane, review.herdr.as_ref().unwrap().pane_id);
    let command = std::fs::read_to_string(tmp.path().join("engine/launch-command")).unwrap();
    assert!(command.contains("agent launch --bootstrap"), "{command}");
    assert!(command.contains(&assets.display().to_string()));
    assert!(!command.contains("fixture-planner"));
    assert_eq!(
        ctx.store
            .get(&id)
            .unwrap()
            .plans
            .last()
            .unwrap()
            .reviews
            .last()
            .unwrap(),
        &review
    );
    let _ = std::fs::remove_dir_all(
        crate::agent::launch::runtime_root().join(paths::store_id(&ctx.store)),
    );
}
