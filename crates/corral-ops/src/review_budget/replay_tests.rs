use super::*;

#[test]
fn cached_missing_inventory_cannot_erase_a_concurrent_reviewer_sighting() {
    // Given an observed reviewer occupying the fixture's only review slot.
    let fx = Fixture::new(1);
    let id = fx.task();
    let review = crate::reviewer::review_run(&fx.ctx, &id, Some("rev")).unwrap();
    let pane = review.pane.as_ref().unwrap();
    let present = fx.inventory();
    assert!(
        crate::reviewer::reconcile_review_at(&fx.ctx, &id, review.at, &present)
            .unwrap()
            .is_none()
    );
    std::fs::remove_file(fx.tmp.path().join(pane)).unwrap();
    let inventory = crate::pane_liveness::inventory(&fx.ctx).unwrap();
    let missing = inventory
        .panes
        .iter()
        .cloned()
        .map(|pane| (pane.pane_id.clone(), pane))
        .collect();
    let snapshot = inventory.snapshot(&missing);
    let ctx_a = fx.ctx.clone();
    let id_a = id.clone();
    let t1 = review.at + jiff::SignedDuration::from_secs(1);
    let t2 = review.at + jiff::SignedDuration::from_secs(2);
    let t3 = review.at + jiff::SignedDuration::from_secs(3);

    // When B records presence between A's watchdog and cached budget replay.
    std::thread::scope(|scope| {
        let (sampled_tx, sampled_rx) = mpsc::sync_channel(0);
        let (resume_tx, resume_rx) = mpsc::sync_channel(0);
        let a = scope.spawn(move || {
            assert!(
                crate::reviewer::reconcile_review_sampled_at(&ctx_a, &id_a, t1, snapshot)
                    .unwrap()
                    .is_none()
            );
            sampled_tx.send(()).unwrap();
            resume_rx.recv_timeout(Duration::from_secs(10)).unwrap();
            crate::review_budget::reconcile(&ctx_a, snapshot, t1).unwrap();
        });
        sampled_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        std::fs::write(fx.tmp.path().join(pane), "").unwrap();
        let present = fx.inventory();
        assert!(
            crate::reviewer::reconcile_review_at(&fx.ctx, &id, t2, &present)
                .unwrap()
                .is_none()
        );
        resume_tx.send(()).unwrap();
        a.join().unwrap();
    });
    drop(inventory);
    std::fs::remove_file(fx.tmp.path().join(pane)).unwrap();
    let missing = fx.inventory();
    crate::reviewer::reconcile_review_at(&fx.ctx, &id, t3, &missing).unwrap();

    // Then C's single fresh miss neither closes the review nor releases its slot.
    let task = fx.ctx.store.get(&id).unwrap();
    let retained = task.current_run().unwrap().review.as_ref().unwrap();
    assert_eq!(
        (
            task.status,
            retained.holds_slot(),
            retained.verdict.as_ref()
        ),
        (Status::Review, true, None)
    );
}
