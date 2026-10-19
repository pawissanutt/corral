use super::*;

fn automatic_task(fx: &mut Fixture) -> String {
    fx.ctx.store.config.review.autopilot = Some(true);
    fx.ctx.store.config.review.reviewer = Some("rev".into());
    fx.ctx.store.save_config().unwrap();
    let id = fx.task();
    auxiliary::grant(fx, std::slice::from_ref(&id));
    fx.ctx
        .store
        .update(&id, |task| {
            let run = task.current_run_mut().unwrap();
            run.outcome = Some(corral_core::RunOutcome::Done);
            run.ended = Some(Timestamp::now());
            Ok(())
        })
        .unwrap();
    id
}

#[test]
fn sync_releases_inventory_before_gate_and_reviewer_launches() {
    // Given an authorized completed worker and a usable sampled inventory.
    let mut fx = Fixture::new(2);
    let id = automatic_task(&mut fx);

    // When sync starts a gate, then consumes its real receipt on the next tick.
    crate::sync_at(&fx.ctx, Timestamp::now()).unwrap();
    let task = fx.ctx.store.get(&id).unwrap();
    let gate = task.current_run().unwrap().gate.as_ref().unwrap();
    assert_eq!(gate.result, GateResult::Running);
    assert_eq!(fx.created(), 1);
    let output = support::command("bash", fx.tmp.path())
        .arg(crate::gate::gate_paths(&fx.ctx, &id, "R1").script)
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    crate::sync_at(&fx.ctx, Timestamp::now()).unwrap();

    // Then both exclusive admissions succeeded after inventory consumption.
    let task = fx.ctx.store.get(&id).unwrap();
    let run = task.current_run().unwrap();
    assert_eq!(run.gate.as_ref().unwrap().result, GateResult::Pass);
    assert!(run.review.as_ref().unwrap().holds_slot());
    assert_eq!(run.review_runs.len(), 1);
    assert_eq!(fx.created(), 2);
    assert!(
        fx.ctx
            .store
            .try_lock("session-ownership")
            .unwrap()
            .is_some(),
        "sync leaked a sampled inventory lease"
    );
}

#[test]
fn sync_launch_still_excludes_an_external_inventory_consumer() {
    // Given another observer retaining its lease until explicitly released.
    let mut fx = Fixture::new(1);
    let id = automatic_task(&mut fx);
    let (ready, sampled) = mpsc::channel();
    let (release, released) = mpsc::channel();
    std::thread::scope(|scope| {
        let ctx = &fx.ctx;
        let observer = scope.spawn(move || {
            let inventory = crate::pane_liveness::inventory(ctx).unwrap();
            ready.send(()).unwrap();
            released.recv_timeout(Duration::from_secs(10)).unwrap();
            drop(inventory);
        });
        sampled.recv_timeout(Duration::from_secs(10)).unwrap();

        // When sync finishes its own sample but the external reader remains.
        crate::sync_at(ctx, Timestamp::now()).unwrap();
        let task = ctx.store.get(&id).unwrap();
        assert!(task.current_run().unwrap().gate.is_none());
        assert!(task.review_reservation.is_none());
        assert!(!journal(ctx).exists());
        assert_eq!(fx.created(), 0);
        release.send(()).unwrap();
        observer.join().unwrap();
    });

    // Then releasing that reader, not bypassing the lock, admits exactly one gate.
    crate::sync_at(&fx.ctx, Timestamp::now()).unwrap();
    assert!(
        fx.ctx
            .store
            .get(&id)
            .unwrap()
            .current_run()
            .unwrap()
            .gate
            .is_some()
    );
    assert_eq!(fx.created(), 1);
}
