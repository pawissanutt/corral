use super::*;

#[test]
fn rebase_after_approval_requires_a_new_gate_and_semantic_review() {
    let fx = Fx::new(|r| {
        r.gate = gate(&["true"]);
        r.reviewer = Some("rev".into());
        r.accept = Some(Accept::Auto);
        r.remove_worktree = Some(false);
    });
    fx.change("widget.txt", "w\n");
    fx.done();
    fx.sync();
    fx.verdict(Verdict::Approve, "First candidate reviewed.");
    let original = fx.run();
    let first = original.gate.unwrap().identity.unwrap();
    fx.commit_on_main(
        "unrelated.txt",
        &"other task\n".repeat(2000),
        "unrelated landed task",
    );
    // When main moves after approval, the integrated candidate must be reviewed anew.
    assert_eq!(fx.sync().status, Status::Review);
    let run = fx.run();
    let gate = run.gate.as_ref().unwrap();
    let review = run.review.as_ref().unwrap();
    assert_ne!(gate.sha, first.head);
    assert_ne!(gate.identity.as_ref().unwrap().attempt, first.attempt);
    assert_eq!(review.identity, gate.identity);
    assert_eq!(review.verdict, None);
    assert_eq!(run.base_sha, first.dispatch_base);
    let info = corral_ops::review_info(&fx.ctx, &fx.id).unwrap();
    assert_eq!(info.stat.added, 1);
    assert_eq!(info.stat.removed, 0);
    assert_eq!(info.stat.files[0].path, "widget.txt");
    assert!(!fx.app.join("widget.txt").exists());
    fx.verdict(Verdict::Approve, "Integrated candidate reviewed.");
    assert_eq!(fx.sync().status, Status::Done);
    assert_eq!(git(&fx.app, &["rev-parse", "HEAD"]), gate.sha);
}

#[test]
fn policy_edit_after_owner_review_runs_the_new_recipe_without_landing() {
    let fx = Fx::new(|r| {
        r.gate = gate(&["true"]);
        r.max_rounds = Some(0);
    });
    fx.change("widget.txt", "w\n");
    fx.done();
    fx.sync();
    assert_eq!(fx.stage(), Some(AutoStage::Reviewed));
    let head = fx.run().gate.unwrap().sha;
    let mut store = Store::open(fx.ctx.store.root()).unwrap();
    store.config.review.gate = gate(&["false"]);
    store.save_config().unwrap();
    fx.sync();
    assert_eq!(fx.stage(), Some(AutoStage::Escalated));
    let gate = fx.run().gate.unwrap();
    assert_eq!(gate.sha, head);
    assert_eq!(gate.result, GateResult::Fail);
    assert!(!fx.app.join("widget.txt").exists());
}

#[test]
fn rename_cannot_hide_sensitive_original_path() {
    let fx = Fx::new(|r| {
        r.accept = Some(Accept::Auto);
        r.owner_paths = Some(vec!["README.md".into()]);
    });
    git(&fx.run().worktree, &["mv", "README.md", "ordinary.txt"]);
    fx.done();
    assert_eq!(fx.sync().status, Status::NeedsMe);
    assert!(fx.reason().contains("README.md"));
    assert!(fx.app.join("README.md").is_file());
}

#[test]
fn delayed_review_callback_cannot_approve_another_gate_attempt() {
    let fx = Fx::new(|r| {
        r.gate = gate(&["true"]);
        r.reviewer = Some("rev".into());
        r.accept = Some(Accept::Auto);
    });
    fx.change("widget.txt", "w\n");
    fx.done();
    fx.sync();
    let attempt = fx.run().review.unwrap().identity.unwrap().attempt;
    corral_ops::gate_start(&fx.ctx, &fx.id).unwrap();
    corral_ops::gate_wait(&fx.ctx, &fx.id).unwrap();
    fx.sync();
    let result = corral_ops::verdict_bound(
        &fx.ctx,
        &fx.id,
        Verdict::Approve,
        "Delayed.",
        Some(&attempt),
    );
    assert!(result.is_err());
    assert_eq!(fx.run().review.unwrap().verdict, None);
    assert!(!fx.app.join("widget.txt").exists());
}
