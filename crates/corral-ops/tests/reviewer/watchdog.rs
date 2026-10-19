use std::collections::HashMap;

use corral_core::{AutoStage, RunOutcome};
use corral_herdr::{AgentStatus, PaneInfo};
use corral_ops::ApproveStrategy;
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

fn inventory_without_reviewer() -> HashMap<String, PaneInfo> {
    let mut other = pane(AgentStatus::Unknown).remove("w1:p2").unwrap();
    other.pane_id = "w0:p1".into();
    other.workspace_id = "w0".into();
    other.tab_id = "w0:t1".into();
    HashMap::from([(other.pane_id.clone(), other)])
}

fn after(now: Timestamp, seconds: i64) -> Timestamp {
    now.checked_add(SignedDuration::from_secs(seconds)).unwrap()
}

#[test]
fn pending_background_check_survives_worker_idle_bound_and_completes_review() {
    // Given a reviewer with an outstanding check and an apparently idle pane.
    let (_tmp, mut ctx, id, worktree) = task_in_review();
    ctx.store.config.review.autopilot = Some(false);
    ctx.store.save_config().unwrap();
    let now = started(&ctx, &id);
    let closed = _tmp.path().join("closed");
    let before = std::fs::read_to_string(&closed).unwrap_or_default();
    let panes = pane(AgentStatus::Idle);
    std::thread::scope(|scope| {
        let (ready, started_check) = std::sync::mpsc::sync_channel(0);
        let (release, outstanding) = std::sync::mpsc::sync_channel(0);
        let check = scope.spawn(move || {
            ready.send(()).unwrap();
            outstanding
                .recv_timeout(std::time::Duration::from_secs(60))
                .unwrap();
            assert!(corral_git::is_clean(&worktree).unwrap());
            std::fs::read_to_string(worktree.join("spin.txt")).unwrap()
        });
        started_check
            .recv_timeout(std::time::Duration::from_secs(60))
            .unwrap();
        // When sync repeatedly observes the wait past the worker's 120-second bound.
        for seconds in [0, 120, 121, 600] {
            assert!(
                corral_ops::sync_at(&ctx, after(now, seconds))
                    .unwrap()
                    .iter()
                    .all(|change| change.to != Status::NeedsMe)
            );
            let task = ctx.store.get(&id).unwrap();
            let run = task.current_run().unwrap();
            assert_eq!(task.status, Status::Review);
            assert_eq!(run.idle_since, Some(now));
            assert!(run.review_runs.last().unwrap().ended.is_none());
            assert_eq!(std::fs::read_to_string(&closed).unwrap_or_default(), before);
        }
        // Then its actual check can complete and publish the attempt-bound verdict.
        release.send(()).unwrap();
        let result = check.join().unwrap();
        assert_eq!(result, "clockwise\n");
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
        corral_ops::verdict_bound(&ctx, &id, Verdict::Approve, &result, Some(attempt)).unwrap();
    });
    assert!(
        corral_ops::reconcile_review_at(&ctx, &id, after(now, 1801), &panes)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        ctx.store
            .get(&id)
            .unwrap()
            .current_run()
            .unwrap()
            .review
            .as_ref()
            .unwrap()
            .verdict,
        Some(Verdict::Approve)
    );
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
    let changes = corral_ops::sync_at(&ctx, after(now, 1800)).unwrap();
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
    assert_eq!(ctx.store.config.sync.review_idle_after_secs, 1800);
    for seconds in [119, 120, 1799] {
        assert!(
            corral_ops::reconcile_review_at(&ctx, &id, after(now, seconds), &panes)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            ctx.store
                .get(&id)
                .unwrap()
                .current_run()
                .unwrap()
                .idle_since,
            Some(now)
        );
    }
    let change = corral_ops::reconcile_review_at(&ctx, &id, after(now, 1800), &panes)
        .unwrap()
        .unwrap();
    assert_eq!(change.to, Status::NeedsMe);
    assert_eq!(change.stage, Some(AutoStage::Escalated));
    let saved = ctx.store.get(&id).unwrap();
    assert!(saved.escalated());
    let run = saved.current_run().unwrap();
    assert_eq!(run.review.as_ref().unwrap().verdict, None);
    assert_eq!(run.review_runs[0].ended, Some(after(now, 1800)));
    let reason = run.autopilot.as_ref().unwrap().reason.as_ref().unwrap();
    assert!(reason.contains("1800 seconds"));
    assert!(reason.contains(&notes.display().to_string()));
    assert_eq!(run.blocked_excerpt.as_ref(), Some(reason));
    assert_eq!(
        std::fs::read_to_string(&notes).unwrap(),
        "Finished reviewing; callback did not arrive.\n"
    );
    assert!(
        corral_ops::reconcile_review_at(&ctx, &id, after(now, 1801), &panes)
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
    ctx.store.config.sync.review_idle_after_secs = 30;
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
    corral_ops::reconcile_review_at(&ctx, &id, now, &pane(AgentStatus::Working)).unwrap();
    assert!(
        corral_ops::reconcile_review_at(&ctx, &id, after(now, 1), &inventory_without_reviewer())
            .unwrap()
            .is_none()
    );
    let change =
        corral_ops::reconcile_review_at(&ctx, &id, after(now, 2), &inventory_without_reviewer())
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
fn unseen_reviewer_uses_configured_startup_deadline_with_or_without_attached_pane() {
    for attached in [true, false] {
        // Given an unseen reviewer with a non-default startup window.
        let (_tmp, mut ctx, id, _) = task_in_review();
        ctx.store.config.sync.startup_grace_secs = 5;
        let now = started(&ctx, &id);
        if !attached {
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
        }
        // When inventories miss the pane repeatedly before and at its startup deadline.
        for seconds in [0, 1, 4] {
            assert!(
                corral_ops::reconcile_review_at(
                    &ctx,
                    &id,
                    after(now, seconds),
                    &inventory_without_reviewer()
                )
                .unwrap()
                .is_none()
            );
        }
        // Then the startup deadline, not the longer idle timeout, makes it actionable.
        assert!(
            corral_ops::reconcile_review_at(
                &ctx,
                &id,
                after(now, 5),
                &inventory_without_reviewer()
            )
            .unwrap()
            .is_some()
        );
    }
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

const FIXTURE_HEADER: &str = "2026-10-15T09:00:00.000Z";
const FIXTURE_ERROR: &str = "2026-10-15T09:10:00.000Z";

fn stall_log(name: &str, header: Timestamp, error: Timestamp) -> String {
    let path = format!(
        "{}/tests/fixtures/provider-stall/{name}.jsonl",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(path)
        .unwrap()
        .replace(FIXTURE_HEADER, &header.to_string())
        .replace(FIXTURE_ERROR, &error.to_string())
}

fn write_log(path: &std::path::Path, log: &str, modified: Timestamp) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, log).unwrap();
    let modified = std::time::UNIX_EPOCH
        + std::time::Duration::from_secs(u64::try_from(modified.as_second()).unwrap());
    std::fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(modified))
        .unwrap();
}

fn omp_reviewer(ctx: &mut Ctx) {
    reviewer_profile(ctx, "true".into());
    ctx.store.config.harnesses.insert(
        "omp".into(),
        serde_json::from_value(json!({"command": ": {model_args}; true"})).unwrap(),
    );
    ctx.store.config.profiles.get_mut("rev").unwrap().harness = "omp".into();
    ctx.store.config.sync.omp_agent_dir = Some(ctx.store.root().join("omp-agent"));
    ctx.store.save_config().unwrap();
}

fn retries(ctx: &Ctx) -> Vec<serde_json::Value> {
    let events = ctx.store.events().unwrap();
    events
        .iter()
        .filter(|e| e.kind == "provider_retry")
        .map(|e| e.detail.clone())
        .collect()
}

fn count(ctx: &Ctx, kind: &str) -> usize {
    let events = ctx.store.events().unwrap();
    events.iter().filter(|e| e.kind == kind).count()
}

/// Reviewer session under test: created one second after the review started, error one second later.
fn reviewer_log(name: &str, now: Timestamp) -> String {
    stall_log(name, after(now, 1), after(now, 2))
}

/// Ticks up to the last retry. The error is at now+2s, so retries are due at +62, +182 and +482.
fn retry_three_times(ctx: &Ctx, id: &str, now: Timestamp, panes: &HashMap<String, PaneInfo>) {
    let schedule = [
        (0, 0),
        (61, 0),
        (62, 1),
        (181, 1),
        (182, 2),
        (481, 2),
        (482, 3),
    ];
    for (secs, sent) in schedule {
        let change = corral_ops::reconcile_review_at(ctx, id, after(now, secs), panes).unwrap();
        assert!(change.is_none(), "t+{secs}");
        assert_eq!(retries(ctx).len(), sent, "t+{secs}");
        assert_eq!(
            ctx.store.get(id).unwrap().status,
            Status::Review,
            "t+{secs}"
        );
    }
}

#[test]
fn overloaded_reviewer_is_reprompted_then_interrupted_and_releases_the_barrier() {
    let (_tmp, mut ctx, id, worktree) = task_in_review();
    reviewer_profile(&mut ctx, "true".into());
    let now = started(&ctx, &id);
    let reviewer = transcript_path(&ctx, &worktree);
    write_log(&reviewer, &reviewer_log("overloaded", now), after(now, 1));
    // The implementer's session predates the review and keeps being written to.
    let implementer = reviewer.with_file_name("implementer.jsonl");
    let healthy = stall_log("recovered", after(now, -100), after(now, -90));
    write_log(&implementer, &healthy, after(now, 50));
    let (peer, _) = review_candidate(&ctx, "Peer task", "peer.txt");
    let panes = pane(AgentStatus::Idle);

    retry_three_times(&ctx, &id, now, &panes);
    for (index, detail) in retries(&ctx).iter().enumerate() {
        assert_eq!(detail["attempt"], index + 1);
        assert_eq!(detail["of"], 3);
        assert_eq!(detail["submitted"], true);
        assert_eq!(detail["pane"], "w1:p2");
    }
    let held = corral_ops::approve(&ctx, &peer, ApproveStrategy::Rebase, false, false);
    let held = held.unwrap_err().to_string();
    assert!(held.contains("review in progress"), "{held}");

    assert!(
        corral_ops::reconcile_review_at(&ctx, &id, after(now, 781), &panes)
            .unwrap()
            .is_none()
    );
    let change = corral_ops::reconcile_review_at(&ctx, &id, after(now, 782), &panes)
        .unwrap()
        .unwrap();
    assert_eq!(change.to, Status::NeedsMe);
    assert!(
        change.note.contains("provider overloaded since "),
        "{}",
        change.note
    );
    let saved = ctx.store.get(&id).unwrap();
    assert_eq!(saved.status, Status::NeedsMe);
    let run = saved.current_run().unwrap();
    assert_eq!(run.review_runs[0].ended, Some(after(now, 782)));
    assert_eq!(run.review.as_ref().unwrap().verdict, None);
    assert_eq!(run.review_runs[0].outcome, Some(RunOutcome::Interrupted));
    assert!(run.review_interrupted());
    for kind in [
        "provider_escalated",
        "review_interrupted",
        "review_escalated",
    ] {
        assert_eq!(count(&ctx, kind), 1, "{kind}");
    }
    if let Err(error) = corral_ops::approve(&ctx, &peer, ApproveStrategy::Rebase, false, false) {
        let error = error.to_string();
        assert!(!error.contains("review in progress"), "{error}");
    }
}

#[test]
fn a_reviewer_still_working_after_the_last_retry_is_not_interrupted() {
    let (_tmp, mut ctx, id, worktree) = task_in_review();
    reviewer_profile(&mut ctx, "true".into());
    let now = started(&ctx, &id);
    let log = transcript_path(&ctx, &worktree);
    write_log(&log, &reviewer_log("overloaded", now), after(now, 1));
    let idle = pane(AgentStatus::Idle);
    let working = pane(AgentStatus::Working);
    retry_three_times(&ctx, &id, now, &idle);
    for secs in [782, 900] {
        let change = corral_ops::reconcile_review_at(&ctx, &id, after(now, secs), &working);
        assert!(change.unwrap().is_none(), "t+{secs}");
    }
    assert_eq!(ctx.store.get(&id).unwrap().status, Status::Review);
    assert_eq!(count(&ctx, "provider_escalated"), 0);
    write_log(&log, &reviewer_log("recovered", now), after(now, 901));
    let change = corral_ops::reconcile_review_at(&ctx, &id, after(now, 902), &idle).unwrap();
    assert!(change.is_none());
    assert_eq!(count(&ctx, "provider_recovered"), 1);
    assert_eq!(count(&ctx, "provider_escalated"), 0);
    assert_eq!(ctx.store.get(&id).unwrap().status, Status::Review);
}

#[test]
fn a_reviewer_still_working_after_the_last_retry_is_interrupted_when_it_fails_again() {
    let (_tmp, mut ctx, id, worktree) = task_in_review();
    reviewer_profile(&mut ctx, "true".into());
    let now = started(&ctx, &id);
    let log = transcript_path(&ctx, &worktree);
    write_log(&log, &reviewer_log("overloaded", now), after(now, 1));
    let idle = pane(AgentStatus::Idle);
    retry_three_times(&ctx, &id, now, &idle);
    let failed = stall_log("retry-failed", after(now, 1), after(now, 2))
        .replace("2026-10-15T09:18:20.000Z", &after(now, 500).to_string());
    write_log(&log, &failed, after(now, 500));
    let working = pane(AgentStatus::Working);
    let change = corral_ops::reconcile_review_at(&ctx, &id, after(now, 510), &working);
    assert!(change.unwrap().is_none());
    assert_eq!(ctx.store.get(&id).unwrap().status, Status::Review);
    let change = corral_ops::reconcile_review_at(&ctx, &id, after(now, 520), &idle).unwrap();
    assert_eq!(change.unwrap().to, Status::NeedsMe);
}

#[test]
fn omp_reviewer_log_is_retried_and_interrupted() {
    let (_tmp, mut ctx, id, worktree) = task_in_review();
    omp_reviewer(&mut ctx);
    let now = started(&ctx, &id);
    let log = reviewer_log("omp-overloaded", now).replace(
        "/home/pawis/miti-workspace/corral/.worktrees/T-001",
        &worktree.to_string_lossy(),
    );
    let slug = worktree
        .to_string_lossy()
        .trim_start_matches('/')
        .replace('/', "-");
    let path = ctx
        .store
        .root()
        .join("omp-agent/sessions")
        .join(format!("--{slug}--"))
        .join("session.jsonl");
    write_log(&path, &log, after(now, 1));
    let panes = pane(AgentStatus::Idle);
    retry_three_times(&ctx, &id, now, &panes);
    let change = corral_ops::reconcile_review_at(&ctx, &id, after(now, 782), &panes).unwrap();
    assert_eq!(change.unwrap().to, Status::NeedsMe);
    assert_eq!(count(&ctx, "provider_escalated"), 1);
}

#[test]
fn a_reviewer_ignores_errors_in_sessions_created_before_it_started() {
    let (_tmp, mut ctx, id, worktree) = task_in_review();
    reviewer_profile(&mut ctx, "true".into());
    let now = started(&ctx, &id);
    let stale = stall_log("overloaded", after(now, -100), after(now, -90));
    write_log(&transcript_path(&ctx, &worktree), &stale, after(now, 1));
    let panes = pane(AgentStatus::Idle);
    for secs in [0, 62, 400] {
        let change = corral_ops::reconcile_review_at(&ctx, &id, after(now, secs), &panes);
        assert!(change.unwrap().is_none(), "t+{secs}");
    }
    assert!(retries(&ctx).is_empty());
}

fn pm_pane(status: AgentStatus) -> HashMap<String, PaneInfo> {
    let mut info = pane(status).remove("w1:p2").unwrap();
    info.pane_id = "w9:p9".into();
    HashMap::from([(info.pane_id.clone(), info)])
}

fn pm_review(ctx: &mut Ctx, id: &str) -> Timestamp {
    reviewer_profile(ctx, "true".into());
    ctx.store.config.pm.agent = "rev".into();
    std::fs::write(
        ctx.store.state_dir().join("pm.json"),
        r#"{"pane_id":"w9:p9"}"#,
    )
    .unwrap();
    corral_ops::review_run(ctx, id, Some("pm")).unwrap().at
}

#[test]
fn a_reused_pm_session_is_retried_and_interrupted_with_the_real_reason() {
    let (_tmp, mut ctx, id, _worktree) = task_in_review();
    let now = pm_review(&mut ctx, &id);
    // The live PM session was created long before the review request; its error is after it.
    let live = transcript_path(&ctx, ctx.store.root());
    write_log(
        &live,
        &stall_log("overloaded", after(now, -3600), after(now, 2)),
        after(now, 2),
    );
    // Another PM session in the same directory is idle and has an old overload of its own.
    let old = live.with_file_name("old.jsonl");
    write_log(
        &old,
        &stall_log("overloaded", after(now, -7200), after(now, -7190)),
        after(now, -100),
    );
    let panes = pm_pane(AgentStatus::Idle);

    retry_three_times(&ctx, &id, now, &panes);
    for detail in retries(&ctx) {
        assert_eq!(detail["pane"], "w9:p9");
        assert_eq!(detail["submitted"], true);
    }
    let change = corral_ops::reconcile_review_at(&ctx, &id, after(now, 782), &panes)
        .unwrap()
        .unwrap();
    assert_eq!(change.to, Status::NeedsMe);
    assert!(
        change.note.contains("provider overloaded since "),
        "{}",
        change.note
    );
    let saved = ctx.store.get(&id).unwrap();
    let run = saved.current_run().unwrap();
    assert!(run.review_interrupted());
    assert_eq!(run.review_runs[0].outcome, Some(RunOutcome::Interrupted));
    assert_eq!(count(&ctx, "provider_escalated"), 1);
}

#[test]
fn a_pm_review_ignores_errors_logged_before_the_request() {
    let (_tmp, mut ctx, id, _worktree) = task_in_review();
    let now = pm_review(&mut ctx, &id);
    let live = transcript_path(&ctx, ctx.store.root());
    write_log(
        &live,
        &stall_log("overloaded", after(now, -3600), after(now, -90)),
        after(now, 2),
    );
    let panes = pm_pane(AgentStatus::Idle);
    for secs in [0, 62, 400] {
        let change = corral_ops::reconcile_review_at(&ctx, &id, after(now, secs), &panes);
        assert!(change.unwrap().is_none(), "t+{secs}");
    }
    assert!(retries(&ctx).is_empty());
}

#[test]
fn a_busy_delivery_lock_defers_the_reviewer_retry_without_consuming_an_attempt() {
    let (_tmp, mut ctx, id, worktree) = task_in_review();
    reviewer_profile(&mut ctx, "true".into());
    let now = started(&ctx, &id);
    write_log(
        &transcript_path(&ctx, &worktree),
        &reviewer_log("overloaded", now),
        after(now, 1),
    );
    let panes = pane(AgentStatus::Idle);
    corral_ops::reconcile_review_at(&ctx, &id, now, &panes).unwrap();
    let held = ctx
        .store
        .try_lock(&format!("delivery-{id}"))
        .unwrap()
        .unwrap();
    for secs in [62, 100] {
        let change = corral_ops::reconcile_review_at(&ctx, &id, after(now, secs), &panes);
        assert!(change.unwrap().is_none(), "t+{secs}");
    }
    assert!(retries(&ctx).is_empty());
    drop(held);
    let change = corral_ops::reconcile_review_at(&ctx, &id, after(now, 101), &panes);
    assert!(change.unwrap().is_none());
    let sent = retries(&ctx);
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0]["attempt"], 1);
}

#[test]
fn two_concurrent_due_ticks_submit_one_reviewer_retry() {
    let (_tmp, mut ctx, id, worktree) = task_in_review();
    reviewer_profile(&mut ctx, "true".into());
    let now = started(&ctx, &id);
    write_log(
        &transcript_path(&ctx, &worktree),
        &reviewer_log("overloaded", now),
        after(now, 1),
    );
    let panes = pane(AgentStatus::Idle);
    corral_ops::reconcile_review_at(&ctx, &id, now, &panes).unwrap();
    let barrier = std::sync::Barrier::new(2);
    let ticks = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..2)
            .map(|_| {
                let (ctx, panes, id) = (ctx.clone(), panes.clone(), id.clone());
                let (barrier, due) = (&barrier, after(now, 62));
                scope.spawn(move || {
                    barrier.wait();
                    corral_ops::reconcile_review_at(&ctx, &id, due, &panes).is_ok()
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert!(ticks.contains(&true), "no tick completed");
    let sent = retries(&ctx);
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0]["attempt"], 1);
}
