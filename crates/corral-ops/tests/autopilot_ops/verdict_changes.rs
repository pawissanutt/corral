use super::*;

const NOTES: &str = "Reproduce P2 with an empty widget.\nReject it before indexing.";

fn configured(max_rounds: u32) -> Fx {
    let fx = Fx::new(|r| {
        r.gate = gate(&["true"]);
        r.reviewer = Some("rev".into());
        r.accept = Some(Accept::Auto);
        r.max_rounds = Some(max_rounds);
        r.remove_worktree = Some(false);
    });
    fx.change("widget.txt", "defective\n");
    fx.done();
    fx.sync();
    assert_eq!(fx.stage(), Some(AutoStage::Reviewing));
    fx
}

#[test]
fn changes_loop_back_when_another_reviewed_task_lands_before_sync() {
    // Given two reviewed candidates, with changes recorded before the other lands.
    let mut fx = configured(2);
    let candidate_id = fx.id.clone();
    let candidate_sha = fx.run().review.unwrap().sha;
    let other = fx
        .ctx
        .store
        .create("Unrelated work", Some("app"), Status::Ready, "")
        .unwrap();
    corral_ops::dispatch(
        &fx.ctx,
        &other.id,
        &DispatchOpts {
            profile: Some("fake".into()),
            ..Default::default()
        },
    )
    .unwrap();
    fx.id = other.id;
    fx.change("unrelated.txt", "other task\n");
    fx.done();
    fx.sync();
    corral_ops::verdict(&fx.ctx, &candidate_id, Verdict::Changes, NOTES).unwrap();
    fx.verdict(Verdict::Approve, "Ready to land.");
    corral_ops::approve(&fx.ctx, &fx.id, ApproveStrategy::Rebase, true, false).unwrap();
    assert_eq!(fx.task().status, Status::Done);
    fx.id = candidate_id;
    let run = fx.run();
    let reviewed_base = run.review.as_ref().unwrap().identity.as_ref().unwrap();
    assert_ne!(
        git(&fx.app, &["rev-parse", "main"]),
        reviewed_base.integration_base
    );
    let gates = fx.events("gate_started");
    let reviews = fx.events("review_started");
    let artifacts = fx.task().artifacts.len();

    // When sync observes the recorded changes on the unchanged candidate.
    let task = fx.sync();

    // Then it sends the reviewer notes, consumes one round, and starts no gate/review.
    assert_eq!(task.status, Status::Running, "{}", fx.reason());
    assert_eq!(task.rounds, 1);
    assert_eq!(fx.stage(), Some(AutoStage::Changes));
    assert_eq!(fx.run().id, run.id);
    assert_eq!(fx.run().outcome, None);
    assert_eq!(
        git(&fx.run().worktree, &["rev-parse", "HEAD"]),
        candidate_sha
    );
    assert!(
        fx.artifact_text("Autopilot changes (round 1)")
            .contains(NOTES)
    );
    let events = fx.ctx.store.events().unwrap();
    let delivered = events
        .iter()
        .find(|e| e.kind == "changes_requested" && e.task.as_deref() == Some(&fx.id))
        .unwrap();
    assert_eq!(delivered.detail["via"], "agent_prompt");
    assert_eq!(delivered.detail["submitted"], true);
    let artifact = task
        .artifacts
        .iter()
        .find(|a| a.title == "Autopilot changes (round 1)")
        .unwrap();
    assert!(
        fx.herdr_log("prompts.log")
            .contains(fx.ctx.store.dir().join(&artifact.path).to_str().unwrap())
    );
    assert_eq!(fx.events("gate_started"), gates);
    assert_eq!(fx.events("review_started"), reviews);
    assert_eq!(
        task.artifacts.len(),
        artifacts + 1,
        "only the change request"
    );
    assert_eq!(fx.events("integration_request"), 0);
}

#[test]
fn changes_notes_take_priority_over_a_new_main_conflict() {
    // Given a changes verdict and a main commit conflicting with the candidate.
    let fx = configured(2);
    fx.verdict(Verdict::Changes, NOTES);
    let review = fx.run().review.unwrap();
    fx.commit_on_main("widget.txt", "conflicting main\n", "Other task lands");

    // When sync sees both the conflict and the recorded defect.
    let task = fx.sync();

    // Then the agent receives the review request, not an integration repair request.
    assert_eq!(task.status, Status::Running);
    assert_eq!(task.rounds, 1);
    assert_eq!(fx.stage(), Some(AutoStage::Changes));
    assert!(
        fx.artifact_text("Autopilot changes (round 1)")
            .contains(NOTES)
    );
    assert_eq!(git(&fx.run().worktree, &["rev-parse", "HEAD"]), review.sha);
    assert_eq!(fx.events("integration_request"), 0);
    assert_eq!(fx.events("gate_started"), 1);
    assert_eq!(fx.events("review_started"), 1);
}

#[test]
fn changes_escalate_at_max_rounds_even_when_main_moves() {
    // Given a recorded changes verdict with the configured round budget exhausted.
    let fx = configured(2);
    fx.ctx
        .store
        .update(&fx.id, |t| {
            t.rounds = 2;
            Ok(())
        })
        .unwrap();
    fx.verdict(Verdict::Changes, NOTES);
    fx.commit_on_main("unrelated.txt", "other task\n", "Other task lands");

    // When sync processes the changes after the base moved.
    let task = fx.sync();

    // Then it escalates without spending a round or restarting verification.
    assert_eq!(task.status, Status::NeedsMe, "{}", fx.reason());
    assert_eq!(task.rounds, 2);
    assert_eq!(fx.stage(), Some(AutoStage::Escalated));
    assert!(fx.reason().contains("all 2 change rounds used"));
    assert_eq!(fx.run().review.unwrap().verdict, Some(Verdict::Changes));
    assert_eq!(fx.events("changes_requested"), 0);
    assert_eq!(fx.events("gate_started"), 1);
    assert_eq!(fx.events("review_started"), 1);
}

#[test]
fn current_changes_prevent_regating_even_from_a_gating_stage() {
    // Given a persisted gating stage with a changes verdict on its current SHA.
    let fx = configured(2);
    fx.verdict(Verdict::Changes, NOTES);
    fx.ctx
        .store
        .update(&fx.id, |t| {
            t.current_run_mut()
                .unwrap()
                .autopilot
                .as_mut()
                .unwrap()
                .stage = AutoStage::Gating;
            Ok(())
        })
        .unwrap();
    fx.commit_on_main("unrelated.txt", "other task\n", "Other task lands");

    // When sync resumes this candidate.
    let task = fx.sync();

    // Then changes win over the persisted stage and stale gate.
    assert_eq!(task.status, Status::Running, "{}", fx.reason());
    assert_eq!(task.rounds, 1);
    assert_eq!(fx.stage(), Some(AutoStage::Changes));
    assert!(
        fx.artifact_text("Autopilot changes (round 1)")
            .contains(NOTES)
    );
    assert_eq!(fx.events("gate_started"), 1);
    assert_eq!(fx.events("review_started"), 1);
}

#[test]
fn changes_on_an_old_sha_do_not_block_a_new_candidate() {
    // Given a changes verdict retained in legacy state after the candidate moved.
    let fx = configured(2);
    fx.verdict(Verdict::Changes, NOTES);
    let old_sha = fx.run().review.unwrap().sha;
    fx.change("widget.txt", "reworked\n");
    corral_git::commit_all(&fx.run().worktree, "Fix the defect").unwrap();

    // When sync observes a different current SHA.
    let task = fx.sync();

    // Then verification runs on that new candidate rather than replaying old notes.
    assert_eq!(task.status, Status::Review, "{}", fx.reason());
    assert_eq!(task.rounds, 0);
    assert_eq!(fx.stage(), Some(AutoStage::Reviewing));
    assert_ne!(fx.run().review.unwrap().sha, old_sha);
    assert_eq!(fx.events("gate_started"), 2);
    assert_eq!(fx.events("review_started"), 2);
    assert_eq!(fx.events("changes_requested"), 0);
}
