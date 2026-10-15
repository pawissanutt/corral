use corral_core::{
    DecisionRequest, ObligationSource, ProfileChoice, Status, WaitHandle, WaitRequest,
};
use corral_ops::{done, register_decision, register_wait, retry_run, save_checkpoint};
use serde_json::Value;

use super::delivery::worker;
use super::fixture::Fixture;

fn decision(f: &Fixture) {
    register_decision(
        &f.ctx,
        &f.id,
        DecisionRequest {
            version: 1,
            id: "release".into(),
            run: "R1".into(),
            question: "Which release branch?".into(),
            requested_action: "Choose a branch".into(),
            owner: "owner".into(),
            source: ObligationSource::Worker,
            deadline: None,
        },
        "2026-10-15T12:00:00Z".parse().unwrap(),
    )
    .unwrap();
}

#[test]
fn native_obligations_survive_checkpoint_and_fresh_or_stale_resume() {
    let f = worker();
    register_wait(
        &f.ctx,
        &f.id,
        WaitRequest {
            version: 1,
            id: "native-build".into(),
            run: "R1".into(),
            condition: "The external build completes".into(),
            owner: "fixture-a".into(),
            source: ObligationSource::Worker,
            deadline: None,
            handle: WaitHandle::Channel {
                channel_id: "build-42".into(),
                capability: None,
            },
        },
        "2026-10-15T12:00:00Z".parse().unwrap(),
    )
    .unwrap();
    decision(&f);
    f.seed_checkpoint();
    let mut task = f.task();
    let source = task.current_run().unwrap();
    let native = source.obligations.clone();
    assert_eq!(native.waits.len(), 1);
    assert!(native.pending_decision().is_some());
    let record = source.checkpoints.last().unwrap().clone();
    assert_eq!(record.checkpoint.run_obligations, native);
    let bytes = std::fs::read(f.ctx.store.dir().join(&record.artifact)).unwrap();
    let frozen: corral_core::Checkpoint = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(frozen.run_obligations, native);

    let mut target = source.clone();
    target.id = "R2".into();
    target.worktree = f.tmp.path().join("fresh-worker");
    target.obligations = Default::default();
    corral_git::worktree_add(&f.repo, &target.worktree, "fresh-worker", "main").unwrap();
    for stale in [false, true] {
        if stale {
            task.body.push_str("\nA revised contract.\n");
        }
        let prompt = corral_ops::task_prompt(&f.ctx, &task, &target).unwrap();
        let json = prompt
            .split_once("<corral-resume>\n")
            .unwrap()
            .1
            .split_once("\n</corral-resume>")
            .unwrap()
            .0;
        let packet: Value = serde_json::from_str(json).unwrap();
        assert_eq!(packet["status"], if stale { "stale" } else { "compatible" });
        assert_eq!(
            packet["prior_run_obligations"],
            serde_json::to_value(&native).unwrap()
        );
        assert!(target.obligations.is_empty());
        assert_eq!(
            std::fs::read(f.ctx.store.dir().join(&record.artifact)).unwrap(),
            bytes
        );
    }
}

#[test]
fn a_new_native_decision_requires_a_fresh_handoff_without_rewriting_history() {
    let f = worker();
    f.seed_checkpoint();
    decision(&f);
    let before = f.snapshot();
    let error = done(&f.ctx, &f.id, "Implemented").unwrap_err();
    assert!(error.to_string().contains("obligations_changed"));
    assert_eq!(f.snapshot(), before);
    let old = f.task().current_run().unwrap().checkpoints[0].clone();
    save_checkpoint(&f.ctx, &f.id, &f.checkpoint()).unwrap();
    let task = f.task();
    let run = task.current_run().unwrap();
    assert_eq!(run.checkpoints[0], old);
    assert!(
        run.checkpoints[1]
            .checkpoint
            .run_obligations
            .pending_decision()
            .is_some()
    );
}

#[test]
fn resending_a_checkpoint_request_does_not_answer_an_owner_decision() {
    let f = worker();
    decision(&f);
    std::fs::write(f.tmp.path().join("reject"), "").unwrap();
    assert!(retry_run(&f.ctx, &f.id, "R1", &ProfileChoice::default()).is_err());
    assert!(corral_ops::resend_reply(&f.ctx, &f.id).is_err());
    assert!(
        f.task()
            .current_run()
            .unwrap()
            .obligations
            .pending_decision()
            .is_some()
    );
    std::fs::remove_file(f.tmp.path().join("reject")).unwrap();
    let task = corral_ops::resend_reply(&f.ctx, &f.id).unwrap();
    let run = task.current_run().unwrap();
    assert!(run.pending_reply.is_none());
    assert!(run.obligations.pending_decision().is_some());
    assert_eq!(task.status, Status::NeedsMe);
    assert_eq!(task.runs.len(), 1);
    assert!(!f.tmp.path().join("closed").exists());
}
