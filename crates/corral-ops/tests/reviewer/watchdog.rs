use std::collections::HashMap;

use corral_core::AutoStage;
use corral_herdr::{AgentStatus, PaneInfo};
use jiff::{SignedDuration, Timestamp};

use super::*;

fn pane(status: AgentStatus) -> HashMap<String, PaneInfo> {
    let info = PaneInfo {
        pane_id: "w1:p2".into(),
        workspace_id: "w1".into(),
        tab_id: "w1:t2".into(),
        agent: Some("rev".into()),
        status,
        title: None,
        cwd: None,
    };
    HashMap::from([(info.pane_id.clone(), info)])
}

fn started(ctx: &Ctx, id: &str) -> Timestamp {
    corral_ops::review_run(ctx, id, Some("rev")).unwrap().at
}

fn after(now: Timestamp, seconds: i64) -> Timestamp {
    now.checked_add(SignedDuration::from_secs(seconds)).unwrap()
}

#[test]
fn sync_escalates_idle_review_even_when_autopilot_is_disabled() {
    // Given
    let (_tmp, mut ctx, id, _) = task_in_review();
    ctx.store.config.review.autopilot = Some(false);
    ctx.store.save_config().unwrap();
    let now = started(&ctx, &id);
    corral_ops::sync_at(&ctx, now).unwrap();
    assert_eq!(ctx.store.get(&id).unwrap().status, Status::Review);
    // When
    let changes = corral_ops::sync_at(&ctx, after(now, 120)).unwrap();
    // Then
    assert_eq!(ctx.store.get(&id).unwrap().status, Status::NeedsMe);
    assert!(
        changes
            .iter()
            .any(|change| change.task == id && change.to == Status::NeedsMe)
    );
    assert_eq!(
        ctx.store
            .events()
            .unwrap()
            .iter()
            .filter(|e| e.kind == "review_escalated")
            .count(),
        1
    );
}

#[test]
fn idle_timeout_survives_restart_without_autopilot_and_surfaces_saved_notes() {
    let (_tmp, mut ctx, id, _) = task_in_review();
    ctx.store.config.review.autopilot = Some(false);
    ctx.store.save_config().unwrap();
    let now = started(&ctx, &id);
    let notes = corral_ops::review_notes_path(&ctx, &id, "R1");
    std::fs::write(&notes, "Finished reviewing; callback did not arrive.\n").unwrap();
    let panes = pane(AgentStatus::Idle);

    assert!(
        corral_ops::reconcile_review_at(&ctx, &id, now, &panes)
            .unwrap()
            .is_none()
    );
    ctx.store = Store::open(ctx.store.root()).unwrap();
    assert_eq!(ctx.store.config.sync.idle_after_secs, 120);
    assert!(
        corral_ops::reconcile_review_at(&ctx, &id, after(now, 119), &panes)
            .unwrap()
            .is_none()
    );
    let change = corral_ops::reconcile_review_at(&ctx, &id, after(now, 120), &panes)
        .unwrap()
        .unwrap();
    assert_eq!(change.to, Status::NeedsMe);
    assert_eq!(change.stage, Some(AutoStage::Escalated));
    let saved = ctx.store.get(&id).unwrap();
    assert!(saved.escalated());
    let run = saved.current_run().unwrap();
    assert_eq!(run.review.as_ref().unwrap().verdict, None);
    assert_eq!(run.review_runs[0].ended, Some(after(now, 120)));
    let reason = run.autopilot.as_ref().unwrap().reason.as_ref().unwrap();
    assert!(reason.contains("120 seconds"));
    assert!(reason.contains(&notes.display().to_string()));
    assert_eq!(run.blocked_excerpt.as_ref(), Some(reason));
    assert_eq!(
        std::fs::read_to_string(&notes).unwrap(),
        "Finished reviewing; callback did not arrive.\n"
    );
    assert!(
        corral_ops::reconcile_review_at(&ctx, &id, after(now, 121), &panes)
            .unwrap()
            .is_none()
    );
    let events = ctx.store.events().unwrap();
    let events: Vec<_> = events
        .iter()
        .filter(|e| e.kind == "review_escalated")
        .collect();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].detail["notes"], notes.display().to_string());
}

#[test]
fn activity_resets_configurable_idle_deadline_and_done_counts_as_idle() {
    let (_tmp, mut ctx, id, _) = task_in_review();
    ctx.store.config.sync.idle_after_secs = 30;
    let now = started(&ctx, &id);
    for (seconds, status) in [
        (0, AgentStatus::Idle),
        (29, AgentStatus::Idle),
        (30, AgentStatus::Working),
        (60, AgentStatus::Done),
        (89, AgentStatus::Done),
    ] {
        assert!(
            corral_ops::reconcile_review_at(&ctx, &id, after(now, seconds), &pane(status))
                .unwrap()
                .is_none()
        );
        if status == AgentStatus::Working {
            let task = ctx.store.get(&id).unwrap();
            assert!(task.current_run().unwrap().idle_since.is_none());
        }
    }
    assert!(
        corral_ops::reconcile_review_at(&ctx, &id, after(now, 90), &pane(AgentStatus::Done))
            .unwrap()
            .is_some()
    );
}

#[test]
fn failed_callback_is_durable_and_escalates_as_soon_as_reviewer_is_idle() {
    let (_tmp, mut ctx, id, _) = task_in_review();
    let now = started(&ctx, &id);
    let task = ctx.store.get(&id).unwrap();
    let attempt = &task
        .current_run()
        .unwrap()
        .review
        .as_ref()
        .unwrap()
        .identity
        .as_ref()
        .unwrap()
        .attempt;
    let error =
        corral_ops::verdict_bound(&ctx, &id, Verdict::Approve, "", Some(attempt)).unwrap_err();
    assert!(error.to_string().contains("a verdict needs notes"));
    ctx.store = Store::open(ctx.store.root()).unwrap();
    assert!(
        corral_ops::reconcile_review_at(&ctx, &id, now, &pane(AgentStatus::Working))
            .unwrap()
            .is_none()
    );
    let change = corral_ops::reconcile_review_at(&ctx, &id, now, &pane(AgentStatus::Idle))
        .unwrap()
        .unwrap();
    assert!(change.note.contains("verdict failed"));
    assert!(change.note.contains("a verdict needs notes"));
}

#[test]
fn superseded_callback_cannot_poison_active_review() {
    let (_tmp, ctx, id, _) = task_in_review();
    let now = started(&ctx, &id);
    corral_ops::verdict_bound(&ctx, &id, Verdict::Approve, "Reviewed", Some("old-attempt"))
        .unwrap_err();
    assert!(
        corral_ops::reconcile_review_at(&ctx, &id, now, &pane(AgentStatus::Idle))
            .unwrap()
            .is_none()
    );
    assert!(
        ctx.store
            .get(&id)
            .unwrap()
            .current_run()
            .unwrap()
            .blocked_excerpt
            .is_none()
    );
}

#[test]
fn idle_legacy_review_without_identity_or_accounting_child_is_recoverable() {
    let (_tmp, ctx, id, _) = task_in_review();
    let now = started(&ctx, &id);
    ctx.store
        .update(&id, |task| {
            let run = task.current_run_mut().unwrap();
            run.review.as_mut().unwrap().identity = None;
            run.review_runs.clear();
            Ok(())
        })
        .unwrap();
    assert!(
        corral_ops::reconcile_review_at(&ctx, &id, now, &pane(AgentStatus::Working))
            .unwrap()
            .is_none()
    );
    let change = corral_ops::reconcile_review_at(&ctx, &id, now, &pane(AgentStatus::Idle))
        .unwrap()
        .unwrap();
    assert!(change.note.contains("missing legacy review identity"));
    assert_eq!(ctx.store.get(&id).unwrap().status, Status::NeedsMe);
}

#[test]
fn completed_verdicts_are_not_treated_as_missing_callbacks() {
    let (_tmp, ctx, id, _) = task_in_review();
    let now = started(&ctx, &id);
    for verdict in [Verdict::Approve, Verdict::Changes, Verdict::Escalate] {
        ctx.store
            .update(&id, |task| {
                task.current_run_mut()
                    .unwrap()
                    .review
                    .as_mut()
                    .unwrap()
                    .verdict = Some(verdict);
                Ok(())
            })
            .unwrap();
        assert!(
            corral_ops::reconcile_review_at(&ctx, &id, after(now, 500), &HashMap::new())
                .unwrap()
                .is_none()
        );
        assert_eq!(ctx.store.get(&id).unwrap().status, Status::Review);
    }
}

#[test]
fn replacing_review_clears_old_idle_and_callback_failure() {
    let (_tmp, ctx, id, _) = task_in_review();
    let now = started(&ctx, &id);
    corral_ops::reconcile_review_at(&ctx, &id, now, &pane(AgentStatus::Idle)).unwrap();
    corral_ops::verdict(&ctx, &id, Verdict::Approve, "").unwrap_err();
    let replacement = started(&ctx, &id);
    let saved = ctx.store.get(&id).unwrap();
    let run = saved.current_run().unwrap();
    assert!(run.idle_since.is_none());
    assert!(run.blocked_excerpt.is_none());
    assert!(
        corral_ops::reconcile_review_at(&ctx, &id, replacement, &pane(AgentStatus::Idle))
            .unwrap()
            .is_none()
    );
}

#[test]
fn closed_reviewer_escalates_with_notes_location() {
    let (_tmp, ctx, id, _) = task_in_review();
    let now = started(&ctx, &id);
    let change = corral_ops::reconcile_review_at(&ctx, &id, now, &HashMap::new())
        .unwrap()
        .unwrap();
    assert!(change.note.contains("closed without a verdict"));
    assert!(
        change.note.contains(
            &corral_ops::review_notes_path(&ctx, &id, "R1")
                .display()
                .to_string()
        )
    );
}

#[test]
fn unbound_launch_is_given_one_timeout_window_before_escalation() {
    let (_tmp, ctx, id, _) = task_in_review();
    let now = started(&ctx, &id);
    ctx.store
        .update(&id, |task| {
            task.current_run_mut()
                .unwrap()
                .review
                .as_mut()
                .unwrap()
                .pane = None;
            Ok(())
        })
        .unwrap();
    assert!(
        corral_ops::reconcile_review_at(&ctx, &id, now, &HashMap::new())
            .unwrap()
            .is_none()
    );
    assert!(
        corral_ops::reconcile_review_at(&ctx, &id, after(now, 120), &HashMap::new())
            .unwrap()
            .is_some()
    );
}

#[test]
fn recent_omo_activity_clears_false_herdr_idle_stretch_without_sleeping() {
    let (_tmp, mut ctx, id, worktree) = task_in_review();
    reviewer_profile(&mut ctx, "true".into());
    let now = started(&ctx, &id);
    corral_ops::reconcile_review_at(&ctx, &id, now, &pane(AgentStatus::Idle)).unwrap();
    let file = std::fs::File::create(transcript_path(&ctx, &worktree)).unwrap();
    let modified = std::time::UNIX_EPOCH
        + std::time::Duration::from_secs(u64::try_from(after(now, 119).as_second()).unwrap());
    file.set_times(std::fs::FileTimes::new().set_modified(modified))
        .unwrap();
    assert!(
        corral_ops::reconcile_review_at(&ctx, &id, after(now, 120), &pane(AgentStatus::Idle))
            .unwrap()
            .is_none()
    );
    assert!(
        ctx.store
            .get(&id)
            .unwrap()
            .current_run()
            .unwrap()
            .idle_since
            .is_none()
    );
}

#[test]
fn refused_stale_verdict_cannot_leave_an_idle_reviewer_hanging() {
    let (_tmp, ctx, id, worktree) = task_in_review();
    let now = started(&ctx, &id);
    let notes = corral_ops::review_notes_path(&ctx, &id, "R1");
    std::fs::write(&notes, "Review findings retained before failed verdict.\n").unwrap();
    std::fs::write(worktree.join("spin.txt"), "changed candidate\n").unwrap();
    corral_git::commit_all(&worktree, "Move candidate after review").unwrap();
    let error = corral_ops::verdict(&ctx, &id, Verdict::Approve, "Reviewed").unwrap_err();
    assert!(error.to_string().contains("stale review"));
    let change = corral_ops::reconcile_review_at(&ctx, &id, now, &pane(AgentStatus::Idle))
        .unwrap()
        .unwrap();
    assert!(change.note.contains("stale review"));
    assert!(change.note.contains(&notes.display().to_string()));
}
