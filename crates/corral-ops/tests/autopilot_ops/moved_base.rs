//! T-486: a landing on main while a review runs must not void the reviewer's verdict.
use super::*;

const NOTES: &str = "Reject an empty widget before indexing.";

/// A candidate under review whose reviewed base is then overtaken by another landing on main.
fn reviewing_then_main_moves(main_file: &str, main_text: &str) -> (Fx, String) {
    let fx = Fx::new(|r| {
        r.gate = gate(&["true"]);
        r.reviewer = Some("rev".into());
        r.accept = Some(Accept::Auto);
        r.max_rounds = Some(2);
        r.remove_worktree = Some(false);
    });
    fx.change("widget.txt", "w\n");
    fx.done();
    fx.sync();
    assert_eq!(fx.stage(), Some(AutoStage::Reviewing));
    let reviewed = fx.run().review.unwrap().identity.unwrap().integration_base;
    fx.commit_on_main(main_file, main_text, "Another task lands");
    assert_ne!(git(&fx.app, &["rev-parse", "main"]), reviewed);
    (fx, reviewed)
}

fn moved_event(fx: &Fx) -> serde_json::Value {
    let events = fx.ctx.store.events().unwrap();
    let event = events.iter().find(|e| e.kind == "verdict_base_moved");
    event.expect("verdict_base_moved").detail.clone()
}

#[test]
fn an_approval_given_while_main_moved_is_recorded_and_carried_without_a_second_review() {
    let (fx, reviewed) = reviewing_then_main_moves("unrelated.txt", "other task\n");
    let head = fx.run().review.unwrap().sha;

    // When the reviewer approves after the landing.
    fx.verdict(Verdict::Approve, "Looks right.");

    // Then the verdict is recorded against the base it reviewed.
    let review = fx.run().review.unwrap();
    assert_eq!(review.verdict, Some(Verdict::Approve));
    assert_eq!(review.sha, head);
    assert_eq!(review.identity.unwrap().integration_base, reviewed);
    let moved = moved_event(&fx);
    assert_eq!(moved["reviewed_base"], reviewed.as_str());
    assert_eq!(moved["current_base"], git(&fx.app, &["rev-parse", "main"]));
    assert_eq!(fx.task().status, Status::Review);

    // And autopilot rebases, re-gates and lands it with the carried approval.
    assert_eq!(fx.sync().status, Status::Done, "{}", fx.reason());
    assert_eq!(fx.events("candidate_rebased"), 1);
    assert_eq!(fx.events("gate_started"), 2);
    assert_eq!(fx.events("verdict_carried"), 1);
    assert_eq!(fx.events("review_started"), 1, "no second review");
    assert!(fx.app.join("widget.txt").is_file() && fx.app.join("unrelated.txt").is_file());
}

#[test]
fn changes_given_while_main_moved_reach_the_worker_without_a_pm_relay() {
    let (fx, reviewed) = reviewing_then_main_moves("unrelated.txt", "other task\n");

    fx.verdict(Verdict::Changes, NOTES);

    let review = fx.run().review.unwrap();
    assert_eq!(review.verdict, Some(Verdict::Changes));
    assert_eq!(review.identity.unwrap().integration_base, reviewed);
    let task = fx.sync();
    assert_eq!(task.status, Status::Running, "{}", fx.reason());
    assert_eq!(task.rounds, 1);
    assert_eq!(fx.stage(), Some(AutoStage::Changes));
    assert!(
        fx.artifact_text("Autopilot changes (round 1)")
            .contains(NOTES)
    );
    assert!(fx.herdr_log("prompts.log").contains("requested changes"));
    assert_eq!(fx.events("review_started"), 1);
}

#[test]
fn an_approval_across_a_conflicting_landing_is_never_carried() {
    let (fx, _) = reviewing_then_main_moves("widget.txt", "conflicting main\n");

    fx.verdict(Verdict::Approve, "Looks right.");
    assert_eq!(fx.run().review.unwrap().verdict, Some(Verdict::Approve));

    // The replay conflicts, so the worker gets an integration repair, not a landing.
    let task = fx.sync();
    assert_eq!(task.status, Status::Running, "{}", fx.reason());
    assert_eq!(fx.stage(), Some(AutoStage::Changes));
    assert_eq!(fx.events("integration_request"), 1);
    assert_eq!(fx.events("verdict_carried"), 0);

    // The worker resolves onto main; the resolution is reviewed afresh.
    let worktree = fx.run().worktree;
    git(&worktree, &["rebase", "-X", "theirs", "main"]);
    fx.done();
    let task = fx.sync();
    assert_eq!(task.status, Status::Review, "{}", fx.reason());
    assert_eq!(fx.stage(), Some(AutoStage::Reviewing));
    assert_eq!(fx.events("verdict_carried"), 0);
    assert_eq!(
        fx.events("review_started"),
        2,
        "the resolution needs a new review"
    );
}

#[test]
fn a_verdict_after_main_was_rewritten_is_still_refused() {
    let (fx, reviewed) = reviewing_then_main_moves("unrelated.txt", "other task\n");
    // main no longer descends from the reviewed base.
    git(&fx.app, &["reset", "-q", "--hard", &reviewed]);
    git(
        &fx.app,
        &["commit", "-q", "--amend", "-m", "rewritten history"],
    );

    let error = corral_ops::verdict(&fx.ctx, &fx.id, Verdict::Approve, "Looks right.").unwrap_err();
    assert!(format!("{error:#}").contains("stale review:"), "{error:#}");
    assert!(fx.run().review.unwrap().verdict.is_none());
}
