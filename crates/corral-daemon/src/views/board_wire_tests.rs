use std::{collections::BTreeMap, fs, sync::Arc};

use anyhow::Result;
use corral_engine::{NewTask, Op, Priority, Status};
use corral_protocol::{
    HEALTH_GET, Health, SUB_DELTA,
    detail::{DetailBody, DetailGet, DetailRef},
    frame::Frame,
    health::{SlotCount, Slots},
    rows::{DependencyStatus, EffectiveProjectSettings, StatusCount},
    rpc::{self, ResponseOutcome},
    subscription::{SubDelta, SubOpen, Topic},
};
use serde_json::{Value, json};

use super::{
    detail,
    tests::{Fixture, git},
};
use crate::{app::dispatch, conn::Conn, state::Shared, topics::Subscriptions};

#[derive(Default)]
struct Receiver(Vec<Frame>);

impl Conn for Receiver {
    fn send_frame(&mut self, frame: &Frame) -> Result<()> {
        self.0.push(frame.clone());
        Ok(())
    }
}

impl Receiver {
    fn only_frame(self) -> Frame {
        let [frame]: [Frame; 1] = self.0.try_into().expect("expected exactly one frame");
        frame
    }
}

fn subscribe(shared: &Shared, topic: Topic) -> SubDelta {
    let mut topics = Subscriptions::new(shared);
    topics
        .open(SubOpen {
            topic,
            scope: None,
            since: None,
        })
        .unwrap();
    let mut receiver = Receiver::default();
    topics.flush(&mut receiver).unwrap();
    let message: rpc::Notification<SubDelta> =
        serde_json::from_slice(&receiver.only_frame().payload).unwrap();
    assert_eq!(message.method, SUB_DELTA);
    message.params
}

fn health_get(shared: &Arc<Shared>) -> Health {
    let mut receiver = Receiver::default();
    dispatch::serve(
        &mut receiver,
        shared,
        rpc::Request {
            jsonrpc: rpc::JSONRPC.into(),
            method: HEALTH_GET.into(),
            id: json!(1),
            params: Value::Null,
        },
        None,
        &mut Subscriptions::new(shared),
    )
    .unwrap();
    let response: rpc::Response<Health> =
        serde_json::from_slice(&receiver.only_frame().payload).unwrap();
    let ResponseOutcome::Result { result } = response.outcome else {
        panic!("expected health response: {response:?}");
    };
    result
}

#[test]
fn served_task_and_plan_rows_keep_both_sessions_and_dependency_statuses() {
    let fixture = Fixture::new();
    fixture.project();
    fs::write(
        fixture.shared.root().join(".corral-next/corral.toml"),
        r#"
[defaults]
max_rounds = 4
[projects.demo]
path = "repo"
max_rounds = 0
reviewer = "generic"
"#,
    )
    .unwrap();
    let parent = fixture.named_task("Plan", None);
    let done = fixture.named_task("Landed prerequisite", None);
    let dropped = fixture.named_task("Abandoned prerequisite", None);
    fixture
        .shared
        .engine()
        .unwrap()
        .apply(
            Op::Move {
                id: done.clone(),
                status: Status::Done,
            },
            true,
        )
        .unwrap();
    fixture.apply(Op::Drop {
        id: dropped.clone(),
    });
    let id = fixture.task(NewTask {
        title: "Member".into(),
        parent: Some(parent.clone()),
        project: Some("demo".into()),
        depends_on: vec![dropped.clone(), done.clone()],
        ..Default::default()
    });
    let engine = fixture.shared.engine().unwrap();
    let worker = engine
        .apply(
            Op::Dispatch {
                id: id.clone(),
                profile: None,
                base: None,
            },
            true,
        )
        .unwrap()
        .run
        .unwrap();
    fixture.apply(Op::Done {
        id: id.clone(),
        summary: "draft".into(),
    });
    let reviewer = fixture
        .apply(Op::Review {
            id: id.clone(),
            profile: None,
        })
        .run
        .unwrap();
    // An unavailable dependency still contributes an edge but has no status.
    let missing = "T-999";
    let mut stored = engine.store.require_task(&id).unwrap();
    stored.depends_on.push(missing.into());
    // Two children fit the Tasks topic but need separate pages when sent with legacy rows.
    stored.body = "a".repeat(16 * 1024);
    engine.store.update_task(&stored).unwrap();
    let sibling = fixture.task(NewTask {
        title: "Sibling".into(),
        body: "b".repeat(16 * 1024),
        parent: Some(parent.clone()),
        priority: Priority::P3,
        ..Default::default()
    });

    let get = |page| {
        detail(
            &fixture.shared,
            DetailGet {
                target: DetailRef::Plan { id: parent.clone() },
                page,
                etag: None,
            },
        )
        .unwrap()
    };
    let reply = get(None);
    let next = reply.next.expect("expected a second page");
    let Some(DetailBody::Plan(plan)) = reply.body else {
        panic!("expected plan detail")
    };
    let children = plan.child_rows.unwrap();
    let [member] = children.as_slice() else {
        panic!("expected just the plan member")
    };
    assert_eq!(member.task.id, id);
    assert_eq!(member.task.status, Status::Review);
    assert_eq!(member.run.as_ref().unwrap().id, worker.id);
    assert_eq!(member.max_rounds, Some(0));
    assert_eq!(member.reviewer.as_ref().unwrap().id, reviewer.id);
    assert_eq!(
        member.dependencies.as_deref().unwrap(),
        [
            DependencyStatus {
                id: dropped,
                status: Some(Status::Dropped),
            },
            DependencyStatus {
                id: done,
                status: Some(Status::Done),
            },
            DependencyStatus {
                id: missing.into(),
                status: None,
            },
        ]
    );
    assert_eq!(plan.children.as_slice(), std::slice::from_ref(&member.task));

    let second = get(Some(next));
    assert_eq!(second.etag, reply.etag);
    assert!(second.next.is_none());
    let Some(DetailBody::Plan(plan)) = second.body else {
        panic!("expected plan detail")
    };
    let children = plan.child_rows.unwrap();
    let [row] = children.as_slice() else {
        panic!("expected just the sibling")
    };
    assert_eq!(row.task.id, sibling);
    assert_eq!(plan.children.as_slice(), std::slice::from_ref(&row.task));

    let SubDelta::Tasks { delta } = subscribe(&fixture.shared, Topic::Tasks) else {
        panic!("expected tasks")
    };
    assert_eq!(delta.rows.len(), 3);
    assert_eq!(
        delta.rows.iter().find(|row| row.task.id == id),
        Some(member)
    );
    let parent = delta.rows.iter().find(|row| row.task.id == parent).unwrap();
    assert_eq!(parent.max_rounds, Some(4));
    assert!(parent.dependencies.as_ref().unwrap().is_empty());
    assert!(parent.reviewer.is_none());
}

#[test]
fn served_projects_and_health_show_open_tasks_and_dispatch_slot_budget() {
    let fixture = Fixture::new();
    fixture.project();
    git(
        &fixture.shared.root().join("repo"),
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/trunk",
        ],
    );
    fs::write(
        fixture.shared.root().join(".corral-next/corral.toml"),
        r#"
[limits]
running = 7
[defaults]
profile = "generic"
auto_dispatch = true
auto_land = true
branch_prefix = "work/"
max_rounds = 5
[projects.demo]
path = "repo"
auto_dispatch = false
running = 2
reviewer = "generic"
gate = ["cargo", "check"]
env = { CHECK_MODE = "fast" }
[projects.unlimited]
path = "repo"
base = "explicit"
[projects.offline]
path = "missing-repo"
"#,
    )
    .unwrap();
    let create = |title, project, status| {
        let id = fixture.named_task(title, project);
        fixture
            .shared
            .engine()
            .unwrap()
            .apply(Op::Move { id, status }, true)
            .unwrap();
    };
    for status in [
        Status::Running,
        Status::Blocked,
        Status::Review,
        Status::Ready,
        Status::Done,
        Status::Dropped,
    ] {
        create(status.as_str(), Some("demo"), status);
    }
    create("unassigned worker", None, Status::Running);
    let SubDelta::Projects { delta } = subscribe(&fixture.shared, Topic::Projects) else {
        panic!("expected projects even with an unavailable repository")
    };
    assert_eq!(delta.rows.len(), 3);
    let project = |id| delta.rows.iter().find(|row| row.id == id).unwrap();
    let demo = project("demo");
    assert_eq!(demo.open, 4);
    let by_status = demo.open_by_status.as_deref().unwrap();
    assert_eq!(by_status.len(), 4);
    for status in [
        Status::Running,
        Status::Blocked,
        Status::Review,
        Status::Ready,
    ] {
        assert!(by_status.contains(&StatusCount { status, count: 1 }));
    }
    assert_eq!(
        demo.effective.as_ref().unwrap(),
        &EffectiveProjectSettings {
            path: "repo".into(),
            base: Some("trunk".into()),
            reviewer: Some("generic".into()),
            profile: Some("generic".into()),
            gate: vec!["cargo".into(), "check".into()],
            running: Some(2),
            env: BTreeMap::from([("CHECK_MODE".into(), "fast".into())]),
            auto_dispatch: false,
            auto_land: true,
            max_rounds: 5,
            branch_prefix: "work/".into(),
        }
    );
    let unlimited = project("unlimited");
    assert_eq!(unlimited.open, 0);
    assert!(unlimited.open_by_status.as_ref().unwrap().is_empty());
    let unlimited = unlimited.effective.as_ref().unwrap();
    assert_eq!(unlimited.base.as_deref(), Some("explicit"));
    assert!(unlimited.auto_dispatch);
    let offline = project("offline").effective.as_ref().unwrap();
    assert_eq!(offline.base, None);
    assert_eq!(offline.profile.as_deref(), Some("generic"));

    let shared = Arc::new(fixture.shared);
    let health = health_get(&shared);
    assert_eq!(
        health.slots.as_ref().unwrap(),
        &Slots {
            global: SlotCount { used: 3, limit: 7 },
            projects: BTreeMap::from([("demo".into(), SlotCount { used: 2, limit: 2 })]),
        }
    );
    let SubDelta::Health { rows, .. } = subscribe(&shared, Topic::Health) else {
        panic!("expected health topic")
    };
    assert_eq!(rows, [health]);

    fs::write(shared.root().join(".corral-next/corral.toml"), "invalid").unwrap();
    shared.publish();
    assert!(health_get(&shared).slots.is_none());
    let SubDelta::Health { rows, .. } = subscribe(&shared, Topic::Health) else {
        panic!("health topic must survive an unavailable engine")
    };
    assert!(rows[0].slots.is_none());
}
