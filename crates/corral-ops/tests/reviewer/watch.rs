use super::*;
use corral_ops::watch::{ProgressState, load};

#[test]
fn verdict_and_transcript_survive_replacement_and_send_back() {
    // Given a reviewer emitting a real transcript with reported cost.
    let (_tmp, mut ctx, id, worktree) = task_in_review();
    let path = transcript_path(&ctx, &worktree);
    let header = json!({"type":"session","cwd":worktree}).to_string();
    reviewer_profile(
        &mut ctx,
        format!(
            r#"printf '%s\n' '{header}' > '{path}'; printf '{{"type":"message","timestamp":"%s","message":{{"role":"assistant","content":"First review","usage":{{"input":13,"output":7,"cost":0.25}}}}}}\n' "$(date -u +%Y-%m-%dT%H:%M:%S.%NZ)" >> '{path}'"#,
            path = path.display()
        ),
    );
    corral_ops::review_run(&ctx, &id, Some("rev")).unwrap();
    corral_ops::verdict(&ctx, &id, Verdict::Changes, "First notes").unwrap();
    ctx.store
        .update(&id, |task| {
            task.rounds = 1;
            Ok(())
        })
        .unwrap();
    corral_ops::review_run(&ctx, &id, Some("rev")).unwrap();
    let active = load(&ctx, &id).unwrap();
    assert_eq!(active.reviews.len(), 2);
    assert!(active.reviews[0].live_pane.is_none());
    assert!(active.reviews[1].live_pane.is_some());
    let second_notes = corral_ops::review_notes_path(&ctx, &id, "R1");
    std::fs::write(&second_notes, "Partial second notes").unwrap();
    // When send-back cleans up the second review's pane.
    corral_ops::request_changes(&ctx, &id, "Please fix this.").unwrap();
    let history = load(&ctx, &id).unwrap();
    // Then each attempt owns its receipt, launch accounting, and retained fallback.
    let first = &history.reviews[0];
    assert_eq!(first.accounting.id, "R1-review1");
    assert_eq!(first.round, 0);
    assert_eq!(first.state, ProgressState::Failed);
    assert_eq!(
        first.record.as_ref().unwrap().notes.as_deref(),
        Some("First notes")
    );
    assert_eq!(first.accounting.agent, "rev");
    assert_eq!(first.accounting.model.as_deref(), Some("review-model"));
    assert_eq!(first.accounting.usage.as_ref().unwrap().input, 13);
    assert!(first.accounting.ended.is_some());
    assert_eq!(first.artifacts.len(), 2);
    assert_eq!(first.artifacts[0].kind, corral_core::ArtifactKind::Log);
    // Titles route the UI's fallback type and order.
    assert_eq!(first.artifacts[0].title, "Reviewer transcript (R1-review1)");
    assert_eq!(first.artifacts[1].title, "Reviewer screen (R1-review1)");
    let corral_ops::Preview::Text(transcript) =
        corral_ops::artifact_document(&ctx, &first.artifacts[0]).unwrap()
    else {
        panic!("expected transcript text")
    };
    assert!(transcript.contains("First review"));
    assert!(transcript.contains("0.25"));
    let second = &history.reviews[1];
    assert_eq!(second.round, 1);
    assert_eq!(second.state, ProgressState::Interrupted);
    assert_eq!(
        second.artifacts.last().unwrap().title,
        "Reviewer screen (R1-review2)"
    );
    assert_eq!(
        second.record.as_ref().unwrap().notes.as_deref(),
        Some("Partial second notes")
    );
    assert!(history.reviews.iter().all(|r| r.live_pane.is_none()));
    assert_ne!(second.accounting.id, first.accounting.id);
}

#[test]
fn superseded_review_retains_partial_notes_without_a_verdict() {
    // Given an active reviewer with partial notes.
    let (_tmp, ctx, id, _worktree) = task_in_review();
    corral_ops::review_run(&ctx, &id, Some("rev")).unwrap();
    let notes = corral_ops::review_notes_path(&ctx, &id, "R1");
    std::fs::write(&notes, "Unfinished findings").unwrap();
    // When a new reviewer replaces it.
    corral_ops::review_run(&ctx, &id, Some("rev")).unwrap();
    // Then the old notes and screen survive, without inventing a verdict.
    let history = load(&ctx, &id).unwrap();
    let old = &history.reviews[0];
    assert_eq!(old.state, ProgressState::Interrupted);
    assert!(old.record.as_ref().unwrap().verdict.is_none());
    assert_eq!(
        old.record.as_ref().unwrap().notes.as_deref(),
        Some("Unfinished findings")
    );
    assert!(old.live_pane.is_none());
    assert_eq!(old.artifacts.len(), 1);
    assert!(
        !notes.exists(),
        "replacement must not inherit the old notes file"
    );
}

#[test]
fn offline_herdr_does_not_hide_stored_gate_or_reviewer_history() {
    // Given readable gate history and an active review.
    let (_tmp, mut ctx, id, _worktree) = task_in_review();
    corral_ops::review_run(&ctx, &id, Some("rev")).unwrap();
    let before = ctx.store.get(&id).unwrap();
    ctx.herdr = Herdr::with_bin(ctx.store.root().join("herdr-unavailable"), None);
    // When the live-pane probe cannot reach herdr.
    let history = load(&ctx, &id).unwrap();
    // Then stored history and logs remain readable, without changing verification.
    assert_eq!(history.gates.len(), 1);
    assert_eq!(history.reviews.len(), 1);
    assert!(history.reviews[0].live_pane.is_none());
    assert_eq!(history.reviews[0].state, ProgressState::Running);
    assert!(
        !corral_ops::watch::read_gate_log(&ctx, &history.gates[0], 0, 100)
            .unwrap()
            .is_empty()
    );
    assert_eq!(before, ctx.store.get(&id).unwrap());
}

#[test]
fn missing_reviewer_pane_is_persistently_interrupted_without_a_verdict() {
    // Given an active reviewer and a successful inventory response with no pane.
    let (_tmp, mut ctx, id, _worktree) = task_in_review();
    corral_ops::review_run(&ctx, &id, Some("rev")).unwrap();
    let before = ctx.store.get(&id).unwrap();
    let bin = ctx.store.root().join("empty-herdr");
    std::fs::write(
        &bin,
        "#!/bin/sh\nprintf '%s\\n' '{\"id\":\"fake\",\"result\":{}}'\n",
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    ctx.herdr = Herdr::with_bin(bin, None);
    // When the monitor observes process loss.
    let history = load(&ctx, &id).unwrap();
    let repeated = load(&ctx, &id).unwrap();
    // Then the observation is terminal and stable, but grants no verdict.
    let review = &history.reviews[0];
    assert_eq!(review.state, ProgressState::Interrupted);
    assert!(review.live_pane.is_none());
    assert!(review.record.as_ref().unwrap().verdict.is_none());
    assert!(review.accounting.ended.is_some());
    assert_eq!(
        review.accounting.ended,
        repeated.reviews[0].accounting.ended
    );
    assert_eq!(before, ctx.store.get(&id).unwrap());
}

#[test]
fn foreign_monitor_reads_history_without_probing_or_reconciling_panes() {
    let (_tmp, mut ctx, id, _worktree) = task_in_review();
    corral_ops::review_run(&ctx, &id, Some("rev")).unwrap();
    let before = ctx.store.get(&id).unwrap();
    let marker = ctx.store.root().join("foreign-probe");
    let bin = ctx.store.root().join("foreign-herdr");
    std::fs::write(
        &bin,
        format!(
            "#!/bin/sh\nprintf touched > '{}'\nexit 1\n",
            marker.display()
        ),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    ctx.herdr = Herdr::with_bin(bin, Some("foreign".into()));

    let history = load(&ctx, &id).unwrap();

    assert!(!marker.exists(), "foreign sessions must not probe pane IDs");
    assert_eq!(history.reviews.len(), 1);
    assert!(history.reviews[0].live_pane.is_none());
    assert_eq!(history.reviews[0].state, ProgressState::Running);
    assert!(history.reviews[0].accounting.ended.is_none());
    assert_eq!(before, ctx.store.get(&id).unwrap());
}
