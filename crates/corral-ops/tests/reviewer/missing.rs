use super::*;

#[test]
fn missing_reviewer_records_attempt_exit_and_allows_retry() {
    let (tmp, ctx, id, _) = task_in_review();
    std::fs::write(tmp.path().join("gone-test"), "").unwrap();
    corral_ops::review_run(&ctx, &id, Some("rev")).unwrap();
    ctx.store
        .update(&id, |task| {
            task.runs.last_mut().unwrap().autopilot = None;
            Ok(())
        })
        .unwrap();
    let task = ctx.store.get(&id).unwrap();
    let run = task.current_run().unwrap();
    let child = run.review_runs.last().unwrap();
    let receipt = ctx
        .store
        .state_dir()
        .join("launch-exits")
        .join(format!("{id}-{}-{}.exited", run.id, child.id));
    let _ = std::fs::remove_file(&receipt);
    let at = jiff::Timestamp::now();
    corral_ops::sync_at(&ctx, at).unwrap();
    std::fs::write(tmp.path().join("gone"), "").unwrap();
    corral_ops::sync_at(&ctx, at + jiff::SignedDuration::from_secs(1)).unwrap();
    assert!(
        ctx.store
            .get(&id)
            .unwrap()
            .current_run()
            .unwrap()
            .review
            .as_ref()
            .unwrap()
            .holds_slot()
    );
    corral_ops::sync_at(&ctx, at + jiff::SignedDuration::from_secs(2)).unwrap();
    assert_eq!(
        std::fs::read_to_string(&receipt).unwrap(),
        "unknown: pane disappeared\n"
    );
    let task = ctx.store.get(&id).unwrap();
    assert!(
        task.current_run()
            .unwrap()
            .review
            .as_ref()
            .unwrap()
            .process_lost
    );
    std::fs::remove_file(tmp.path().join("gone")).unwrap();
    // The review watchdog escalates to Needs me; reopening does not require a verdict.
    corral_ops::move_task(&ctx, &id, Status::Review, true).unwrap();
    corral_ops::review_run(&ctx, &id, Some("rev")).unwrap();
    assert_ne!(
        ctx.store
            .get(&id)
            .unwrap()
            .current_run()
            .unwrap()
            .review_runs
            .last()
            .unwrap()
            .id,
        child.id
    );
}
