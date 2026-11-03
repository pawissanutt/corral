use super::*;

#[test]
fn preemption_leaves_the_victims_reviewer_running() {
    let mut fx = Fixture::new(2);
    fx.ctx.store.config.limits.preempt = Some("P0".into());
    fx.ctx.store.save_config().unwrap();
    let victim = fx.task();
    crate::gate::gate_start(&fx.ctx, &victim).unwrap();
    let review = crate::reviewer::review_run(&fx.ctx, &victim, Some("rev")).unwrap();
    let urgent = fx.task();
    fx.ctx
        .store
        .update(&urgent, |t| {
            t.priority = "P0".into();
            Ok(())
        })
        .unwrap();
    crate::gate::gate_start(&fx.ctx, &urgent).unwrap();
    let task = fx.ctx.store.get(&victim).unwrap();
    let run = task.current_run().unwrap();
    assert_eq!(run.gate.as_ref().unwrap().result, GateResult::Cancelled);
    assert_eq!(run.review.as_ref(), Some(&review));
    assert!(run.review.as_ref().unwrap().holds_slot());
    assert!(fx.tmp.path().join(review.pane.unwrap()).exists());
    assert_eq!(task.rounds, 0);
}
