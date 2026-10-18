use super::*;

#[derive(Clone, Copy)]
enum Pressure {
    Memory,
    Disk,
}

impl Pressure {
    fn apply(self, probe: &Probe, path: &Path) {
        let mut sample = probe.0.write().unwrap();
        match self {
            Self::Memory => sample.mem_available = 1,
            Self::Disk => {
                sample.disks = vec![DiskSpace {
                    path: path.into(),
                    free_bytes: 1,
                }];
            }
        }
    }
}

fn ready_fixture() -> (Fx, String) {
    let mut fx = Fx::new(|_| {});
    fx.ctx
        .store
        .update(&fx.id, |task| {
            task.status = Status::Dropped;
            Ok(())
        })
        .unwrap();
    fx.ctx.store.config.defaults.profile = Some("fake".into());
    fx.ctx.store.config.dispatch.ready_projects = vec!["app".into()];
    // One automatic coding slot plus the default two-slot review headroom.
    fx.ctx.store.config.limits.running = 3;
    fx.ctx.store.save_config().unwrap();
    let ready = fx
        .ctx
        .store
        .create("Authorized without enqueue", Some("app"), Status::Ready, "")
        .unwrap();
    fx.grant(std::slice::from_ref(&ready.id));
    (fx, ready.id)
}

fn assert_waiting(ctx: &Ctx, id: &str) {
    let entry = corral_ops::dispatch_queue(ctx)
        .unwrap()
        .into_iter()
        .find(|entry| entry.id == id)
        .expect("waiting work stays in the queue");
    assert!(
        entry.reason.starts_with("waiting: machine busy ("),
        "{entry:?}"
    );
}

#[test]
fn severe_attention_preserves_unqueued_ready_identity_and_force() {
    for pressure in [Pressure::Memory, Pressure::Disk] {
        // Given authorized Ready work, no Running tasks, and no available engine.
        let (mut fx, id) = ready_fixture();
        let online = fx.ctx.herdr.clone();
        fx.ctx.herdr = Herdr::with_bin(fx.bin_dir.join("missing-herdr"), None);
        let probe = Probe::install(&mut fx);
        pressure.apply(&probe, &fx.app);
        assert!(!fx.ctx.store.get(&id).unwrap().queued);
        // When pressure is reconciled before enqueue can run.
        fx.sync();
        fx.sync();
        // Then attention leaves the admission state and capacity unchanged.
        let ready = fx.ctx.store.get(&id).unwrap();
        assert_eq!(ready.status, Status::Ready);
        assert!(!ready.queued);
        assert!(ready.runs.is_empty() && ready.launch.is_none());
        assert_eq!(corral_ops::slot_usage(&fx.ctx).unwrap().total(), 0);
        assert_waiting(&fx.ctx, &id);
        assert_eq!(fx.events("escalated"), 1);

        // When the engine returns, only explicit force can start eligible work.
        fx.ctx.herdr = online;
        assert!(corral_ops::dispatch(&fx.ctx, &id, &DispatchOpts::default()).is_err());
        let outcome = corral_ops::dispatch(
            &fx.ctx,
            &id,
            &DispatchOpts {
                force_resources: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(matches!(outcome, DispatchOutcome::Started { .. }));
        let started = fx.ctx.store.get(&id).unwrap();
        assert_eq!(started.status, Status::Running);
        assert_eq!(started.runs.len(), 1);
        assert_eq!(fx.events("escalated"), 1);
    }
}

#[test]
fn severe_attention_releases_ready_admission_automatically() {
    for pressure in [Pressure::Memory, Pressure::Disk] {
        // Given project-authorized Ready work that has never been enqueued.
        let (mut fx, id) = ready_fixture();
        let probe = Probe::install(&mut fx);
        pressure.apply(&probe, &fx.app);
        let notified = fx.herdr_log("notify.log").lines().count();
        // When repeated syncs observe severe pressure.
        fx.sync();
        fx.sync();
        let held = fx.ctx.store.get(&id).unwrap();
        assert_eq!(held.status, Status::Ready);
        assert!(held.queued && held.runs.is_empty());
        assert_waiting(&fx.ctx, &id);
        assert_eq!(corral_ops::slot_usage(&fx.ctx).unwrap().total(), 0);
        assert_eq!(fx.events("escalated"), 1);
        assert_eq!(fx.herdr_log("notify.log").lines().count(), notified + 1);

        // Then a recovery sample starts the same authorization without owner repair.
        *probe.0.write().unwrap() = FixedResourceProbe::healthy().0;
        fx.sync();
        let started = fx.ctx.store.get(&id).unwrap();
        assert_eq!(started.status, Status::Running);
        assert_eq!(started.runs.len(), 1);
        assert_eq!(fx.events("resources_cleared"), 1);
    }
}

#[test]
fn severe_attention_preserves_pending_gate_identity_force_and_release() {
    for pressure in [Pressure::Memory, Pressure::Disk] {
        for force in [false, true] {
            // Given a completed run awaiting its first autopilot gate, with no Running work.
            let mut fx = Fx::new(|policy| {
                policy.gate = gate(&["true"]);
                policy.accept = Some(Accept::Owner);
            });
            fx.done();
            let before = fx.run();
            let probe = Probe::install(&mut fx);
            pressure.apply(&probe, &fx.app);
            let notified = fx.herdr_log("notify.log").lines().count();
            // When pressure persists across sync ticks.
            fx.sync();
            let held = fx.sync();
            assert_eq!(held.status, Status::Review);
            assert_eq!(held.review_wait, Some(ReviewWork::Gate));
            assert_eq!(fx.run().id, before.id);
            assert_eq!(fx.run().outcome, Some(RunOutcome::Done));
            assert!(fx.run().gate.is_none());
            assert_eq!(held.rounds, 0);
            assert_eq!(fx.run().failures, 0);
            assert_eq!(corral_ops::slot_usage(&fx.ctx).unwrap().total(), 0);
            assert_waiting(&fx.ctx, &fx.id);
            assert_eq!(fx.events("escalated"), 1);
            assert_eq!(fx.herdr_log("notify.log").lines().count(), notified + 1);

            // Then force can start the gate immediately, or recovery admits it automatically.
            if force {
                assert!(corral_ops::gate_start(&fx.ctx, &fx.id).is_err());
                corral_ops::gate_start_with_force(&fx.ctx, &fx.id, true).unwrap();
                corral_ops::gate_collect(&fx.ctx, &fx.id).unwrap();
            }
            *probe.0.write().unwrap() = FixedResourceProbe::healthy().0;
            let released = fx.sync();
            assert_eq!(released.status, Status::Review);
            assert_eq!(fx.stage(), Some(AutoStage::Reviewed));
            assert_eq!(fx.run().gate.unwrap().result, GateResult::Pass);
            assert_eq!(fx.run().id, before.id);
            assert_eq!(released.rounds, 0);
            assert_eq!(fx.events("resources_cleared"), 1);
        }
    }
}

#[test]
fn severe_attention_preserves_pending_reviewer_identity_and_release() {
    for pressure in [Pressure::Memory, Pressure::Disk] {
        // Given a completed run with a passing gate and no reviewer started.
        let mut fx = Fx::new(|policy| {
            policy.gate = gate(&["true"]);
            policy.reviewer = Some("rev".into());
            policy.accept = Some(Accept::Owner);
        });
        corral_ops::gate_start(&fx.ctx, &fx.id).unwrap();
        corral_ops::gate_collect(&fx.ctx, &fx.id).unwrap();
        fx.done();
        let before = fx.run();
        let probe = Probe::install(&mut fx);
        pressure.apply(&probe, &fx.app);
        let notified = fx.herdr_log("notify.log").lines().count();
        // When repeated syncs encounter pressure before reviewer admission.
        fx.sync();
        let held = fx.sync();
        assert_eq!(held.status, Status::Review);
        assert_eq!(held.review_wait, Some(ReviewWork::Reviewer));
        assert_eq!(fx.run().id, before.id);
        assert_eq!(fx.run().gate, before.gate);
        assert!(fx.run().review.is_none() && fx.run().review_runs.is_empty());
        assert_eq!(held.rounds, 0);
        assert_eq!(corral_ops::slot_usage(&fx.ctx).unwrap().total(), 0);
        assert_waiting(&fx.ctx, &fx.id);
        assert_eq!(fx.events("escalated"), 1);
        assert_eq!(fx.herdr_log("notify.log").lines().count(), notified + 1);

        // Then recovery starts exactly one reviewer on the same run.
        *probe.0.write().unwrap() = FixedResourceProbe::healthy().0;
        let released = fx.sync();
        assert_eq!(released.status, Status::Review);
        assert_eq!(fx.run().id, before.id);
        assert_eq!(fx.run().review.unwrap().reviewer, "rev");
        assert_eq!(fx.run().review_runs.len(), 1);
        assert_eq!(released.rounds, 0);
        assert_eq!(fx.events("resources_cleared"), 1);
    }
}

#[test]
fn severe_attention_does_not_override_an_existing_owner_question() {
    // Given a normal NeedsMe task, not resource-owned attention.
    let mut fx = Fx::new(|_| {});
    fx.ctx
        .store
        .update(&fx.id, |task| {
            task.status = Status::NeedsMe;
            task.current_run_mut().unwrap().blocked_excerpt = Some("owner question".into());
            Ok(())
        })
        .unwrap();
    let online = fx.ctx.herdr.clone();
    fx.ctx.herdr = Herdr::with_bin(fx.bin_dir.join("missing-herdr"), None);
    let probe = Probe::install(&mut fx);
    Pressure::Memory.apply(&probe, &fx.app);
    // When resource pressure is observed.
    fx.sync();
    // Then it cannot replace the question or make force bypass the task's state.
    assert_eq!(fx.task().status, Status::NeedsMe);
    assert_eq!(fx.run().blocked_excerpt.as_deref(), Some("owner question"));
    fx.ctx.herdr = online;
    assert!(
        corral_ops::dispatch(
            &fx.ctx,
            &fx.id,
            &DispatchOpts {
                force_resources: true,
                ..Default::default()
            }
        )
        .is_err(),
        "forcing must retain the owner question"
    );
}
