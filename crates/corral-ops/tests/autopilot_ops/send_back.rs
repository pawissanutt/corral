use super::*;

#[test]
fn failed_send_back_keeps_review_and_pending_request_without_spending_two_rounds() {
    // Given a live worker whose native prompt transport rejects a review send-back.
    let fx = Fx::new(|r| {
        r.gate = gate(&["true"]);
        r.reviewer = Some("rev".into());
        r.accept = Some(Accept::Auto);
    });
    fx.change("widget.txt", "candidate\n");
    fx.done();
    assert_eq!(fx.sync().status, Status::Review);
    fx.verdict(Verdict::Changes, "Repair the widget");
    let original = fx.run();
    assert_eq!(fx.stage(), Some(AutoStage::Reviewing));
    let failure = fx.bin_dir.join("prompt-error");
    std::fs::write(&failure, "").unwrap();

    // When the first send fails and a later sync sees the same pending request.
    let task = fx.sync();

    // Then failure rolls back the completed run and leaves its request unconsumed.
    assert_eq!(task.status, Status::Review, "{}", fx.reason());
    assert_eq!(task.rounds, 0);
    assert_eq!(fx.stage(), Some(AutoStage::Reviewing));
    let run = fx.run();
    assert_eq!(run.id, original.id);
    assert_eq!(run.outcome, original.outcome);
    assert_eq!(run.ended, original.ended);
    assert_eq!(run.summary, original.summary);
    let pending = run.pending_reply.unwrap();
    let events = fx.ctx.store.events().unwrap();
    let delivery = events
        .iter()
        .find(|event| event.kind == "changes_requested")
        .unwrap();
    assert_eq!(delivery.detail["submitted"], false);
    assert_eq!(delivery.detail["via"], "agent_prompt");
    assert_eq!(delivery.detail["text"].as_str(), Some(pending.as_str()));
    assert_eq!(fx.herdr_log("prompts.log"), format!("{pending}\n"));

    let task = fx.sync();
    assert_eq!(task.status, Status::Review);
    assert_eq!(task.rounds, 0);
    assert_eq!(fx.stage(), Some(AutoStage::Reviewing));
    assert_eq!(fx.run().pending_reply.as_deref(), Some(pending.as_str()));

    // Transport recovery alone does not bypass T-144's owner-disposition boundary.
    std::fs::remove_file(failure).unwrap();
    let task = fx.sync();
    assert_eq!(task.status, Status::Review);
    assert_eq!(task.rounds, 0);
    assert_eq!(fx.stage(), Some(AutoStage::Reviewing));
    assert_eq!(fx.run().pending_reply.as_deref(), Some(pending.as_str()));
    assert_eq!(fx.events("changes_requested"), 1);

    // An explicit owner resend resumes the worker without spending an automatic round.
    let task = corral_ops::resend_reply(&fx.ctx, &fx.id).unwrap();
    assert_eq!(task.status, Status::Running, "{}", fx.reason());
    assert_eq!(task.rounds, 0);
    assert_eq!(fx.stage(), None);
    assert!(fx.run().pending_reply.is_none());
    assert_eq!(fx.run().outcome, None);
    let task = fx.sync();
    assert_eq!(task.status, Status::Running);
    assert_eq!(task.rounds, 0);
    assert_eq!(fx.stage(), None);
    assert_eq!(fx.events("changes_requested"), 2);
}

#[test]
fn send_back_with_a_queued_fresh_retry_leaves_review_for_ready() {
    // Given reviewer changes, a gone worker, and another task occupying the only slot.
    let mut fx = Fx::new(|r| {
        r.gate = gate(&["true"]);
        r.reviewer = Some("rev".into());
        r.accept = Some(Accept::Auto);
    });
    fx.change("widget.txt", "candidate\n");
    fx.done();
    assert_eq!(fx.sync().status, Status::Review);
    fx.verdict(Verdict::Changes, "Repair the widget");
    let original = fx.run();
    std::fs::write(
        fx.bin_dir.join("gone"),
        format!("{}\n", original.herdr.as_ref().unwrap().pane_id),
    )
    .unwrap();
    fx.ctx.store.config.limits.running = 1;
    fx.ctx.store.save_config().unwrap();
    fx.ctx
        .store
        .create("Occupied slot", Some("app"), Status::NeedsMe, "")
        .unwrap();
    assert_eq!(fx.task().rounds, 0);

    // When autopilot queues a fresh retry instead of delivering to the missing pane.
    let task = fx.sync();

    // Then Ready retains the repair round on R1 until there is room to launch R2.
    assert_eq!(task.status, Status::Ready, "{}", fx.reason());
    assert_eq!(task.rounds, 1);
    assert_eq!(fx.stage(), Some(AutoStage::Changes));
    assert!(task.queued);
    assert!(task.dispatch_plan.is_some());
    assert_eq!(task.runs.len(), 1);
    let run = fx.run();
    assert_eq!(run.id, original.id);
    assert_eq!(run.outcome, Some(RunOutcome::Superseded));
    assert_eq!(fx.events("changes_requested"), 0);
    let entries = corral_ops::dispatch_queue(&fx.ctx).unwrap();
    assert!(
        entries
            .iter()
            .any(|entry| entry.id == fx.id && entry.reason.starts_with("waiting for a slot")),
        "{entries:?}"
    );
}

#[test]
fn landing_gate_failure_after_rebase_returns_the_task_to_running() {
    // Given a passing candidate blocked from landing by a dirty main checkout.
    let fx = Fx::new(|r| {
        r.gate = gate(&["grep -qx hello README.md"]);
        r.accept = Some(Accept::Auto);
        r.reviewer = Some("rev".into());
    });
    fx.change("widget.txt", "candidate\n");
    fx.done();
    std::fs::write(fx.app.join("README.md"), "uncommitted main\n").unwrap();
    assert_eq!(fx.sync().status, Status::Review);
    assert_eq!(fx.stage(), Some(AutoStage::Reviewing));
    fx.verdict(Verdict::Approve, "Independent review before landing.");
    assert_eq!(fx.sync().status, Status::Review);
    assert_eq!(fx.stage(), Some(AutoStage::Blocked));
    let original = fx.run();
    assert_eq!(original.gate.as_ref().unwrap().result, GateResult::Pass);
    assert_eq!(fx.task().rounds, 0);
    fx.commit_on_main("README.md", "main moved\n", "Resolve the dirty checkout");
    let base = git(&fx.app, &["rev-parse", "main"]);

    // When landing resumes, rebases, and the gate fails on the integrated candidate.
    let task = fx.sync();

    // Then the same worker resumes with exactly one change round, not a Review card.
    assert_eq!(task.status, Status::Running, "{}", fx.reason());
    assert_eq!(task.rounds, 1);
    assert_eq!(fx.stage(), Some(AutoStage::Changes));
    let run = fx.run();
    assert_eq!(run.id, original.id);
    assert_eq!(run.outcome, None);
    let gate = run.gate.unwrap();
    assert_eq!(gate.result, GateResult::Fail);
    assert_ne!(gate.sha, original.gate.unwrap().sha);
    assert_eq!(gate.sha, git(&run.worktree, &["rev-parse", "HEAD"]));
    assert_eq!(gate.identity.unwrap().integration_base, base);
    assert!(corral_git::is_ancestor(&run.worktree, &base, &gate.sha).unwrap());
    assert_eq!(fx.events("gate_started"), 2);
    assert_eq!(
        fx.transitions(),
        [
            "gating",
            "reviewing",
            "landing",
            "blocked",
            "landing",
            "gating",
            "changes"
        ]
    );
    assert_eq!(git(&fx.app, &["rev-parse", "main"]), base);
}

/// A completed candidate whose branch conflicts with main, so autopilot's next step is a send-back.
fn conflicting_candidate() -> Fx {
    let fx = Fx::new(|r| {
        r.gate = gate(&["true"]);
        r.reviewer = Some("rev".into());
        r.accept = Some(Accept::Auto);
    });
    fx.change("README.md", "candidate\n");
    fx.done();
    fx.commit_on_main("README.md", "main\n", "conflicting main");
    fx
}

/// Nothing was escalated, spent or delivered, and the completed run is exactly as it was.
fn assert_send_back_pending(fx: &Fx, original: &Run) {
    let task = fx.task();
    assert_eq!(task.status, Status::Review, "{}", fx.reason());
    assert_eq!(task.rounds, 0);
    assert_ne!(fx.stage(), Some(AutoStage::Escalated));
    let run = fx.run();
    assert_eq!(run.id, original.id);
    assert_eq!(run.outcome, original.outcome);
    assert_eq!(run.ended, original.ended);
    assert_eq!(run.summary, original.summary);
    assert_eq!(fx.events("escalated"), 0);
    assert_eq!(fx.events("changes_requested"), 0);
    assert_eq!(fx.herdr_log("prompts.log"), "");
}

fn assert_send_back_delivered(fx: &Fx) {
    let task = fx.task();
    assert_eq!(task.status, Status::Running, "{}", fx.reason());
    assert_eq!(task.rounds, 1);
    assert_eq!(fx.events("changes_requested"), 1);
    assert_eq!(fx.events("escalated"), 0);
    assert_eq!(fx.herdr_log("prompts.log").lines().count(), 1);
}

#[test]
fn same_pane_send_back_refused_for_a_slot_waits_and_delivers_once_when_one_frees() {
    // Given a conflicting candidate with a live worker, and another task holding the only slot.
    let mut fx = conflicting_candidate();
    fx.ctx.store.config.limits.running = 1;
    fx.ctx.store.save_config().unwrap();
    let occupant = fx
        .ctx
        .store
        .create("Occupied slot", Some("app"), Status::NeedsMe, "")
        .unwrap();
    let original = fx.run();

    // When autopilot's rebase send-back is refused for lack of a slot, on repeated syncs.
    fx.sync();
    fx.sync();

    // Then the prepared send-back is retained: no escalation, no round, no delivery.
    assert_send_back_pending(&fx, &original);

    // When the slot is released, the next sync delivers that one send-back.
    fx.ctx
        .store
        .update(&occupant.id, |t| {
            t.status = Status::Done;
            Ok(())
        })
        .unwrap();
    fx.sync();
    assert_send_back_delivered(&fx);
}

#[test]
fn send_back_refused_for_a_moved_snapshot_waits_and_delivers_with_current_state() {
    // Given a conflicting candidate and a hook that edits the task file between the send-back's
    // task snapshot and its final submission check: the first pane lookup belongs to autopilot,
    // the second to the submission, which runs after the snapshot is taken.
    let fx = conflicting_candidate();
    let original = fx.run();
    let task_file = fx.task().path;
    std::fs::write(
        fx.bin_dir.join("pane-get-hook"),
        format!(
            "sed -i 's/^  priority: .*/  priority: P1/' '{}'\n",
            task_file.display()
        ),
    )
    .unwrap();
    std::fs::write(fx.bin_dir.join("pane-get-skip"), "1\n").unwrap();

    // When autopilot's rebase send-back finds the snapshot changed.
    fx.sync();

    // Then the hook ran, and the prepared send-back was kept instead of escalated or delivered.
    assert!(!fx.bin_dir.join("pane-get-skip").exists());
    assert_eq!(fx.task().priority, "P1");
    assert_send_back_pending(&fx, &original);

    // When the next sync uses the current state, the send-back is delivered once.
    fx.sync();
    assert_send_back_delivered(&fx);
}
