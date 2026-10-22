//! Durable-boundary regressions: each test restores the exact persisted state a crash or a
//! racing worker leaves between two writes, then syncs once with an injected clock.

use corral_core::{AttemptFailure, AvailabilityClass, AvailabilityEpisode};
use serde_json::json;

use super::review::{in_review, reviewer_candidates};
use super::*;

fn pending_episode(fx: &Fx) {
    let run = fx.run();
    let metadata = fx.ctx.store.config.profiles["general"]
        .metadata
        .as_ref()
        .unwrap();
    let mut episode = AvailabilityEpisode::new(
        "T-001-R1-implementation",
        &fx.id,
        ProfileRole::Implementation,
        "general",
        metadata,
        fx.at(600),
    );
    episode
        .seed(
            &metadata.candidates[0],
            Some(&run.id),
            "preferred",
            fx.at(0),
        )
        .unwrap();
    episode.record_failure(&run.id, failure(fx.at(10))).unwrap();
    episode
        .advance(&metadata.candidates[1], "selected fallback", fx.at(600))
        .unwrap();
    fx.write_state(
        "availability",
        &episode.id,
        &serde_json::to_value(&episode).unwrap(),
    );
}

fn failure(at: Timestamp) -> AttemptFailure {
    AttemptFailure {
        class: AvailabilityClass::Unavailable,
        message: "model_not_found".into(),
        retries: 0,
        at,
        recovered_at: None,
    }
}

fn stall(fx: &Fx, key: &str, since: Timestamp, selected: bool) {
    fx.write_state(
        "provider-stall",
        key,
        &json!({
            "since": since, "error_at": since, "message": "model_not_found",
            "attempts": 0, "last_attempt": null, "escalated": true,
            "class": {"kind": "unavailable"}, "fallback_selected": selected
        }),
    );
}

#[test]
fn a_selection_persisted_before_its_stall_flag_resumes_after_restart() {
    // Given the episode's durable selection without the stall owner's selection flag.
    let fx = Fx::new(Setup::default());
    fx.checkpoint();
    fx.fail_turn(&fx.run(), "s1.jsonl", "model_not_found", 10);
    pending_episode(&fx);
    stall(&fx, "T-001-R1", fx.at(10), false);
    // When the restarted sync runs.
    let task = fx.sync(900);
    // Then the recorded candidate continues, without a second selection.
    assert_eq!(task.runs.len(), 2);
    assert_eq!(binding(task.current_run().unwrap()).candidate, "opus");
    assert!(fx.events("availability_fallback_selected").is_empty());
}

#[test]
fn an_unassessed_escalation_is_assessed_again_after_restart() {
    // Given the escalation recorded before its assessment was ever written.
    let fx = Fx::new(Setup::default());
    fx.checkpoint();
    let first = fx.run();
    fx.fail_turn(&first, "s1.jsonl", MODEL_QUOTA, 10);
    stall(
        &fx,
        "T-001-R1",
        first.started + SignedDuration::from_secs(10),
        false,
    );
    // When the restarted sync runs.
    let task = fx.sync(900);
    // Then the owner is told why, rather than the stall being stranded silently.
    assert_eq!(task.runs.len(), 1);
    assert_eq!(task.status, Status::NeedsMe);
}

#[test]
fn a_saved_selection_never_replaces_a_worker_that_is_working_again() {
    // Given a restored selection while herdr reports the old worker working.
    let fx = Fx::new(Setup::default());
    fx.checkpoint();
    fx.fail_turn(&fx.run(), "s1.jsonl", "model_not_found", 10);
    pending_episode(&fx);
    stall(&fx, "T-001-R1", fx.at(10), true);
    fx.pane_status("working");
    // When the executor rechecks liveness under its claim.
    let task = fx.sync(900);
    // Then nothing is replaced until the worker is quiescent again.
    assert_eq!(task.runs.len(), 1);
    assert_eq!(task.status, Status::Running);
    fx.pane_status("idle");
    assert_eq!(fx.sync(1000).runs.len(), 2);
}

#[test]
fn a_saved_selection_never_replaces_a_worker_whose_session_resumed() {
    // Given a restored selection, but the old session answered after its failure.
    let fx = Fx::new(Setup::default());
    fx.checkpoint();
    let first = fx.run();
    fx.fail_turn(&first, "s1.jsonl", "model_not_found", 10);
    fx.answer(&first, "s1.jsonl", 20);
    pending_episode(&fx);
    let session = fx.ctx.store.config.sync.omo_agent_dir.clone().unwrap();
    let body = first
        .worktree
        .to_string_lossy()
        .trim_start_matches('/')
        .replace(['/', '\\', ':'], "-");
    let mut state = json!({
        "since": fx.at(10), "error_at": fx.at(10), "message": "model_not_found",
        "attempts": 0, "last_attempt": null, "escalated": true,
        "class": {"kind": "unavailable"}, "fallback_selected": true,
    });
    state["session"] = json!(session.join(format!("sessions/--{body}--/s1.jsonl")));
    fx.write_state("provider-stall", "T-001-R1", &state);
    // When the executor rechecks the stall.
    let task = fx.sync(900);
    // Then the resumed worker keeps its run.
    assert_eq!(task.runs.len(), 1);
}

#[test]
fn a_lost_old_pane_never_waives_the_handoff() {
    // Given a restored selection, no checkpoint, and the old pane gone.
    let fx = Fx::new(Setup::default());
    fx.fail_turn(&fx.run(), "s1.jsonl", "model_not_found", 10);
    pending_episode(&fx);
    stall(&fx, "T-001-R1", fx.at(10), true);
    std::fs::write(fx.bin_dir.join("gone"), "w1:p1\n").unwrap();
    // When sync runs.
    let task = fx.sync(900);
    // Then the original run stays current and the owner decides.
    assert_eq!(task.runs.len(), 1);
    assert_eq!(task.status, Status::NeedsMe);
}

fn reviewers() -> Vec<ExecutionCandidate> {
    reviewer_candidates(&[("rv-claude", "anthropic"), ("rv-gemini", "google")])
}

#[test]
fn an_interrupted_reviewer_handoff_resumes_after_restart() {
    // Given the reviewer child marked Interrupted before its replacement launched.
    let fx = in_review(reviewers());
    let run = fx.run();
    let child = run.review_runs[0].clone();
    fx.fail_turn(&child, "r1.jsonl", "model_not_found", 10);
    let metadata = fx.ctx.store.config.profiles["review-general"]
        .metadata
        .as_ref()
        .unwrap();
    let mut episode = AvailabilityEpisode::new(
        "T-001-R1-review-review-general",
        &fx.id,
        ProfileRole::ImplementationReview,
        "review-general",
        metadata,
        fx.at(600),
    );
    episode
        .seed(
            &metadata.candidates[0],
            Some(&child.id),
            "preferred",
            fx.at(0),
        )
        .unwrap();
    episode
        .record_failure(&child.id, failure(child.started))
        .unwrap();
    episode
        .advance(&metadata.candidates[1], "selected reviewer", fx.at(600))
        .unwrap();
    fx.write_state(
        "availability",
        &episode.id,
        &serde_json::to_value(&episode).unwrap(),
    );
    stall(&fx, &format!("T-001-{}", child.id), child.started, true);
    fx.ctx
        .store
        .update(&fx.id, |task| {
            task.current_run_mut().unwrap().review_runs[0].outcome = Some(RunOutcome::Interrupted);
            Ok(())
        })
        .unwrap();
    // When the restarted sync runs.
    let task = fx.sync(900);
    // Then the selected reviewer continuation launches.
    let run = task.current_run().unwrap();
    assert_eq!(task.status, Status::Review);
    assert_eq!(run.review_runs.len(), 2);
    assert_eq!(
        run.review_runs[1].model.as_deref(),
        Some("google/rv-gemini-fixture")
    );
}

#[test]
fn an_explicit_reviewer_profile_survives_its_fallback() {
    // Given an explicitly chosen reviewer profile with its own instructions.
    let mut fx = in_review(reviewers());
    let mut pinned = fx.ctx.store.config.profiles["review-general"].clone();
    pinned.metadata.as_mut().unwrap().instructions = "Pinned review instructions".into();
    fx.ctx
        .store
        .config
        .profiles
        .insert("pinned-review".into(), pinned);
    fx.ctx.store.save_config().unwrap();
    corral_ops::review_run(&fx.ctx, &fx.id, Some("pinned-review")).unwrap();
    let child = fx.run().review_runs.last().unwrap().clone();
    assert_eq!(child.agent, "pinned-review");
    // When its provider fails.
    fx.fail_turn(&child, "r2.jsonl", "model_not_found", 10);
    let task = fx.sync(900);
    // Then the replacement keeps that profile, on its next approved candidate.
    let actual = task.current_run().unwrap().review_runs.last().unwrap();
    assert_eq!(actual.agent, "pinned-review");
    assert_eq!(actual.model.as_deref(), Some("google/rv-gemini-fixture"));
}

#[test]
fn a_reviewer_rejected_before_start_is_never_retried_in_its_episode() {
    // Given a reviewer whose preferred model is unavailable before start.
    let mut fx = in_review(reviewers());
    let profile = fx
        .ctx
        .store
        .config
        .profiles
        .get_mut("review-general")
        .unwrap();
    profile.model = Some("anthropic/missing-fixture".into());
    profile.metadata.as_mut().unwrap().candidates[0].model = "anthropic/missing-fixture".into();
    fx.ctx.store.save_config().unwrap();
    corral_ops::review_run(&fx.ctx, &fx.id, Some("review-general")).unwrap();
    let child = fx.run().review_runs.last().unwrap().clone();
    assert_eq!(child.model.as_deref(), Some("google/rv-gemini-fixture"));
    // When the selected fallback reviewer fails too.
    fx.fail_turn(&child, "r2.jsonl", "model_not_found", 10);
    let task = fx.sync(900);
    // Then the episode is exhausted instead of cycling back.
    assert_eq!(task.status, Status::NeedsMe);
    let last = task.current_run().unwrap().review_runs.last().unwrap();
    assert_ne!(last.model.as_deref(), Some("anthropic/missing-fixture"));
}

fn enable_only_unrelated(fx: &Fx) {
    let agent_dir = fx.ctx.store.config.sync.omo_agent_dir.as_ref().unwrap();
    std::fs::write(
        agent_dir.join("settings.json"),
        r#"{"enabledModels":["openai/unrelated-fixture"]}"#,
    )
    .unwrap();
}

#[test]
fn a_pm_with_every_candidate_unavailable_does_not_open() {
    // Given a PM profile whose approved candidates are all unavailable.
    let mut fx = Fx::new(Setup::default());
    let mut pm = fx.ctx.store.config.profiles["general"].clone();
    pm.metadata.as_mut().unwrap().eligibility.roles = [ProfileRole::Pm].into();
    fx.ctx
        .store
        .config
        .profiles
        .insert("coordinator".into(), pm);
    fx.ctx.store.config.pm.agent = "coordinator".into();
    fx.ctx.store.save_config().unwrap();
    enable_only_unrelated(&fx);
    // When the PM is opened.
    let result = corral_ops::open_pm(&fx.ctx);
    // Then exhaustion is reported with the rejected candidates, and nothing launches.
    let error = format!("{:#}", result.unwrap_err());
    assert!(error.contains("cannot start"), "{error}");
    assert!(error.contains("opus"), "{error}");
}

#[test]
fn a_reviewer_with_every_candidate_unavailable_does_not_start() {
    // Given a review whose approved reviewer candidates are all unavailable.
    let fx = in_review(reviewers());
    enable_only_unrelated(&fx);
    let before = fx.run().review_runs.len();
    // When another review is requested.
    let result = corral_ops::review_run(&fx.ctx, &fx.id, Some("review-general"));
    // Then it is refused with the reason and no reviewer child is created.
    let error = format!("{:#}", result.unwrap_err());
    assert!(error.contains("cannot start"), "{error}");
    assert_eq!(fx.run().review_runs.len(), before);
}

#[test]
fn a_reviewer_that_recovers_after_selection_cancels_it_and_is_watched_again() {
    // Given a selected reviewer fallback, then the original reviewer answers normally.
    let mut fx = in_review(reviewers());
    fx.ctx.store.config.sync.review_idle_after_secs = 1;
    fx.ctx.store.config.sync.idle_after_secs = 1;
    fx.ctx.store.save_config().unwrap();
    let child = fx.run().review_runs[0].clone();
    fx.fail_turn(&child, "r1.jsonl", "model_not_found", 10);
    let metadata = fx.ctx.store.config.profiles["review-general"]
        .metadata
        .as_ref()
        .unwrap();
    let mut episode = AvailabilityEpisode::new(
        "T-001-R1-review-review-general",
        &fx.id,
        ProfileRole::ImplementationReview,
        "review-general",
        metadata,
        fx.at(600),
    );
    episode
        .seed(
            &metadata.candidates[0],
            Some(&child.id),
            "preferred",
            child.started,
        )
        .unwrap();
    episode
        .record_failure(&child.id, failure(child.started))
        .unwrap();
    episode
        .advance(&metadata.candidates[1], "selected reviewer", fx.at(600))
        .unwrap();
    fx.write_state(
        "availability",
        &episode.id,
        &serde_json::to_value(&episode).unwrap(),
    );
    let body = child
        .worktree
        .to_string_lossy()
        .trim_start_matches('/')
        .replace(['/', '\\', ':'], "-");
    let session = fx.ctx.store.config.sync.omo_agent_dir.clone().unwrap();
    let mut state = json!({
        "since": child.started, "error_at": child.started, "message": "model_not_found",
        "attempts": 0, "last_attempt": null, "escalated": true,
        "class": {"kind": "unavailable"}, "fallback_selected": true,
    });
    state["session"] = json!(session.join(format!("sessions/--{body}--/r1.jsonl")));
    fx.write_state("provider-stall", &format!("T-001-{}", child.id), &state);
    fx.answer(&child, "r1.jsonl", 20);
    // When syncs pass the recovery and the watchdog windows.
    fx.sync(900);
    let task = fx.sync(2000);
    // Then the unlaunched selection is cancelled and ordinary review monitoring resumes.
    let recorded: AvailabilityEpisode = serde_json::from_value(fx.episode(&episode.id)).unwrap();
    assert!(recorded.pending().is_none());
    assert_eq!(recorded.attempts.len(), 1);
    assert_eq!(task.current_run().unwrap().review_runs.len(), 1);
    assert_eq!(task.status, Status::NeedsMe);
}
