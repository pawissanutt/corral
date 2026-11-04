use super::*;

#[test]
fn planning_force_is_not_persisted_as_reusable_queue_permission() {
    if isolated("planning::planning_force_is_not_persisted_as_reusable_queue_permission") {
        return;
    }
    let mut f = crate::submission_paths_tests::fixture::Fx::new();
    let id = f.task("One planning admission", Status::Ready).id;
    let mut probe = crate::resources::FixedResourceProbe::healthy();
    probe.0.load_per_core = f.ctx.store.config.resources.max_load_per_core * 2.0;
    probe.0.load_one = probe.0.load_per_core;
    f.ctx.resource_probe = std::sync::Arc::new(probe);
    crate::resources::observe(&f.ctx, corral_core::clock::now()).unwrap();
    let input = recover_resource_input(&f.ctx, RecoveryAdmissionStage::PlanDispatch).unwrap();
    let preview = recover_preview(&f.ctx, &id, &input).unwrap();
    assert!(
        preview.remaining_blockers.is_empty(),
        "{:?}",
        preview.remaining_blockers
    );
    let record = recover_apply(&f.ctx, &preview, &input, Some(confirmation(&preview))).unwrap();
    assert_eq!(
        state(&record).unwrap(),
        RecoveryState::Applied,
        "{:?}",
        record.outcomes
    );
    let task = f.ctx.store.get(&id).unwrap();
    assert_eq!(task.plan_runs.len(), 1);
    assert_eq!(task.plan_runs[0].id, "D1");
    let claim: serde_json::Value = serde_json::from_slice(
        &std::fs::read(task.plan_runs[0].output_dir.join("launch.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(claim["force_resources"], false);
    assert!(record.links.iter().any(|l| matches!(
        l.receipt,
        RecoveryReceipt::LaunchJournal {
            stage: RecoveryAdmissionStage::PlanDispatch,
            ..
        }
    )));
    assert_eq!(
        recover_apply(&f.ctx, &preview, &input, None).unwrap(),
        record
    );
    assert_eq!(f.ctx.store.get(&id).unwrap().plan_runs.len(), 1);
}
