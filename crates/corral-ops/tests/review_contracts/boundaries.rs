//! R1 review regressions: callback isolation, pane teardown, stale stronger requests,
//! assessment freshness at verdict and landing, and review-context receipts.
use super::*;
use corral_core::{
    AutoProfileSelection, PROFILE_RECORD_VERSION, ProfileReviewSelection, SelectionActor,
};

fn closed_panes(fx: &Fx) -> Vec<String> {
    std::fs::read_to_string(fx._tmp.path().join("closed"))
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

fn reroute(fx: &Fx, id: &str, routine: &ReviewRecord) -> String {
    let outcome = corral_ops::request_stronger_review(
        &fx.ctx,
        id,
        &attempt(routine),
        "The lock ordering argument needs design-level analysis.",
    )
    .unwrap();
    let StrongerOutcome::Rerouted { superseded } = outcome else {
        panic!("expected reroute: {outcome:?}");
    };
    superseded
}

#[test]
fn superseded_routine_callback_cannot_record_on_the_strong_attempt() {
    let fx = fixture(|_| {});
    let id = candidate(&fx, "logic", &[], "src/widget.rs");
    let routine = corral_ops::review_run(&fx.ctx, &id, None).unwrap();
    let old_token = attempt(&routine);
    let gate_token = routine.identity.as_ref().unwrap().attempt.clone();
    reroute(&fx, &id, &routine);
    let strong = corral_ops::review_run(&fx.ctx, &id, None).unwrap();
    assert_eq!(strong.reviewer, "design");
    for token in [old_token, gate_token] {
        let error = corral_ops::verdict_bound(&fx.ctx, &id, Verdict::Approve, "ok", Some(&token))
            .unwrap_err();
        assert!(format!("{error:#}").contains("stale review"), "{error:#}");
    }
    assert_eq!(recorded(&fx, &id).verdict, None);
    let saved = corral_ops::verdict_bound(
        &fx.ctx,
        &id,
        Verdict::Changes,
        "fix",
        Some(&attempt(&strong)),
    )
    .unwrap();
    assert_eq!(saved.verdict, Some(Verdict::Changes));
}

#[test]
fn stronger_launch_closes_the_superseded_reviewer_pane_first() {
    let fx = fixture(|_| {});
    let id = candidate(&fx, "logic", &[], "src/widget.rs");
    let routine = corral_ops::review_run(&fx.ctx, &id, None).unwrap();
    let old_pane = routine.pane.clone().unwrap();
    reroute(&fx, &id, &routine);
    assert!(
        closed_panes(&fx).is_empty(),
        "the requester's own pane stays until the replacement launches"
    );
    let strong = corral_ops::review_run(&fx.ctx, &id, None).unwrap();
    assert_eq!(closed_panes(&fx), vec![old_pane]);
    assert!(strong.pane.is_some());
}

#[test]
fn stale_stronger_request_does_not_supersede_the_attempt() {
    let mut fx = fixture(|_| {});
    let id = candidate(&fx, "logic", &[], "src/widget.rs");
    let routine = corral_ops::review_run(&fx.ctx, &id, None).unwrap();
    fx.ctx
        .store
        .config
        .profiles
        .get_mut("impl")
        .and_then(|p| p.metadata.as_mut())
        .unwrap()
        .revision = "r2".into();
    let error = corral_ops::request_stronger_review(&fx.ctx, &id, &attempt(&routine), "deeper")
        .unwrap_err();
    assert!(format!("{error:#}").contains("stale review"), "{error:#}");
    let task = fx.ctx.store.get(&id).unwrap();
    let run = task.current_run().unwrap();
    assert_eq!(run.review_runs.len(), 1);
    assert_eq!(run.review_runs[0].outcome, None);
    assert_eq!(run.review.as_ref().unwrap().verdict, None);
}

#[test]
fn stronger_request_after_a_candidate_change_is_stale() {
    let fx = fixture(|_| {});
    let id = candidate(&fx, "logic", &[], "src/widget.rs");
    let routine = corral_ops::review_run(&fx.ctx, &id, None).unwrap();
    let worktree = fx
        .ctx
        .store
        .get(&id)
        .unwrap()
        .current_run()
        .unwrap()
        .worktree
        .clone();
    std::fs::write(worktree.join("src/extra.rs"), "late\n").unwrap();
    corral_git::commit_all(&worktree, "late change").unwrap();
    let error = corral_ops::request_stronger_review(&fx.ctx, &id, &attempt(&routine), "deeper")
        .unwrap_err();
    assert!(format!("{error:#}").contains("stale review"), "{error:#}");
    let run = fx
        .ctx
        .store
        .get(&id)
        .unwrap()
        .current_run()
        .unwrap()
        .clone();
    assert_eq!(run.review_runs[0].outcome, None);
}

#[test]
fn changed_auto_mapping_stales_a_pending_review() {
    let mut fx = fixture(|_| {});
    let id = candidate(&fx, "logic", &[], "src/widget.rs");
    corral_ops::review_run(&fx.ctx, &id, None).unwrap();
    fx.ctx
        .store
        .config
        .routing
        .review_profiles
        .insert(ReviewContractKind::Implementation, "design".into());
    let error = corral_ops::verdict(&fx.ctx, &id, Verdict::Approve, "ok").unwrap_err();
    let text = format!("{error:#}");
    assert!(
        text.contains("stale review") && text.contains("reviewer_selection"),
        "{text}"
    );
}

#[test]
fn risk_floor_change_after_approval_blocks_landing() {
    let fx = fixture(|_| {});
    let id = candidate(&fx, "logic", &[], "src/widget.rs");
    corral_ops::review_run(&fx.ctx, &id, None).unwrap();
    let approved = corral_ops::verdict(&fx.ctx, &id, Verdict::Approve, "ok").unwrap();
    assert_eq!(approved.verdict, Some(Verdict::Approve));
    fx.ctx
        .store
        .update(&id, |t| {
            t.labels.push("permissions".into());
            Ok(())
        })
        .unwrap();
    let landed = corral_ops::approve(
        &fx.ctx,
        &id,
        corral_ops::ApproveStrategy::FastForward,
        false,
        true,
    );
    assert!(
        landed.is_err(),
        "landed without the strong review: {landed:?}"
    );
    let main = fx._tmp.path().join("app");
    assert!(!main.join("src/widget.rs").exists());
}

fn custom_reviewer(config: &mut Config) {
    use ProfileRole::{ImplementationReview, PlanReview};
    let mut database = reviewer("opus", "openai", &[PlanReview, ImplementationReview]);
    if let Some(metadata) = database.metadata.as_mut() {
        metadata.guidance.use_when = vec!["Schema migrations dominate".into()];
    }
    config.profiles.insert("custom.database".into(), database);
}

#[test]
fn custom_profile_selected_by_a_review_receipt_launches_under_auto() {
    let fx = fixture(custom_reviewer);
    let id = candidate(&fx, "logic", &[], "src/migration.rs");
    let catalog =
        corral_ops::selection_catalog(&fx.ctx.store, &id, ProfileRole::ImplementationReview)
            .unwrap();
    let view = catalog.review.clone().expect("review context");
    let custom = &catalog.eligible["custom.database"];
    assert!(custom.profile.metadata.is_some());
    let receipt = AutoProfileSelection {
        version: PROFILE_RECORD_VERSION,
        id: "review-receipt-1".into(),
        task: id.clone(),
        actor: SelectionActor::Pm,
        selected_at: "2026-10-20T00:00:00Z".parse().unwrap(),
        role: ProfileRole::ImplementationReview,
        profile: "custom.database".into(),
        profile_revision: custom.profile_revision.clone(),
        spec_revision: catalog.spec_revision.clone(),
        catalog_revision: catalog.catalog_revision.clone(),
        reason: "The change is a schema migration".into(),
        closest_alternative: None,
        distinction: None,
        review: Some(ProfileReviewSelection {
            version: PROFILE_RECORD_VERSION,
            profile: "custom.database".into(),
            profile_revision: custom.profile_revision.clone(),
            contract: view.requirement.contract.clone(),
            subject: view.subject.clone(),
            policy: view.policy.clone(),
            reason: "Migration guidance fits".into(),
        }),
    };
    corral_ops::selection_save(&fx.ctx.store, &receipt).unwrap();
    let record = corral_ops::review_run(&fx.ctx, &id, None).unwrap();
    let route = route(&record);
    assert_eq!(record.reviewer, "custom.database");
    assert_eq!(route.profile_source, "auto_receipt");
    assert_eq!(route.receipt.as_deref(), Some("review-receipt-1"));
    assert_eq!(
        route.requirement.contract.kind,
        ReviewContractKind::Implementation
    );
}

#[test]
fn review_receipt_below_the_capability_floor_is_refused() {
    let fx = fixture(custom_reviewer);
    let id = candidate(&fx, "logic", &["permissions"], "src/auth.rs");
    let catalog =
        corral_ops::selection_catalog(&fx.ctx.store, &id, ProfileRole::ImplementationReview)
            .unwrap();
    let view = catalog.review.clone().unwrap();
    assert_eq!(view.requirement.capability, ReviewCapability::Strong);
    assert!(catalog.excluded.contains_key("impl"));
    assert!(catalog.eligible.contains_key("custom.database"));
}
