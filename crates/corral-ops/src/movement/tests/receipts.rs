use super::*;

#[test]
fn missing_pane_defers_with_a_visible_receipt() {
    // Given an overlapping batch and a missing pane.
    let fx = Fx::new();
    fx.worker_change("file.rs");
    fx.land("file.rs", "fn landed() {}\n");
    std::fs::write(fx.bin.join("missing"), "").unwrap();
    // When delivery is attempted.
    fx.tick(0);
    fx.tick(2);
    fx.tick(3);
    // Then one durable deferred receipt is visible without repeated notifications.
    assert!(fx.sent().is_empty());
    assert_eq!(fx.state().notices[0].state, DeliveryState::Deferred);
    assert_eq!(fx.receipts().len(), 1);
    assert_eq!(fx.receipts()[0].detail["state"], "deferred");
    assert!(fx.receipts()[0].detail["receipt"].as_str().is_some());
}

#[test]
fn stuck_worker_retains_pending_answer_and_later_receives_the_same_notice() {
    // Given a stuck worker whose answer is already queued.
    let fx = Fx::new();
    fx.worker_change("file.rs");
    fx.land("file.rs", "fn landed() {}\n");
    fx.ctx
        .store
        .update(&fx.task.id, |t| {
            let r = t.current_run_mut().unwrap();
            r.pending_reply = Some("owner answer".into());
            r.stuck = Some(corral_core::StuckProcess {
                pid: 42,
                started: "identity".into(),
                argv: vec!["command".into()],
            });
            Ok(())
        })
        .unwrap();
    let before = fx.ctx.store.get(&fx.task.id).unwrap();
    // When the informational queue tries to deliver.
    fx.tick(0);
    fx.tick(2);
    // Then the answer is untouched and the notice has its own deferred receipt.
    let state = fx.state();
    assert_eq!(state.notices[0].state, DeliveryState::Deferred);
    assert_eq!(fx.ctx.store.get(&fx.task.id).unwrap().runs, before.runs);
    assert!(fx.sent().is_empty());
    // Given recovery and successful answer submission handled elsewhere.
    fx.ctx
        .store
        .update(&fx.task.id, |t| {
            let r = t.current_run_mut().unwrap();
            r.pending_reply = None;
            r.stuck = None;
            Ok(())
        })
        .unwrap();
    // When the queue is reconciled again, then that same notice is submitted once.
    fx.tick(3);
    assert_eq!(fx.state().notices[0].id, state.notices[0].id);
    assert_eq!(fx.state().notices[0].state, DeliveryState::Delivered);
    assert_eq!(fx.sent().len(), 1);
}

#[test]
fn pending_reply_and_owner_wait_each_defer_without_status_reset() {
    for pending_reply in [true, false] {
        // Given an owner-wait or an existing undelivered reply.
        let fx = Fx::new();
        fx.worker_change("file.rs");
        fx.land("file.rs", "fn landed() {}\n");
        fx.ctx
            .store
            .update(&fx.task.id, |t| {
                if pending_reply {
                    t.current_run_mut().unwrap().pending_reply = Some("answer".into());
                } else {
                    t.status = Status::NeedsMe;
                }
                Ok(())
            })
            .unwrap();
        let before = fx.ctx.store.get(&fx.task.id).unwrap();
        // When the notice is due.
        fx.tick(0);
        fx.tick(2);
        // Then neither owner input nor the state of the task is consumed by the notice.
        let after = fx.ctx.store.get(&fx.task.id).unwrap();
        assert_eq!(after.runs, before.runs);
        assert_eq!(after.status, before.status);
        assert_eq!(after.rounds, before.rounds);
        assert_eq!(fx.state().notices[0].state, DeliveryState::Deferred);
        assert!(fx.sent().is_empty());
    }
}

#[test]
fn transport_failure_persists_delivery_evidence_and_retries_once() {
    // Given a transport refusal, not an acknowledged submission.
    let fx = Fx::new();
    fx.worker_change("file.rs");
    fx.land("file.rs", "fn landed() {}\n");
    std::fs::write(fx.bin.join("fail"), "").unwrap();
    fx.tick(0);
    // When the first submission fails.
    fx.tick(2);
    // Then the failed receipt retains the actual delivery metadata.
    let failed = fx.state();
    assert_eq!(failed.notices[0].state, DeliveryState::Failed);
    let delivery = failed.notices[0].delivery.as_ref().unwrap();
    assert!(!delivery.submitted);
    assert_eq!(delivery.attempts, 1);
    assert!(delivery.error.is_some());
    assert_eq!(fx.receipts().last().unwrap().detail["state"], "failed");
    // Given the transport is restored.
    std::fs::remove_file(fx.bin.join("fail")).unwrap();
    // When retried, then the same notice is acknowledged once.
    fx.tick(3);
    fx.tick(4);
    assert_eq!(fx.sent().len(), 1);
    assert_eq!(fx.state().notices[0].id, failed.notices[0].id);
    assert!(fx.state().notices[0].delivery.as_ref().unwrap().submitted);
}

#[test]
fn interrupted_submission_is_uncertain_and_is_never_automatically_replayed() {
    // Given the persisted pre-send boundary, with or without an actual prior submission.
    for prior_submission in [false, true] {
        let fx = Fx::new();
        fx.worker_change("file.rs");
        let landing = fx.land("file.rs", "fn landed() {}\n");
        let mut state = fx.queued(landing);
        let path = state_path(&fx.ctx, &fx.task.id);
        if prior_submission {
            delivery::deliver(&fx.ctx, &path, &mut state, 0, true).unwrap();
        }
        state.notices[0].state = DeliveryState::Sending;
        save(&path, &state).unwrap();
        // When a new process representation sees the interrupted boundary.
        delivery::deliver(&fx.ctx, &path, &mut state, 0, true).unwrap();
        delivery::deliver(&fx.ctx, &path, &mut state, 0, true).unwrap();
        // Then uncertainty is visible, and it cannot duplicate a possible prior delivery.
        assert_eq!(state.notices[0].state, DeliveryState::Uncertain);
        assert_eq!(fx.sent().len(), usize::from(prior_submission));
    }
}

#[test]
fn task_finishing_during_construction_cancels_before_any_send() {
    // Given a notice constructed from a live snapshot.
    let fx = Fx::new();
    fx.worker_change("file.rs");
    let landing = fx.land("file.rs", "fn landed() {}\n");
    let mut state = fx.queued(landing);
    // Given done reaches its record transition before delivery acquires the task lock.
    crate::done(
        &fx.ctx,
        &fx.task.id,
        "Verified: fixture\nNot verified: none",
    )
    .unwrap();
    let before = fx.ctx.store.get(&fx.task.id).unwrap();
    // When the stale constructed notice reaches the external delivery boundary.
    delivery::deliver(
        &fx.ctx,
        &state_path(&fx.ctx, &fx.task.id),
        &mut state,
        0,
        true,
    )
    .unwrap();
    // Then no prompt or state reset affects the finished run.
    assert_eq!(state.notices[0].state, DeliveryState::Cancelled);
    assert!(fx.sent().is_empty());
    assert_eq!(fx.ctx.store.get(&fx.task.id).unwrap(), before);
}

#[test]
fn changed_run_or_recorded_pane_cannot_receive_a_stale_notice() {
    for replace_run in [true, false] {
        // Given a constructed notice whose task is redirected before delivery.
        let fx = Fx::new();
        fx.worker_change("file.rs");
        let landing = fx.land("file.rs", "fn landed() {}\n");
        let mut state = fx.queued(landing);
        fx.ctx
            .store
            .update(&fx.task.id, |t| {
                let r = t.current_run_mut().unwrap();
                if replace_run {
                    r.id = "R2".into();
                } else {
                    r.herdr.as_mut().unwrap().pane_id = "w1:p2".into();
                }
                Ok(())
            })
            .unwrap();
        // When the stale notice is reconciled.
        delivery::deliver(
            &fx.ctx,
            &state_path(&fx.ctx, &fx.task.id),
            &mut state,
            0,
            true,
        )
        .unwrap();
        // Then a replacement task or pane never receives it.
        assert_eq!(state.notices[0].state, DeliveryState::Cancelled);
        assert!(fx.sent().is_empty());
    }
}

#[test]
fn unrelated_pane_workspace_or_cwd_cancels_delivery() {
    for marker in ["workspace", "cwd"] {
        // Given the same pane ID now refers to an unrelated surface.
        let fx = Fx::new();
        fx.worker_change("file.rs");
        let landing = fx.land("file.rs", "fn landed() {}\n");
        let mut state = fx.queued(landing);
        std::fs::write(fx.bin.join(marker), "unrelated").unwrap();
        // When actual pane identity is reconciled.
        delivery::deliver(
            &fx.ctx,
            &state_path(&fx.ctx, &fx.task.id),
            &mut state,
            0,
            true,
        )
        .unwrap();
        // Then the notice is not sent to that pane.
        assert_eq!(state.notices[0].state, DeliveryState::Cancelled);
        assert!(fx.sent().is_empty());
    }
}

#[test]
fn replaced_shell_after_a_failed_submission_is_not_a_retry_target() {
    // Given an attempt pinned to a shell whose submission failed.
    let fx = Fx::new();
    fx.worker_change("file.rs");
    let landing = fx.land("file.rs", "fn landed() {}\n");
    let mut state = fx.queued(landing);
    let path = state_path(&fx.ctx, &fx.task.id);
    std::fs::write(fx.bin.join("fail"), "").unwrap();
    delivery::deliver(&fx.ctx, &path, &mut state, 0, true).unwrap();
    std::fs::remove_file(fx.bin.join("fail")).unwrap();
    std::fs::write(fx.bin.join("pid"), "5678").unwrap();
    // When a retry reconciles the live shell.
    delivery::deliver(&fx.ctx, &path, &mut state, 0, true).unwrap();
    // Then it cannot redirect the notice to the replacement process.
    assert_eq!(state.notices[0].state, DeliveryState::Cancelled);
    assert!(fx.sent().is_empty());
}

#[test]
fn unavailable_herdr_retains_the_notice_until_reconnection() {
    // Given a due overlap while the terminal engine is unavailable.
    let fx = Fx::new();
    fx.worker_change("file.rs");
    fx.land("file.rs", "fn landed() {}\n");
    reconcile(&fx.ctx, fx.now, false).unwrap();
    // When the quiet boundary is processed without terminal availability.
    reconcile(&fx.ctx, fx.now + SignedDuration::from_secs(2), false).unwrap();
    // Then deferred state survives and is submitted upon reconnection.
    assert_eq!(fx.state().notices[0].state, DeliveryState::Deferred);
    assert!(fx.sent().is_empty());
    fx.tick(3);
    assert_eq!(fx.state().notices[0].state, DeliveryState::Delivered);
}

#[test]
fn delivery_lock_excludes_a_concurrent_reply_and_preserves_the_notice() {
    // Given a reply holding the existing per-task delivery lock.
    let fx = Fx::new();
    fx.worker_change("file.rs");
    let landing = fx.land("file.rs", "fn landed() {}\n");
    let mut state = fx.queued(landing);
    let path = state_path(&fx.ctx, &fx.task.id);
    let lock = fx
        .ctx
        .store
        .try_lock(&format!("delivery-{}", fx.task.id))
        .unwrap()
        .unwrap();
    // When the notice reaches the same delivery boundary.
    delivery::deliver(&fx.ctx, &path, &mut state, 0, true).unwrap();
    // Then it defers without touching the transport.
    assert_eq!(state.notices[0].state, DeliveryState::Deferred);
    assert!(fx.sent().is_empty());
    drop(lock);
    delivery::deliver(&fx.ctx, &path, &mut state, 0, true).unwrap();
    assert_eq!(fx.sent().len(), 1);
}
