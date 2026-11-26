use super::*;

#[test]
fn review_keeps_its_slot_through_sync_and_send_back() {
    let mut cx = Fixture::new(&[]);
    cx.engine.config.limits.running = 1;
    cx.project().running = Some(1);
    let first = cx.backlog("enter review");
    let run = cx.dispatch(&first);
    assert_eq!(cx.done(&first).task.unwrap().status, Status::Review);
    let second = cx.new_task("next ready task", Status::Ready, Priority::P2, vec![]);
    cx.project().auto_dispatch = Some(true);
    cx.apply(Op::Sync, false);
    assert_eq!(cx.task(&second).status, Status::Ready);
    assert_eq!(cx.runner.starts().len(), 1);
    assert_eq!(cx.engine.store.occupied_slots(None).unwrap(), 1);
    assert_eq!(cx.engine.store.occupied_slots(Some("project")).unwrap(), 1);
    assert!(cx.engine.dispatch(&second, None, None, true).is_err());
    cx.engine.config.limits.running = 2;
    let refusal = cx.engine.dispatch(&second, None, None, true).unwrap_err();
    assert!(
        as_refusal(&refusal)
            .unwrap()
            .reason
            .contains("project.running")
    );
    cx.engine.config.limits.running = 1;

    cx.engine
        .verdict(&first, false, Some("fix the draft"))
        .unwrap();
    assert_eq!(cx.task(&first).status, Status::Running);
    assert_eq!(cx.task(&first).round, 1);
    assert_eq!(cx.events(&first, EventKind::SentBack).len(), 1);
    assert_eq!(cx.runner.sends()[0].session, run.session.unwrap());
    assert_eq!(cx.engine.store.occupied_slots(None).unwrap(), 1);

    cx.apply(
        Op::Move {
            id: first,
            status: Status::Backlog,
        },
        true,
    );
    assert_eq!(cx.runner.stops().len(), 1);
    cx.apply(Op::Sync, false);
    assert_eq!(cx.task(&second).status, Status::Running);
}

#[test]
fn manual_dispatch_and_sync_compete_for_one_slot() {
    let mut cx = Fixture::new(&[]);
    cx.engine.config.limits.running = 1;
    cx.project().auto_dispatch = Some(true);
    let ready = cx.new_task(
        "eligible for auto-dispatch",
        Status::Ready,
        Priority::P2,
        vec![],
    );
    let manual = cx.backlog("eligible for manual dispatch");
    let mut other = Engine::open(cx._root.path()).unwrap();
    other.config = cx.engine.config.clone();
    other.runner = Box::new(cx.runner.clone());

    let start = std::sync::Barrier::new(2);
    std::thread::scope(|scope| {
        let sync_start = &start;
        let sync = scope.spawn(move || {
            sync_start.wait();
            other.sync().unwrap();
        });
        start.wait();
        let dispatched = cx.engine.dispatch(&manual, None, None, false);
        sync.join().unwrap();
        match dispatched {
            Ok(_) => assert_eq!(cx.task(&ready).status, Status::Ready),
            Err(error) => {
                assert!(
                    as_refusal(&error)
                        .unwrap()
                        .reason
                        .contains("limits.running")
                );
                assert_eq!(cx.task(&ready).status, Status::Running);
            }
        }
    });
    assert_eq!(cx.runner.starts().len(), 1);
    assert_eq!(cx.engine.store.occupied_slots(None).unwrap(), 1);
}

#[test]
fn force_records_only_the_skipped_checks_and_stops_live_sessions() {
    let mut cx = Fixture::new(&["false"]);
    cx.engine.pipeline = PipelineMode::Off;
    let running = cx.backlog("occupies slot");
    let run = cx.dispatch(&running);
    let dependency = cx.backlog("not done");
    let id = cx.new_task(
        "forced work",
        Status::Backlog,
        Priority::P2,
        vec![dependency],
    );
    let refused = cx.engine.apply(dispatch(&id), false).unwrap_err();
    assert!(
        as_refusal(&refused)
            .unwrap()
            .next
            .iter()
            .any(|n| n.force && n.op == dispatch(&id))
    );
    let forced = cx.apply(dispatch(&id), true).run.unwrap();
    assert_eq!(
        cx.events(&id, EventKind::Forced)[0].detail["skipped"],
        serde_json::json!(["dependencies"])
    );
    fs::write(forced.worktree.join("file.txt"), "forced change\n").unwrap();
    let submitted = cx.done(&id);
    assert_eq!(submitted.task.unwrap().status, Status::Review);
    let head = submitted.run.unwrap().head_sha.unwrap();
    cx.runner
        .set_status(forced.session.as_ref().unwrap(), SessionStatus::Working);
    assert_eq!(cx.engine.gate(&id).unwrap().result, CheckResult::Fail);
    cx.apply(approve(&id), true);
    cx.assert_landed(&forced, &head);
    assert_eq!(cx.events(&id, EventKind::Gate).len(), 1);
    assert_eq!(
        cx.events(&id, EventKind::Forced)[1].detail["skipped"],
        serde_json::json!(["gate"])
    );
    cx.apply(
        Op::Move {
            id: running.clone(),
            status: Status::Done,
        },
        true,
    );
    assert_eq!(
        cx.runner.stops(),
        [forced.session.unwrap(), run.session.unwrap()]
    );
    assert!(run.worktree.exists());
    assert!(cx.branch_exists(&run.branch));
    assert_eq!(
        cx.events(&running, EventKind::Forced)[0].detail["skipped"],
        serde_json::json!(["status rule"])
    );
}
