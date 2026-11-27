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
