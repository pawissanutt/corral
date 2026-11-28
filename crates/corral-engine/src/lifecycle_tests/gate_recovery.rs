use super::*;

#[test]
fn gate_spawn_failure_blocks_and_a_repaired_gate_resumes_review() {
    let mut cx = Fixture::new(&[]);
    cx.engine.pipeline = PipelineMode::Off;
    let id = cx.backlog("run a gate that cannot spawn");
    cx.dispatch(&id);
    cx.done(&id);
    cx.project().gate = vec!["invalid\0argument".into()];
    let op = Op::Gate { id: id.clone() };
    let failure = cx.engine.apply(op.clone(), false).unwrap_err();
    assert!(format!("{failure:#}").contains("run gate line"));
    let blocked = cx.task(&id).blocked.unwrap();
    assert_eq!(blocked.reason, "gate failed");
    assert!(blocked.detail.unwrap().contains("run gate line"));
    assert_eq!(blocked.next, vec![crate::Next::run(op.clone())]);
    assert_eq!(
        cx.events(&id, EventKind::Error)[0].detail["operation"],
        "gate"
    );
    cx.project().gate = vec!["true".into()];
    assert_eq!(cx.apply(op, false).task.unwrap().status, Status::Review);
}

#[test]
fn repaired_gate_during_rework_requires_resubmission() {
    let mut cx = Fixture::new(&[]);
    cx.engine.pipeline = PipelineMode::Off;
    let id = cx.backlog("rework a reviewed submission");
    let run = cx.dispatch(&id);
    cx.done(&id);
    cx.engine.changes(&id, "revise").unwrap();
    git(
        &run.worktree,
        &["commit", "--allow-empty", "-m", "candidate B"],
    );
    cx.project().gate = vec!["invalid\0argument".into()];
    assert!(cx.engine.gate(&id).is_err());
    cx.project().gate = vec!["true".into()];
    let check = cx.engine.gate(&id).unwrap();
    assert_eq!(cx.task(&id).status, Status::Running);
    cx.done(&id);
    assert_eq!(cx.task(&id).status, Status::Review);
    assert_eq!(
        cx.events(&id, EventKind::Done).last().unwrap().detail["sha"],
        check.key
    );
}

#[test]
fn gate_retry_after_lost_pipeline_requires_resubmission() {
    let mut cx = Fixture::new(&[]);
    cx.project().reviewer = Some("generic".into());
    cx.project().auto_land = Some(false);
    cx.engine.pipeline = PipelineMode::Off;
    let id = cx.backlog("review only the gated submission");
    let worker = cx.dispatch(&id);
    fs::write(worker.worktree.join("file.txt"), "candidate A\n").unwrap();
    let submitted = cx.done(&id).run.unwrap();
    let a = submitted.head_sha.as_deref().unwrap();
    fs::write(worker.worktree.join("file.txt"), "candidate B\n").unwrap();
    git(&worker.worktree, &["commit", "-am", "candidate B"]);
    let b = git(&worker.worktree, &["rev-parse", "HEAD"]);
    git(&worker.worktree, &["reset", "--hard", a]);

    // Execute the real gate but omit the caller's error-to-block transition,
    // as when a detached pipeline is lost after settling the failed attempt.
    cx.project().gate = vec![format!("git reset --hard {b}")];
    let project = cx.engine.project_for(&cx.task(&id)).unwrap().unwrap();
    let error = cx
        .engine
        .run_gate(&submitted, 0, Some(&project), Actor::Auto, &mut false)
        .unwrap_err();
    assert!(format!("{error:#}").contains("HEAD changed"));
    let attempt = cx.engine.latest_gate_attempt(&id).unwrap().unwrap();
    assert_eq!(attempt.sha, a);
    assert!(attempt.check.is_none());
    assert_eq!(cx.task(&id).status, Status::Review);

    cx.project().gate = vec!["true".into()];
    cx.engine.pipeline = PipelineMode::Inline;
    let retried = cx.engine.gate(&id).unwrap();
    assert_eq!((retried.result, &retried.key), (CheckResult::Pass, &b));
    assert!(cx.events(&id, EventKind::ReviewStarted).is_empty());
    assert_eq!(
        cx.runner.starts().len(),
        1,
        "only the worker should have started"
    );
    let blocked = cx.task(&id).blocked.unwrap();
    assert_eq!(blocked.next, vec![crate::review::next_done(&id)]);
    assert_eq!(cx.events(&id, EventKind::Done).len(), 1);

    let resubmitted = cx.done(&id).run.unwrap();
    assert_eq!(resubmitted.head_sha.as_deref(), Some(b.as_str()));
    cx.engine.advance(&id).unwrap();
    assert_eq!(cx.events(&id, EventKind::ReviewStarted).len(), 1);
    assert_eq!(
        cx.runner.starts().len(),
        2,
        "resubmission starts one reviewer"
    );
}
