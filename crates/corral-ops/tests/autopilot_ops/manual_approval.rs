use super::*;

#[test]
fn manual_approval_rejects_runtime_skew_before_git_side_effects() {
    for strategy in [
        ApproveStrategy::Rebase,
        ApproveStrategy::FastForward,
        ApproveStrategy::KeepBranch,
    ] {
        // Given an owner-ready task whose store schema moved past this runtime.
        let fx = Fx::new(|_| {});
        fx.change("widget.txt", "w\n");
        fx.done();
        let run = fx.run();
        let head = git(&run.worktree, &["rev-parse", "HEAD"]);
        fx.commit_on_main("other.txt", "main moved\n", "main advanced");
        let base = git(&fx.app, &["rev-parse", "HEAD"]);
        std::fs::write(
            fx.ctx.store.state_dir().join("schema-version"),
            (corral_core::STORE_SCHEMA_VERSION + 1).to_string(),
        )
        .unwrap();

        // When either approval entry point reaches the shared landing boundary.
        let error = corral_ops::approve(&fx.ctx, &fx.id, strategy, true, false).unwrap_err();

        // Then runtime rejection happens before rebase, landing or cleanup.
        assert!(error.to_string().contains(corral_core::UPDATE_NOTICE));
        assert_eq!(git(&run.worktree, &["rev-parse", "HEAD"]), head);
        assert_eq!(git(&fx.app, &["rev-parse", "HEAD"]), base);
        assert!(run.worktree.is_dir());
        assert!(!fx.app.join("widget.txt").exists());
        assert_eq!(fx.events("approved"), 0);
    }
}

fn approved(gate_command: &str) -> Fx {
    let fx = Fx::new(|r| {
        r.gate = gate(&[gate_command]);
        r.reviewer = Some("rev".into());
        r.accept = Some(Accept::Owner);
        r.remove_worktree = Some(false);
    });
    fx.change("widget.txt", "w\n");
    fx.done();
    fx.sync();
    fx.verdict(Verdict::Approve, "Original candidate reviewed.");
    fx
}

#[test]
fn manual_rebase_carries_equal_patch_after_fresh_gate_and_lands() {
    // Given approval bound to the old HEAD and integration base.
    let fx = approved("test ! -f fail-gate.txt");
    let old = fx.run().review.unwrap();
    fx.commit_on_main("other.txt", "main moved\n", "main advanced");
    let base = git(&fx.app, &["rev-parse", "HEAD"]);
    let (tx, messages) = std::sync::mpsc::channel();
    let _capture = corral_ops::capture_messages(tx);

    // When a single manual approve integrates, gates, carries, and lands.
    corral_ops::approve(&fx.ctx, &fx.id, ApproveStrategy::Rebase, false, true).unwrap();

    // Then both receipts identify the landed candidate, with attributable evidence.
    let run = fx.run();
    let review = run.review.unwrap();
    let gate = run.gate.unwrap();
    assert_eq!(fx.task().status, Status::Done);
    assert_eq!(git(&fx.app, &["rev-parse", "HEAD"]), gate.sha);
    assert_ne!(review.sha, old.sha);
    assert_eq!(review.identity, gate.identity);
    assert_eq!(review.identity.unwrap().integration_base, base);
    assert_eq!(review.verdict, Some(Verdict::Approve));
    assert_eq!(review.reviewer, old.reviewer);
    assert_eq!(review.notes, old.notes);
    assert_eq!(fx.events("gate_started"), 2);
    assert_eq!(fx.events("review_started"), 1);
    assert_eq!(fx.events("verdict_carried"), 1);
    let events = fx.ctx.store.events().unwrap();
    let carry = events.iter().find(|e| e.kind == "verdict_carried").unwrap();
    assert_eq!(carry.detail["old_sha"], old.sha);
    assert_eq!(carry.detail["new_sha"], gate.sha);
    assert_eq!(
        carry.detail["evidence"]["old_patch_sha256"],
        carry.detail["evidence"]["new_patch_sha256"]
    );
    let gate_at = events.iter().rposition(|e| e.kind == "gate").unwrap();
    let carry_at = events
        .iter()
        .position(|e| e.kind == "verdict_carried")
        .unwrap();
    let land_at = events.iter().position(|e| e.kind == "approved").unwrap();
    assert!(gate_at < carry_at && carry_at < land_at);
    assert!(messages.try_iter().any(|m| m.source == "review"));
}

#[test]
fn changed_patch_after_rebase_requires_review_and_does_not_land() {
    // Given a reviewed two-commit patch, one commit independently lands on main.
    let fx = Fx::new(|r| {
        r.gate = gate(&["true"]);
        r.reviewer = Some("rev".into());
        r.remove_worktree = Some(false);
    });
    fx.change("shared.txt", "already landed\n");
    corral_git::commit_all(&fx.run().worktree, "shared change").unwrap();
    fx.change("widget.txt", "w\n");
    fx.done();
    fx.sync();
    fx.verdict(Verdict::Approve, "Reviewed both commits.");
    fx.commit_on_main(
        "shared.txt",
        "already landed\n",
        "independent shared change",
    );
    let base = git(&fx.app, &["rev-parse", "HEAD"]);

    // When rebase drops the already-applied commit, changing the reviewed patch.
    let error =
        corral_ops::approve(&fx.ctx, &fx.id, ApproveStrategy::Rebase, false, true).unwrap_err();

    // Then a fresh gate cannot carry the changed semantic review.
    assert!(error.to_string().contains("corral review-run"));
    assert_eq!(fx.run().gate.unwrap().result, GateResult::Pass);
    assert_eq!(fx.task().status, Status::Review);
    assert_eq!(fx.events("verdict_carried"), 0);
    assert_eq!(fx.events("approved"), 0);
    assert_eq!(git(&fx.app, &["rev-parse", "HEAD"]), base);
    assert!(!fx.app.join("widget.txt").exists());
}

#[test]
fn stale_review_after_approve_rebase_stays_in_review_and_gets_a_fresh_review() {
    // Given approve's own rebase changed the reviewed patch, so the verdict cannot carry.
    let fx = Fx::new(|r| {
        r.gate = gate(&["true"]);
        r.reviewer = Some("rev".into());
        r.remove_worktree = Some(false);
    });
    fx.change("shared.txt", "already landed\n");
    corral_git::commit_all(&fx.run().worktree, "shared change").unwrap();
    fx.change("widget.txt", "w\n");
    fx.done();
    fx.sync();
    fx.verdict(Verdict::Approve, "Reviewed both commits.");
    fx.commit_on_main(
        "shared.txt",
        "already landed\n",
        "independent shared change",
    );
    corral_ops::approve(&fx.ctx, &fx.id, ApproveStrategy::Rebase, false, true).unwrap_err();

    // When autopilot next looks at the rebased worktree.
    let task = fx.sync();

    // Then it is not reviewer tampering: the task stays in Review and a fresh review starts.
    assert_eq!(task.status, Status::Review);
    assert_ne!(fx.stage(), Some(AutoStage::Escalated), "{}", fx.reason());
    assert_eq!(fx.events("review_started"), 2);
}

#[test]
fn integrated_gate_failure_never_carries_or_lands() {
    // Given a gate passing before main adds an incompatible input.
    let fx = approved("test ! -f fail-gate.txt");
    fx.commit_on_main(
        "fail-gate.txt",
        "fail the integrated gate\n",
        "break integration",
    );
    let base = git(&fx.app, &["rev-parse", "HEAD"]);

    // When one approve rebases and the fresh gate fails.
    let error =
        corral_ops::approve(&fx.ctx, &fx.id, ApproveStrategy::Rebase, false, true).unwrap_err();

    // Then main is unchanged, no verdict is carried, and a command is given.
    assert!(error.to_string().contains("corral gate"));
    assert_eq!(fx.run().gate.unwrap().result, GateResult::Fail);
    assert_eq!(fx.task().status, Status::Review);
    assert_eq!(fx.events("verdict_carried"), 0);
    assert_eq!(fx.events("approved"), 0);
    assert_eq!(git(&fx.app, &["rev-parse", "HEAD"]), base);
}

#[test]
fn rebase_conflict_never_carries_and_reports_resolution_command() {
    // Given approved content conflicting with main.
    let fx = approved("true");
    fx.commit_on_main("widget.txt", "different\n", "conflict");
    let old = fx.run().review.unwrap().sha;

    // When approve attempts the real replay.
    let error =
        corral_ops::approve(&fx.ctx, &fx.id, ApproveStrategy::Rebase, false, true).unwrap_err();

    // Then the replay is aborted without a carry or landing.
    assert!(error.to_string().contains("git -C"));
    assert_eq!(git(&fx.run().worktree, &["rev-parse", "HEAD"]), old);
    assert_eq!(fx.events("verdict_carried"), 0);
    assert_eq!(fx.events("approved"), 0);
}

#[test]
fn repeated_approve_cannot_bypass_a_failed_integrated_gate_without_a_reviewer() {
    // Given a stale candidate with a passing original gate and no semantic reviewer.
    let fx = Fx::new(|r| {
        r.gate = gate(&["test ! -f fail-gate.txt"]);
        r.remove_worktree = Some(false);
    });
    fx.change("widget.txt", "w\n");
    fx.done();
    fx.sync();
    fx.commit_on_main("fail-gate.txt", "fail\n", "incompatible main");
    let base = git(&fx.app, &["rev-parse", "HEAD"]);

    // When the same approval is retried after its integrated gate fails.
    for _ in 0..2 {
        assert!(
            corral_ops::approve(&fx.ctx, &fx.id, ApproveStrategy::Rebase, false, true).is_err()
        );
    }

    // Then neither attempt lands the unverified candidate.
    assert_eq!(fx.events("approved"), 0);
    assert_eq!(git(&fx.app, &["rev-parse", "HEAD"]), base);
    assert_eq!(fx.task().status, Status::Review);
}

#[test]
fn equal_patch_does_not_carry_when_review_inputs_change() {
    // Given an approved patch, with main changing a verification input rather than the patch.
    let fx = approved("true");
    fx.commit_on_main(
        "AGENTS.md",
        "New project instructions.\n",
        "new review input",
    );
    let base = git(&fx.app, &["rev-parse", "HEAD"]);

    // When approve cleanly integrates and gates it.
    let error =
        corral_ops::approve(&fx.ctx, &fx.id, ApproveStrategy::Rebase, false, true).unwrap_err();

    // Then patch equivalence alone cannot authorize review with changed inputs.
    assert!(error.to_string().contains("corral review-run"));
    assert_eq!(fx.run().gate.unwrap().result, GateResult::Pass);
    assert_eq!(fx.events("verdict_carried"), 0);
    assert_eq!(fx.events("approved"), 0);
    assert_eq!(git(&fx.app, &["rev-parse", "HEAD"]), base);
}
