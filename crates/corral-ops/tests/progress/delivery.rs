use corral_core::{AgentCmd, HerdrRef, ProfileChoice, Status, Store};
use corral_ops::retry_run;

use super::fixture::Fixture;

pub(super) fn worker() -> Fixture {
    let mut f = Fixture::new();
    let bin = f.tmp.path().join("herdr");
    std::os::unix::fs::symlink(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/progress/herdr.sh"),
        &bin,
    )
    .unwrap();
    f.ctx.herdr = corral_herdr::Herdr::with_bin(bin, Some("corral-test".into()));
    f.ctx.store.config.agents.insert(
        "fake".into(),
        AgentCmd {
            command: "true".into(),
        },
    );
    f.ctx.store.save_config().unwrap();
    f.ctx
        .store
        .update(&f.id, |task| {
            task.current_run_mut().unwrap().herdr = Some(HerdrRef {
                workspace_id: "w1".into(),
                pane_id: "w1:p1".into(),
            });
            Ok(())
        })
        .unwrap();
    f
}

#[test]
fn failed_checkpoint_request_is_durable_without_superseding_worker() {
    // Given a reachable worker with no checkpoint and a failed native submission.
    let f = worker();
    std::fs::write(f.tmp.path().join("reject"), "").unwrap();
    // When retry asks that worker for its mandatory handoff.
    let error = retry_run(&f.ctx, &f.id, "R1", &ProfileChoice::default()).unwrap_err();
    // Then exact input is retained and a typed failed receipt explains the blocked retry.
    assert_eq!(
        error.to_string().split(':').next().unwrap(),
        "checkpoint_request_not_delivered"
    );
    let store = Store::open(f.tmp.path()).unwrap();
    let task = store.get(&f.id).unwrap();
    let run = task.current_run().unwrap();
    let sent = std::fs::read_to_string(f.tmp.path().join("request.txt")).unwrap();
    assert_eq!(run.pending_reply.as_deref(), Some(sent.as_str()));
    assert!(run.outcome.is_none());
    assert_eq!(task.status, Status::Running);
    assert_eq!(task.runs.len(), 1);
    assert!(!f.tmp.path().join("closed").exists());
    let receipt = store
        .events()
        .unwrap()
        .into_iter()
        .find(|e| e.kind == "checkpoint_requested")
        .unwrap();
    assert_eq!(receipt.detail["submitted"], false);
    assert_eq!(receipt.detail["attempts"], 1);
}

#[test]
fn successful_checkpoint_request_resend_clears_pending_but_does_not_retry() {
    // Given a durable pending request after a failed submission.
    let f = worker();
    std::fs::write(f.tmp.path().join("reject"), "").unwrap();
    assert!(retry_run(&f.ctx, &f.id, "R1", &ProfileChoice::default()).is_err());
    std::fs::remove_file(f.tmp.path().join("reject")).unwrap();
    // When delivery succeeds but the worker has not yet published a checkpoint.
    let error = retry_run(&f.ctx, &f.id, "R1", &ProfileChoice::default()).unwrap_err();
    // Then submission is recorded and the existing run still owns the work.
    assert_eq!(
        error.to_string().split(':').next().unwrap(),
        "checkpoint_required"
    );
    let task = f.task();
    assert!(task.current_run().unwrap().pending_reply.is_none());
    assert!(task.current_run().unwrap().outcome.is_none());
    assert_eq!(task.runs.len(), 1);
    assert!(!f.tmp.path().join("closed").exists());
    let receipts: Vec<_> = f
        .ctx
        .store
        .events()
        .unwrap()
        .into_iter()
        .filter(|e| e.kind == "checkpoint_requested")
        .collect();
    assert_eq!(receipts.len(), 2);
    assert_eq!(receipts[1].detail["submitted"], true);
}

#[test]
fn checkpoint_request_cannot_replace_an_undelivered_owner_reply() {
    // Given a different owner reply already pending in the current run.
    let f = worker();
    f.ctx
        .store
        .update(&f.id, |task| {
            task.current_run_mut().unwrap().pending_reply = Some("owner-input".into());
            Ok(())
        })
        .unwrap();
    let before = f.snapshot();
    // When retry would need to ask for a checkpoint.
    let error = retry_run(&f.ctx, &f.id, "R1", &ProfileChoice::default()).unwrap_err();
    // Then the existing input is neither overwritten nor submitted as another request.
    assert_eq!(
        error.to_string().split(':').next().unwrap(),
        "pending_reply_before_checkpoint_request"
    );
    assert_eq!(f.snapshot(), before);
    assert!(!f.tmp.path().join("request.txt").exists());
}
