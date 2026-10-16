use super::*;
use corral_core::RunOutcome;
use corral_ops::watch::{ProgressState, load};

#[test]
fn every_approval_strategy_finalizes_an_unfinished_current_run() {
    for strategy in [
        ApproveStrategy::FastForward,
        ApproveStrategy::Rebase,
        ApproveStrategy::KeepBranch,
    ] {
        // Given owner-ready state with the reopened fields left by an older writer.
        let fx = Fx::new();
        fx.owner_ready();
        fx.ctx
            .store
            .update(&fx.id, |task| {
                let run = task.current_run_mut().unwrap();
                run.ended = None;
                run.outcome = None;
                run.agent_state = Some("working".into());
                run.pending_reply = Some("changes requested".into());
                run.idle_no_report = true;
                Ok(())
            })
            .unwrap();

        // When the owner accepts through any successful landing strategy.
        corral_ops::approve(&fx.ctx, &fx.id, strategy, false, true).unwrap();

        // Then Done and the finished run are persisted together.
        let task = fx.ctx.store.get(&fx.id).unwrap();
        let run = task.current_run().unwrap();
        assert_eq!(task.status, Status::Done);
        assert!(run.ended.is_some(), "{strategy:?}: landed run has no end");
        assert_eq!(run.outcome, Some(RunOutcome::Done), "{strategy:?}");
        assert_eq!(run.agent_state.as_deref(), Some("done"));
        assert!(run.pending_reply.is_none());
        assert!(!run.idle_no_report);
    }
}

#[test]
fn approval_rejects_autopilot_changes_interleaved_before_owner_acceptance() {
    // Given a reviewer change request actually sent back by the autopilot.
    let fx = Fx::new();
    fx.review();
    corral_ops::verdict(&fx.ctx, &fx.id, Verdict::Changes, "Fix the defect.").unwrap();
    fx.sync();
    let before = fx.run();
    assert!(before.ended.is_none());
    assert!(before.outcome.is_none());
    assert_eq!(before.autopilot.as_ref().unwrap().stage, AutoStage::Changes);
    let main = git(&fx.app, &["rev-parse", "main"]);

    // When the owner approves immediately after that transition.
    let error =
        corral_ops::approve(&fx.ctx, &fx.id, ApproveStrategy::Rebase, true, false).unwrap_err();

    // Then it cannot land a now-running candidate or leave a Done/open-run pair.
    assert!(error.to_string().contains("only Review tasks"));
    assert_eq!(fx.ctx.store.get(&fx.id).unwrap().status, Status::Running);
    assert_eq!(fx.run(), before);
    assert_eq!(git(&fx.app, &["rev-parse", "main"]), main);
    assert!(before.worktree.is_dir());
}

#[test]
fn finished_review_capture_is_not_polled_again_after_landing() {
    // Given a review captured at its verdict and then landed with its panes closed.
    let fx = Fx::new();
    let panes = fx.owner_ready();
    let capture = fx
        .ctx
        .store
        .state_dir()
        .join("review-watch")
        .join(&fx.id)
        .join("R1-review1.json");
    let before = std::fs::read(&capture).unwrap();
    corral_ops::approve(&fx.ctx, &fx.id, ApproveStrategy::FastForward, true, false).unwrap();
    fx.reset_calls();
    let (tx, messages) = std::sync::mpsc::channel();
    let _capture = corral_ops::capture_messages(tx);

    // When repeated sync and watch loads revisit the terminal task.
    fx.sync();
    fx.sync();
    let history = load(&fx.ctx, &fx.id).unwrap();

    // Then the final capture is stable and no finished reviewer pane is read.
    assert_eq!(std::fs::read(&capture).unwrap(), before);
    assert!(history.reviews.iter().all(|r| r.live_pane.is_none()));
    assert!(!fx.called(&["pane", "get", &panes[2]]));
    assert!(
        fx.state()["calls"]
            .as_array()
            .unwrap()
            .iter()
            .all(|a| a[0] != "pane" || a[1] != "read")
    );
    assert!(
        !messages
            .try_iter()
            .any(|m| m.text.contains("pane_not_found"))
    );
}

#[test]
fn terminal_tasks_never_expose_legacy_open_review_or_worker_panes() {
    for status in [Status::Done, Status::Dropped] {
        // Given a terminal task with legacy unfinished worker/reviewer records.
        let fx = Fx::new();
        fx.review();
        fx.ctx
            .store
            .update(&fx.id, |task| {
                task.status = status;
                let run = task.current_run_mut().unwrap();
                run.ended = None;
                run.outcome = None;
                Ok(())
            })
            .unwrap();
        fx.reset_calls();

        // When the read-only watches load before cleanup has closed the panes.
        let history = load(&fx.ctx, &fx.id).unwrap();
        let tail = corral_ops::live_tail(&fx.ctx, &fx.id, 10).unwrap();

        // Then neither watch polls live output for terminal work.
        assert!(tail.is_empty());
        assert_eq!(history.reviews[0].state, ProgressState::Interrupted);
        assert!(history.reviews[0].live_pane.is_none());
        assert!(
            fx.state()["calls"]
                .as_array()
                .unwrap()
                .iter()
                .all(|a| a[0] != "pane" || (a[1] != "get" && a[1] != "read"))
        );
    }
}

#[test]
fn missing_reviewer_watch_stops_probing_after_first_loss() {
    // Given an active reviewer whose pane has disappeared.
    let fx = Fx::new();
    let panes = fx.review();
    fx.edit(|s| {
        s["panes"].as_object_mut().unwrap().remove(&panes[2]);
    });
    let first = load(&fx.ctx, &fx.id).unwrap();
    fx.reset_calls();

    // When a later watch refresh observes the same attempt.
    let repeated = load(&fx.ctx, &fx.id).unwrap();

    // Then the first interruption is stable without polling the lost pane again.
    assert_eq!(first.reviews[0].state, ProgressState::Interrupted);
    assert_eq!(repeated.reviews[0].state, ProgressState::Interrupted);
    assert_eq!(
        repeated.reviews[0].accounting.ended,
        first.reviews[0].accounting.ended
    );
    assert!(!fx.called(&["pane", "get", &panes[2]]));
}

#[test]
fn legacy_terminal_capture_handles_an_already_missing_reviewer_quietly() {
    // Given an uncaptured legacy review on a terminal task with a closed pane.
    let fx = Fx::new();
    let panes = fx.review();
    fx.edit(|s| {
        s["panes"].as_object_mut().unwrap().remove(&panes[2]);
    });
    fx.ctx
        .store
        .update(&fx.id, |task| {
            task.status = Status::Done;
            Ok(())
        })
        .unwrap();
    let (tx, messages) = std::sync::mpsc::channel();
    let _capture = corral_ops::capture_messages(tx);

    // When sync first collects the old window, then revisits it.
    fx.sync();
    fx.reset_calls();
    fx.sync();

    // Then pane loss produces no warning and the final capture is never retried.
    assert!(
        !messages
            .try_iter()
            .any(|m| m.text.contains("pane_not_found"))
    );
    assert!(
        fx.state()["calls"]
            .as_array()
            .unwrap()
            .iter()
            .all(|a| a[0] != "pane" || a[1] != "read")
    );
}

#[test]
fn verdict_capture_stops_before_terminal_cleanup() {
    // Given a verdict captured while waiting for owner acceptance.
    let fx = Fx::new();
    let panes = fx.owner_ready();
    fx.reset_calls();

    // When watch refreshes a finished review whose pane is still open.
    let history = load(&fx.ctx, &fx.id).unwrap();
    let tail = corral_ops::live_tail(&fx.ctx, &fx.id, 10).unwrap();

    // Then both coding and review output are saved, not live.
    assert!(tail.is_empty());
    assert_eq!(history.reviews[0].state, ProgressState::Passed);
    assert!(history.reviews[0].live_pane.is_none());
    assert!(!fx.called(&["pane", "get", &panes[2]]));
    assert!(
        fx.state()["calls"]
            .as_array()
            .unwrap()
            .iter()
            .all(|a| a[0] != "pane" || a[1] != "read")
    );
}
