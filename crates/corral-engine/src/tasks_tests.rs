use crate::{
    Actor, Blocked, Engine, LaunchSpec, Outcome, Runner, Status, as_refusal,
    events::EventKind,
    tasks::{NewTask, TaskEdit},
    testing::FakeRunner,
    tests::run,
};

const STATUSES: [Status; 8] = [
    Status::Idea,
    Status::Backlog,
    Status::Ready,
    Status::Running,
    Status::Blocked,
    Status::Review,
    Status::Done,
    Status::Dropped,
];

#[test]
fn normal_move_rules_refusal_next_and_forced_any_to_any() {
    let root = crate::tests::tempdir();
    let engine = Engine::init(root.path(), None).unwrap();
    let mut task = engine
        .new_task(NewTask {
            title: "move".to_owned(),
            ..Default::default()
        })
        .unwrap();
    for from in STATUSES {
        for to in STATUSES {
            task.status = from;
            task.blocked = (from == Status::Blocked).then(|| Blocked {
                reason: "question".to_owned(),
                detail: None,
                next: vec![super::next_reply("T-001")],
            });
            engine.store.update_task(&task).unwrap();
            let planning =
                |status| matches!(status, Status::Idea | Status::Backlog | Status::Ready);
            let allowed = (planning(from) && planning(to))
                || (matches!(from, Status::Done | Status::Dropped) && to == Status::Backlog);
            let result = engine.move_task(&task.id, to, false);
            assert_eq!(result.is_ok(), allowed, "{from:?} -> {to:?}");
            if let Err(error) = result {
                assert_eq!(
                    as_refusal(&error).unwrap().next,
                    vec![crate::Next::force(crate::Op::Move {
                        id: task.id.clone(),
                        status: to
                    })]
                );
                assert_eq!(engine.store.require_task(&task.id).unwrap().status, from);
            }
            // Restore the source so every forced pair is independently exercised.
            engine.store.update_task(&task).unwrap();
            let forced = engine.move_task(&task.id, to, true).unwrap();
            assert_eq!(forced.status, to);
            assert_eq!(forced.blocked.is_some(), to == Status::Blocked);
            let events = engine.store.list_events(&task.id).unwrap();
            let event = events.last().unwrap();
            assert_eq!(event.kind, EventKind::Forced);
            assert_eq!(
                event.detail["skipped"].as_array().unwrap().is_empty(),
                allowed
            );
        }
    }
}

#[test]
fn terminal_moves_stop_live_runs_preserve_checkout_and_owner_actions_reset_round() {
    let root = crate::tests::tempdir();
    Engine::init(root.path(), None).unwrap();
    let fake = FakeRunner::new();
    let mut engine = Engine::open_with_runner(root.path(), Box::new(fake.clone())).unwrap();
    let mut task = engine
        .new_task(NewTask {
            title: "stop me".to_owned(),
            ..Default::default()
        })
        .unwrap();
    let checkout = root.path().join("checkout");
    std::fs::create_dir(&checkout).unwrap();
    let spec = LaunchSpec {
        label: task.title.clone(),
        cwd: checkout.clone(),
        command: "true".to_owned(),
        env: Vec::new(),
        dir: engine.state_dir.join("runs").join(&task.id).join("R1"),
        tokens: Vec::new(),
    };
    let session = fake.start(&spec).unwrap();
    let mut run = run(&task.id, "R1");
    run.session = Some(session.clone());
    run.worktree = checkout.clone();
    engine.store.insert_run(&run).unwrap();
    engine.move_task(&task.id, Status::Done, true).unwrap();
    assert_eq!(fake.stops(), [session]);
    let stopped = engine.store.get_run(&task.id, "R1").unwrap().unwrap();
    assert!(stopped.ended.is_some());
    assert_eq!(stopped.outcome, Some(Outcome::Stopped));
    assert_eq!(stopped.branch, run.branch);
    assert!(checkout.is_dir());
    task = engine.reopen(&task.id).unwrap();
    assert_eq!(task.status, Status::Backlog);
    task.round = 2;
    engine.store.update_task(&task).unwrap();
    let edited = engine
        .edit_task(
            &task.id,
            TaskEdit {
                title: Some("renamed".to_owned()),
                project: Some(None),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(edited.round, 0);
    assert_eq!(edited.title, "renamed");
    assert!(edited.project.is_none());
    let comment = engine.comment(&task.id, "owner note").unwrap();
    assert!(comment.body.ends_with("owner note"));
    assert_eq!(engine.drop_task(&task.id).unwrap().status, Status::Dropped);
    assert_eq!(engine.reopen(&task.id).unwrap().status, Status::Backlog);
    engine.actor = Actor::Auto;
    assert!(
        as_refusal(
            &engine
                .move_task(&task.id, Status::Review, true)
                .unwrap_err()
        )
        .is_some()
    );
}
