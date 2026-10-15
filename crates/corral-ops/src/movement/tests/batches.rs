use super::*;

#[test]
fn pending_batch_survives_restart_before_the_quiet_boundary() {
    // Given a pending batch persisted before the process restarts.
    let fx = Fx::new();
    fx.worker_change("file.rs");
    fx.land("file.rs", "fn landed() {}\n");
    fx.tick(0);
    let restarted = Ctx {
        store: Store::open(fx.ctx.store.root()).unwrap(),
        herdr: fx.ctx.herdr.clone(),
    };
    // When the restarted process reaches the original quiet boundary.
    reconcile(&restarted, fx.now + SignedDuration::from_secs(2), true).unwrap();
    // Then it delivers that batch once without resetting its debounce.
    assert_eq!(fx.sent().len(), 1);
    assert_eq!(fx.state().notices.len(), 1);
}

#[test]
fn competing_syncs_submit_a_mature_batch_once() {
    // Given one mature batch and two callers subscribed to the same start barrier.
    let fx = Fx::new();
    fx.worker_change("file.rs");
    fx.land("file.rs", "fn landed() {}\n");
    fx.tick(0);
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
    let workers: Vec<_> = (0..2)
        .map(|_| {
            let barrier = barrier.clone();
            let ctx = fx.ctx.clone();
            let now = fx.now + SignedDuration::from_secs(2);
            std::thread::spawn(move || {
                barrier.wait();
                reconcile(&ctx, now, true).unwrap();
            })
        })
        .collect();
    // When both syncs reach the per-task queue boundary together.
    barrier.wait();
    for worker in workers {
        worker.join().unwrap();
    }
    // Then only the queue-lock owner submits the batch.
    assert_eq!(fx.sent().len(), 1);
    assert_eq!(fx.state().notices.len(), 1);
}

#[test]
fn restored_path_still_has_headroom_when_overlap_is_declared_only() {
    // Given a declaration-only worker and two landed edits with zero net diff.
    let fx = Fx::new();
    fx.ctx
        .store
        .update(&fx.task.id, |t| {
            t.write_scope = vec!["file.rs".into()];
            Ok(())
        })
        .unwrap();
    fx.land("file.rs", "fn landed() {}\n");
    fx.tick(0);
    fx.land("file.rs", "fn initial() {}\n");
    fx.tick(1);
    // When the batch is constructed.
    fx.tick(3);
    // Then headroom exists for every overlapping Rust path, even outside the net packet diff.
    let state = fx.state();
    assert_eq!(state.notices[0].rust_headroom.len(), 1);
    assert_eq!(state.notices[0].rust_headroom[0].main, Some(799));
    assert_eq!(state.notices[0].rust_headroom[0].candidate, Some(799));
}

#[test]
fn unrelated_project_base_or_pre_dispatch_approvals_do_not_notify() {
    for mismatch in ["project", "base", "started"] {
        // Given an otherwise overlapping approval outside this run's integration window.
        let fx = Fx::new();
        fx.worker_change("file.rs");
        let landing = fx.land("file.rs", "fn landed() {}\n");
        let tasks = fx.ctx.store.list().unwrap();
        let mut events = fx.ctx.store.events().unwrap();
        let event = events.iter_mut().find(|e| e.kind == "approved").unwrap();
        match mismatch {
            "project" => event.detail["project"] = json!("another-project"),
            "base" => event.detail["base"] = json!("another-base"),
            "started" => event.ts = fx.now - SignedDuration::from_secs(1),
            _ => unreachable!(),
        }
        assert_eq!(event.detail["sha"], landing.sha);
        // When the explicit snapshots pass through reconciliation.
        reconcile_one(&fx.ctx, &fx.task, &events, &tasks, fx.now, true).unwrap();
        reconcile_one(
            &fx.ctx,
            &fx.task,
            &events,
            &tasks,
            fx.now + SignedDuration::from_secs(2),
            true,
        )
        .unwrap();
        // Then the worker is not interrupted.
        assert!(fx.sent().is_empty());
        assert!(fx.state().notices.is_empty());
    }
}

#[test]
fn changing_a_pane_on_the_same_run_does_not_replay_its_backlog() {
    // Given a pending notice for the original pane.
    let fx = Fx::new();
    fx.worker_change("file.rs");
    fx.land("file.rs", "fn landed() {}\n");
    fx.tick(0);
    fx.ctx
        .store
        .update(&fx.task.id, |t| {
            t.current_run_mut().unwrap().herdr.as_mut().unwrap().pane_id = "w1:p2".into();
            Ok(())
        })
        .unwrap();
    // When sync observes a replacement pane under the same run ID.
    fx.tick(2);
    fx.tick(4);
    // Then the old batch cannot be rebuilt for that replacement.
    assert!(fx.sent().is_empty());
    assert!(fx.state().pending.is_empty());
    assert!(fx.state().notices.is_empty());
}

#[test]
fn uncommitted_symlink_overlap_never_reads_its_unbounded_target() {
    // Given an untracked symlink to an unbounded device, also newly added on main.
    let fx = Fx::new();
    std::os::unix::fs::symlink("/dev/zero", fx.worker.join("new.rs")).unwrap();
    let landing = fx.land("new.rs", "fn landed() {}\n");
    // When observing overlap and constructing committed-blob context.
    let state = fx.queued(landing);
    // Then the path is observed without counting or following its uncommitted contents.
    assert_eq!(state.notices[0].observed_paths, ["new.rs"]);
    assert_eq!(state.notices[0].overlapping_paths, ["new.rs"]);
    assert_eq!(state.notices[0].rust_headroom[0].candidate, None);
}
