use super::*;

#[test]
fn main_movement_waits_for_an_active_gate_before_rebasing() {
    // Given a gate whose owned result has not yet become available.
    let fx = Fx::new(|r| {
        r.gate = gate(&["true"]);
        r.remove_worktree = Some(false);
    });
    fx.change("widget.txt", "widget\n");
    fx.done();
    let original = corral_ops::gate_start(&fx.ctx, &fx.id).unwrap();
    let paths = corral_ops::gate_paths(&fx.ctx, &fx.id, &fx.run().id);
    let held = paths.result.with_extension("held");
    std::fs::rename(&paths.result, &held).unwrap();
    fx.commit_on_main("other.txt", "landed\n", "main moved");
    let base = git(&fx.app, &["rev-parse", "main"]);

    // When sync sees main movement while the gate still owns the candidate.
    fx.sync();

    // Then it leaves HEAD and the attempt alone until the result is collectable.
    assert_eq!(
        git(&fx.run().worktree, &["rev-parse", "HEAD"]),
        original.sha
    );
    assert_eq!(fx.run().gate.unwrap(), original);
    assert_eq!(fx.events("candidate_rebased"), 0);
    assert_eq!(fx.events("gate_started"), 1);

    // When that exact result arrives, sync can rebase and claim one replacement.
    std::fs::rename(held, paths.result).unwrap();
    fx.sync();

    // Then only the replacement checks the integrated candidate.
    let gate = fx.run().gate.unwrap();
    assert_eq!(gate.result, GateResult::Pass);
    assert_ne!(gate.sha, original.sha);
    assert_eq!(gate.identity.unwrap().integration_base, base);
    assert_eq!(fx.events("gate_started"), 2);
    assert_eq!(fx.task().rounds, 0);
}

#[test]
fn main_movement_preserves_an_active_reviewer_candidate() {
    // Given a reviewer still judging the original candidate.
    let fx = Fx::new(|r| {
        r.gate = gate(&["true"]);
        r.reviewer = Some("rev".into());
        r.accept = Some(Accept::Auto);
        r.remove_worktree = Some(false);
    });
    fx.change("widget.txt", "widget\n");
    fx.done();
    fx.sync();
    let original = fx.run().review.unwrap();
    fx.commit_on_main("other.txt", "landed\n", "main moved");

    // When sync observes main movement before a verdict.
    fx.sync();

    // Then it does not change the candidate underneath the reviewer.
    assert_eq!(
        git(&fx.run().worktree, &["rev-parse", "HEAD"]),
        original.sha
    );
    assert_eq!(fx.run().review.unwrap(), original);
    assert_eq!(fx.events("candidate_rebased"), 0);
    assert_eq!(fx.events("gate_started"), 1);
}
