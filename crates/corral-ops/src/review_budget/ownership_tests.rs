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
fn post_land_hooks_run_without_artifact_storage_or_the_inventory_lease() {
    // Given a landed operation whose hook needs both locks free, as a hook running corral would.
    let mut fx = Fixture::new(1);
    let locks = fx.ctx.store.dir().join("lock");
    let flock = |name: &str| {
        [
            "/usr/bin/flock".into(),
            "-n".into(),
            locks.join(name).display().to_string(),
        ]
    };
    let mut argv: Vec<String> = flock("artifact-storage.lock").into();
    argv.extend(flock("session-ownership.lock"));
    argv.push("/usr/bin/true".into());
    fx.ctx
        .store
        .config
        .projects
        .get_mut("app")
        .unwrap()
        .post_land = vec![corral_core::PostLandHook {
        name: "locks".into(),
        argv,
        cwd: ".".into(),
        env: std::collections::BTreeMap::new(),
        timeout_secs: 5,
        max_attempts: 1,
        identity: None,
    }];
    fx.ctx.store.save_config().unwrap();
    let id = fx.task();
    let task = fx.ctx.store.get(&id).unwrap();
    let app = fx.tmp.path().join("app");
    let head = git(&app, &["rev-parse", "HEAD"]);
    let before = "0".repeat(40);
    let run = task.current_run().unwrap();
    let mut op = crate::hooks::prepare(&fx.ctx, &task, run, &app, &before, &head)
        .unwrap()
        .unwrap();
    op.landed = true;
    crate::hooks::save(&fx.ctx, &op).unwrap();

    // When sync reconciles the operation.
    crate::sync_at(&fx.ctx, Timestamp::now()).unwrap();

    // Then the hook ran with neither lock held, so a corral command inside it cannot hang.
    let operations = crate::post_land_operations(&fx.ctx, Some("app")).unwrap();
    assert!(operations[0].ready(), "{:?}", operations[0].hooks[0]);
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
