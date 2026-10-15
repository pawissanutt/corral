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

#[test]
fn manual_rebase_requires_fresh_gate_and_semantic_review_before_landing() {
    // Given approval bound to the old HEAD and integration base.
    let fx = Fx::new(|r| {
        r.gate = gate(&["true"]);
        r.reviewer = Some("rev".into());
        r.accept = Some(Accept::Owner);
        r.remove_worktree = Some(false);
    });
    fx.change("widget.txt", "w\n");
    fx.done();
    fx.sync();
    fx.verdict(Verdict::Approve, "Original candidate reviewed.");
    let old = fx.run().review.unwrap().identity.unwrap();
    fx.commit_on_main("other.txt", "main moved\n", "main advanced");
    let base = git(&fx.app, &["rev-parse", "HEAD"]);

    // When manual approval rebases the candidate.
    let result = corral_ops::approve(&fx.ctx, &fx.id, ApproveStrategy::Rebase, true, false);

    // Then it must not land or clean up the unverified, rebased candidate.
    assert!(
        result.is_err(),
        "old approval landed rebased HEAD: {result:?}"
    );
    let run = fx.run();
    let head = git(&run.worktree, &["rev-parse", "HEAD"]);
    assert_ne!(head, old.head);
    assert_eq!(git(&fx.app, &["rev-parse", "HEAD"]), base);
    assert!(!fx.app.join("widget.txt").exists());
    assert_eq!(fx.task().status, Status::Review);
    assert_eq!(run.gate.as_ref().unwrap().identity.as_ref(), Some(&old));
    assert_eq!(run.review.as_ref().unwrap().identity.as_ref(), Some(&old));
    assert_ne!(old.integration_base, base);
    assert_eq!(fx.events("approved"), 0);

    // A retry (including fast-forward) cannot bypass the stale receipts.
    for strategy in [ApproveStrategy::Rebase, ApproveStrategy::FastForward] {
        assert!(corral_ops::approve(&fx.ctx, &fx.id, strategy, false, true).is_err());
        assert_eq!(git(&fx.app, &["rev-parse", "HEAD"]), base);
    }
    corral_ops::gate_start(&fx.ctx, &fx.id).unwrap();
    assert_eq!(
        corral_ops::gate_wait(&fx.ctx, &fx.id).unwrap().result,
        GateResult::Pass
    );
    assert!(
        corral_ops::approve(&fx.ctx, &fx.id, ApproveStrategy::FastForward, false, true).is_err(),
        "a fresh gate alone must not refresh semantic approval"
    );
    corral_ops::review_run(&fx.ctx, &fx.id, None).unwrap();
    assert!(
        corral_ops::approve(&fx.ctx, &fx.id, ApproveStrategy::FastForward, false, true).is_err(),
        "a pending semantic review is not approval"
    );
    fx.verdict(Verdict::Approve, "Rebased candidate reviewed.");
    let fresh = fx.run().review.unwrap().identity.unwrap();
    assert_eq!(fresh.head, head);
    assert_eq!(fresh.integration_base, base);
    assert_ne!(fresh.attempt, old.attempt);
    corral_ops::approve(&fx.ctx, &fx.id, ApproveStrategy::Rebase, false, true).unwrap();
    assert_eq!(fx.task().status, Status::Done);
    assert_eq!(git(&fx.app, &["rev-parse", "HEAD"]), head);
    assert!(fx.app.join("widget.txt").is_file());
}
