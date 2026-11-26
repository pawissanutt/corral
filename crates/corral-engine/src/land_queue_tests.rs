use super::*;

#[test]
fn legacy_repair_survives_gate_error_and_resumes_even_over_capacity() {
    let mut cx = Fixture::new(&[]);
    cx.engine.pipeline = PipelineMode::Off;
    let first = cx.backlog("recover review feedback");
    cx.dispatch(&first);
    cx.done(&first);
    cx.dispatch(&cx.backlog("started when Review held no slot"));
    cx.engine.config.limits.running = 1;
    cx.engine
        .store
        .connection
        .execute(
            "INSERT INTO pending_repairs(task,notes) VALUES(?1,'restore these notes')",
            [&first],
        )
        .unwrap();

    cx.project().gate = vec!["invalid\0argument".into()];
    assert!(cx.engine.gate(&first).is_err());
    assert_eq!(cx.task(&first).status, Status::Review);
    cx.apply(Op::Sync, false);
    assert_eq!(cx.runner.sends()[0].text, "restore these notes");
    assert_eq!(cx.task(&first).status, Status::Running);
    assert!(!cx.engine.store.has_queued_repair(&first).unwrap());
    assert_eq!(cx.engine.store.occupied_slots(None).unwrap(), 2);
}

#[test]
fn failed_queued_repair_offers_retry_with_original_notes() {
    let mut cx = Fixture::new(&[]);
    cx.engine.config.limits.running = 1;
    let first = cx.backlog("needs revision");
    let worker = cx.dispatch(&first);
    cx.done(&first);
    // Simulate a repair persisted by the previous engine before upgrade.
    cx.engine
        .store
        .connection
        .execute(
            "INSERT INTO pending_repairs(task,notes) VALUES(?1,'fix the draft')",
            [&first],
        )
        .unwrap();
    cx.runner
        .set_status(worker.session.as_ref().unwrap(), SessionStatus::Gone);
    git(
        &cx.repo,
        &["worktree", "remove", worker.worktree.to_str().unwrap()],
    );
    cx.apply(Op::Sync, false);
    let blocked = cx.task(&first).blocked.expect("actionable repair failure");
    assert!(blocked.detail.unwrap().contains("no worktree"));
    let retry = blocked.next.first().unwrap();
    assert!(
        matches!(&retry.op, Op::Retry { id, note: Some(note), fresh: false, .. } if id == &first && note == "fix the draft")
    );
    cx.apply(retry.op.clone(), false);
    assert_eq!(cx.task(&first).status, Status::Running);
    assert!(worker.worktree.is_dir());
}

#[test]
fn exhausted_review_can_resume_its_slot_at_capacity() {
    let mut cx = Fixture::new(&[]);
    cx.engine.config.limits.running = 1;
    let first = cx.backlog("needs owner decision");
    cx.dispatch(&first);
    cx.done(&first);
    cx.project().max_rounds = Some(0);
    cx.engine
        .verdict(&first, false, Some("fix the draft"))
        .unwrap();
    let blocked = cx
        .task(&first)
        .blocked
        .expect("exhaustion requires an owner decision");
    assert_eq!(blocked.reason, "automatic change rounds exhausted");
    assert!(!cx.engine.store.has_queued_repair(&first).unwrap());
    cx.engine.reply(&first, "please repair").unwrap();
    assert_eq!(cx.task(&first).status, Status::Running);
    cx.done(&first);
    cx.engine.changes(&first, "owner repair").unwrap();
    assert_eq!(cx.task(&first).status, Status::Running);
    cx.done(&first);
    cx.engine.retry(&first, None, false, None).unwrap();
    assert_eq!(cx.task(&first).status, Status::Running);
    assert_eq!(cx.engine.store.occupied_slots(None).unwrap(), 1);
}

#[test]
fn approval_preserves_repair_queued_during_gate() {
    let mut cx = Fixture::new(&["true"]);
    cx.engine.pipeline = PipelineMode::Off;
    let base = git(&cx.repo, &["rev-parse", "main"]);
    let id = cx.backlog("land after review");
    let run = cx.dispatch(&id);
    fs::write(run.worktree.join("change.txt"), "worker change\n").unwrap();
    cx.done(&id);
    // The initial admission check has passed, but the final transaction hasn't begun.
    cx.engine
        .store
        .connection
        .execute_batch(
            "CREATE TEMP TRIGGER queue_late_repair AFTER INSERT ON checks
         BEGIN INSERT INTO pending_repairs(task, notes) VALUES(NEW.task, 'owner feedback'); END;",
        )
        .unwrap();
    let refusal = cx.engine.approve(&id, None, false).unwrap_err();
    assert!(as_refusal(&refusal).is_some());
    assert_eq!(
        cx.engine.store.require_task(&id).unwrap().status,
        Status::Review
    );
    assert!(cx.engine.store.has_queued_repair(&id).unwrap());
    assert_eq!(git(&cx.repo, &["rev-parse", "main"]), base);
    assert!(run.worktree.is_dir());

    cx.engine.approve(&id, None, true).unwrap();
    let events = cx.engine.store.list_events(&id).unwrap();
    let audit = events
        .iter()
        .find(|event| event.kind == EventKind::Forced)
        .unwrap();
    assert_eq!(
        audit.detail["skipped"],
        serde_json::json!(["queued repair"])
    );
}
