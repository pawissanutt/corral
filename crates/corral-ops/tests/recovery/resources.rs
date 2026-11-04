use super::*;
use std::sync::Arc;

fn pressure(ctx: &mut Ctx) {
    let mut probe = crate::resources::FixedResourceProbe::healthy();
    probe.0.load_per_core = ctx.store.config.resources.max_load_per_core * 2.0;
    probe.0.load_one = probe.0.load_per_core;
    ctx.resource_probe = Arc::new(probe);
}

#[test]
fn resource_episodes_are_compatible_and_force_is_one_stage_not_capacity() {
    if isolated("resources::resource_episodes_are_compatible_and_force_is_one_stage_not_capacity") {
        return;
    }
    let mut f = Fixture::new(false);
    f.ctx.store.config.limits.running = 8;
    f.ctx.store.save_config().unwrap();
    pressure(&mut f.ctx);
    // Legacy state has no episode field: new readers inspect it without migrating or granting.
    let legacy = crate::resources::read_status(&f.ctx).unwrap();
    corral_core::records::RecordArea::<ResourceState>::new("resources")
        .save(&f.ctx.store, "state", &legacy)
        .unwrap();
    let unbound = recover_resource_input(&f.ctx, RecoveryAdmissionStage::PlanReview).unwrap();
    assert!(
        recover_preview(&f.ctx, &f.id, &unbound)
            .unwrap()
            .remaining_blockers
            .iter()
            .any(|b| b.code == "pressure-episode")
    );
    crate::resources::observe(&f.ctx, corral_core::clock::now()).unwrap();
    let input = recover_resource_input(&f.ctx, RecoveryAdmissionStage::PlanReview).unwrap();
    let preview = recover_preview(&f.ctx, &f.id, &input).unwrap();
    assert!(
        preview.remaining_blockers.is_empty(),
        "{:?}",
        preview.remaining_blockers
    );
    assert!(recover_confirm(&f.ctx, &preview, &input, "Not piped authorization").is_err());
    // Sampling times and load magnitudes within the same pressure condition are diagnostic.
    let mut jitter = crate::resources::FixedResourceProbe::healthy();
    jitter.0.load_per_core = f.ctx.store.config.resources.max_load_per_core * 3.0;
    jitter.0.load_one = jitter.0.load_per_core;
    f.ctx.resource_probe = Arc::new(jitter);
    crate::resources::observe(&f.ctx, corral_core::clock::now()).unwrap();
    assert_eq!(
        recover_resource_input(&f.ctx, RecoveryAdmissionStage::PlanReview).unwrap(),
        input
    );
    assert_eq!(
        recover_preview(&f.ctx, &f.id, &input).unwrap().digest,
        preview.digest
    );
    // Old ResourceState readers ignore the additive episode field.
    assert!(
        corral_core::records::RecordArea::<ResourceState>::new("resources")
            .load(&f.ctx.store, "state")
            .unwrap()
            .unwrap()
            .held
    );
    let busy = f
        .ctx
        .store
        .create(
            "One occupied reviewer slot",
            Some("app"),
            Status::Review,
            "Slot fixture",
        )
        .unwrap();
    crate::plan_submit(&f.ctx, &busy.id, &f.source).unwrap();
    let busy = f.ctx.store.get(&busy.id).unwrap();
    let policy_digest = crate::plan::policy_digest(&f.ctx.store.config, &busy).unwrap();
    f.ctx
        .store
        .update(&busy.id, |task| {
            task.plans[0].reviews.push(PlanReview {
                id: "PR1".into(),
                reviewer: "gpt-sol".into(),
                policy_digest,
                verdict: None,
                notes: None,
                at: corral_core::clock::now(),
                herdr: None,
                pending_reply: None,
                route: None,
                cancelled: false,
                agent_host: None,
                delivery: Default::default(),
                inbox: Default::default(),
            });
            Ok(())
        })
        .unwrap();
    f.ctx.store.config.limits.review = 1;
    f.ctx.store.save_config().unwrap();
    assert!(
        recover_preview(&f.ctx, &f.id, &input)
            .unwrap()
            .remaining_blockers
            .iter()
            .any(|b| b.code == "review-capacity")
    );
    f.ctx.store.config.limits.review = 2;
    f.ctx.store.save_config().unwrap();
    // A new pressure episode cannot consume authorization from the prior one.
    f.ctx.resource_probe = crate::resources::FixedResourceProbe::arc();
    crate::resources::observe(&f.ctx, corral_core::clock::now()).unwrap();
    pressure(&mut f.ctx);
    crate::resources::observe(&f.ctx, corral_core::clock::now()).unwrap();
    let next = recover_resource_input(&f.ctx, RecoveryAdmissionStage::PlanReview).unwrap();
    assert_ne!(next, input);
    assert!(recover_apply(&f.ctx, &preview, &input, Some(confirmation(&preview))).is_err());
    assert!(records(&f.ctx, &f.id).unwrap().is_empty());
    let fresh = recover_preview(&f.ctx, &f.id, &next).unwrap();
    assert!(
        fresh.remaining_blockers.is_empty(),
        "{:?}",
        fresh.remaining_blockers
    );
    let record = recover_apply(&f.ctx, &fresh, &next, Some(confirmation(&fresh))).unwrap();
    assert_eq!(
        state(&record).unwrap(),
        RecoveryState::Applied,
        "{:?}",
        record.outcomes
    );
    assert_eq!(recover_apply(&f.ctx, &fresh, &next, None).unwrap(), record);
    assert_eq!(f.ctx.store.get(&f.id).unwrap().plans[0].reviews.len(), 1);
    // Resource force does not import the different-model recovery floor or waive review policy.
    assert_eq!(f.callback("PR1").verdict, Some(Verdict::Approve));
    assert!(crate::plan_review::accept_auto(&f.ctx, &f.id, "P1").is_err());
    assert!(f.ctx.store.get(&f.id).unwrap().plans[0].approval.is_none());
}
