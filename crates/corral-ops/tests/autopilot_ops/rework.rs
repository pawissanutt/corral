use super::*;

#[test]
fn stale_verdict_then_changes_and_done_gates_the_next_candidate() {
    // Given a reviewer whose verdict was refused after the candidate changed.
    let fx = Fx::new(|r| {
        r.gate = gate(&["true"]);
        r.reviewer = Some("rev".into());
        r.accept = Some(Accept::Auto);
    });
    fx.change("widget.txt", "first\n");
    fx.done();
    fx.sync();
    let old = fx.run().review.unwrap();
    let attempt = &old.identity.as_ref().unwrap().attempt;
    fx.change("widget.txt", "candidate moved during review\n");
    corral_git::commit_all(&fx.run().worktree, "Move candidate").unwrap();
    let error =
        corral_ops::verdict_bound(&fx.ctx, &fx.id, Verdict::Approve, "Reviewed", Some(attempt))
            .unwrap_err();
    assert!(error.to_string().contains("stale review"));
    assert!(fx.run().review.unwrap().verdict.is_none());
    std::fs::write(fx.bin_dir.join("gone"), "w1:p2\n").unwrap();

    // When the owner requests rework and the worker submits a new commit.
    corral_ops::request_changes(&fx.ctx, &fx.id, "Rework the candidate").unwrap();
    let sent_back = fx.run();
    assert!(sent_back.review.is_none());
    assert!(sent_back.review_runs[0].ended.is_some());
    assert_eq!(
        sent_back.review_runs[0].outcome,
        Some(RunOutcome::Superseded)
    );
    fx.change("widget.txt", "reworked\n");
    fx.done();
    assert!(fx.run().review.is_none());
    // The fake reuses its pane ID when the next reviewer launches.
    std::fs::remove_file(fx.bin_dir.join("gone")).unwrap();
    let task = fx.sync();

    // Then the next gate and review belong to the new candidate, without an escalation.
    assert_eq!(task.status, Status::Review);
    assert_eq!(fx.stage(), Some(AutoStage::Reviewing));
    let run = fx.run();
    assert_eq!(run.review_runs.len(), 2);
    let gate = run.gate.unwrap();
    let review = run.review.unwrap();
    assert_ne!(review.sha, old.sha);
    assert_eq!(gate.sha, review.sha);
    assert_eq!(gate.identity, review.identity);
    assert_ne!(review.identity.unwrap().attempt, *attempt);
    assert_eq!(fx.events("gate_started"), 2);
    assert_eq!(fx.events("review_escalated"), 0);
    assert_eq!(fx.sync().status, Status::Review);
}

#[test]
fn new_done_supersedes_pending_review_after_escalation() {
    // Given a missing reviewer and an escalation on its old candidate.
    let fx = Fx::new(|r| {
        r.gate = gate(&["true"]);
        r.reviewer = Some("rev".into());
    });
    fx.change("widget.txt", "first\n");
    fx.done();
    fx.sync();
    let old = fx.run().review.unwrap();
    std::fs::write(fx.bin_dir.join("gone"), "w1:p2\n").unwrap();
    assert_eq!(fx.sync().status, Status::NeedsMe);
    fx.change("widget.txt", "reworked\n");

    // When the worker submits a new candidate without a separate changes command.
    fx.done();
    assert!(fx.run().review.is_none());
    assert!(fx.run().blocked_excerpt.is_none());
    std::fs::remove_file(fx.bin_dir.join("gone")).unwrap();
    let task = fx.sync();

    // Then the old escalation cannot prevent the next gate and review.
    assert_eq!(task.status, Status::Review);
    assert_eq!(fx.stage(), Some(AutoStage::Reviewing));
    assert_ne!(fx.run().review.unwrap().sha, old.sha);
    assert_eq!(fx.events("gate_started"), 2);
    assert_eq!(fx.events("review_escalated"), 1);
}

#[test]
fn forced_review_move_regates_instead_of_reusing_obsolete_pending_review() {
    // Given legacy persisted state with a pending review for an earlier candidate.
    let fx = Fx::new(|r| {
        r.gate = gate(&["true"]);
        r.reviewer = Some("rev".into());
    });
    fx.change("widget.txt", "first\n");
    fx.done();
    fx.sync();
    let old = fx.run().review.unwrap();
    std::fs::write(fx.bin_dir.join("gone"), "w1:p2\n").unwrap();
    assert_eq!(fx.sync().status, Status::NeedsMe);
    fx.change("widget.txt", "reworked\n");
    corral_git::commit_all(&fx.run().worktree, "Rework candidate").unwrap();

    // When the owner force-moves to Review and sync runs.
    corral_ops::move_task(&fx.ctx, &fx.id, Status::Review, true).unwrap();
    std::fs::remove_file(fx.bin_dir.join("gone")).unwrap();
    let task = fx.sync();

    // Then current candidate verification starts, not another old-review escalation.
    assert_eq!(task.status, Status::Review);
    assert_eq!(fx.stage(), Some(AutoStage::Reviewing));
    assert_ne!(fx.run().review.unwrap().sha, old.sha);
    assert_eq!(fx.events("gate_started"), 2);
    assert_eq!(fx.events("review_escalated"), 1);
}
