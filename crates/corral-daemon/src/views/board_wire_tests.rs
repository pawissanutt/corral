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
        &crate::conn::Peer::Unix,
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
    // Long titles keep the two children on separate plan pages. The member's document is as
    // long again, more than a page holds beside them: the plan serves no child's document.
    let filler = "a".repeat(16 * 1024);
    stored.title = filler.clone();
    stored.body = filler.clone();
    engine.store.update_task(&stored).unwrap();
    let sibling = fixture.task(NewTask {
        title: "b".repeat(16 * 1024),
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
    let served = delta.rows.iter().find(|row| row.task.id == id).unwrap();
    assert_eq!(served.task.body, filler);
    let mut without_document = served.clone();
    without_document.task.body.clear();
    assert_eq!(&without_document, member);
    let parent = delta.rows.iter().find(|row| row.task.id == parent).unwrap();
    assert_eq!(parent.max_rounds, Some(4));
    assert!(parent.dependencies.as_ref().unwrap().is_empty());
    assert!(parent.reviewer.is_none());
}

#[test]
fn running_board_serves_only_the_current_gate_and_preserves_attempts() {
    use corral_types::CheckResult;

    let fixture = Fixture::new();
    fixture.project();
    let command = "test -f .gate-pass";
    fs::write(
        fixture.shared.root().join(".corral-next/corral.toml"),
        format!(
            "[runner]\nmode = 'process'\n[projects.demo]\npath = 'repo'\ngate = ['{command}']\n"
        ),
    )
    .unwrap();
    let id = fixture.named_task("Gate before submitting", Some("demo"));
    let run = fixture.dispatch(&id);
    let engine = fixture.shared.engine().unwrap();
    let gate = || {
        let SubDelta::Tasks { delta } = subscribe(&fixture.shared, Topic::Tasks) else {
            panic!("expected tasks");
        };
        let row = delta
            .rows
            .into_iter()
            .find(|row| row.task.id == id)
            .unwrap();
        assert_eq!(row.task.status, Status::Running);
        assert!(row.run.unwrap().head_sha.is_none());
        row.gate
    };
    fixture.apply(Op::Gate { id: id.clone() });
    let failed = gate().unwrap();
    assert_eq!(failed.result, CheckResult::Fail);
    assert_eq!(failed.notes.as_deref(), Some(command));
    assert!(failed.log.is_some());

    git(
        &run.worktree,
        &["commit", "--allow-empty", "-qm", "next candidate"],
    );
    fixture.shared.publish();
    assert!(gate().is_none());
    git(&run.worktree, &["reset", "--hard", &failed.key]);

    let mut task = engine.store.require_task(&id).unwrap();
    task.round += 1;
    engine.store.update_task(&task).unwrap();
    fixture.shared.publish();
    assert!(
        gate().is_none(),
        "the same HEAD failed only in an older round"
    );

    // Retry resets the round counter and reuses the checkout. The previous worker's
    // failure must not appear current even though the HEAD and round match again.
    let replacement = fixture
        .apply(Op::Retry {
            id: id.clone(),
            profile: None,
            fresh: false,
            note: None,
        })
        .run
        .unwrap();
    assert_ne!(replacement.id, run.id);
    assert!(gate().is_none());

    fixture.apply(Op::Gate { id: id.clone() });
    let failed_again = gate().unwrap();
    assert_eq!(failed_again.result, CheckResult::Fail);
    assert_eq!(failed_again.key, failed.key);
    assert_eq!(failed_again.round, failed.round);
    fs::write(replacement.worktree.join(".gate-pass"), "").unwrap();
    fixture.apply(Op::Gate { id: id.clone() });
    let passed = gate().unwrap();
    assert_eq!(passed.result, CheckResult::Pass);
    assert_eq!(
        engine.store.list_checks(&id).unwrap(),
        [failed, failed_again, passed]
    );
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
running = 3
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
            running: Some(3),
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
            global: SlotCount { used: 4, limit: 7 },
            projects: BTreeMap::from([("demo".into(), SlotCount { used: 3, limit: 3 })]),
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

#[test]
fn served_worker_profile_uses_dispatch_routing_without_a_flag() {
    let fixture = Fixture::new();
    fs::write(
        fixture.shared.root().join(".corral-next/corral.toml"),
        r#"
[defaults]
profile = "generic"
[projects.demo]
path = "."
profile = "claude"
[[routing]]
title_regex = "(?i)frontend"
project = "demo"
profile = "omo"
"#,
    )
    .unwrap();
    let routed = fixture.named_task("Build the Frontend", Some("demo"));
    let project = fixture.named_task("Backend", Some("demo"));
    let default = fixture.named_task("Not assigned", None);
    let pinned = fixture.task(NewTask {
        title: "Frontend with a pin".into(),
        project: Some("demo".into()),
        profile: Some("omp".into()),
        ..Default::default()
    });
    let SubDelta::Tasks { delta } = subscribe(&fixture.shared, Topic::Tasks) else {
        panic!("expected tasks");
    };
    for (id, profile) in [
        (routed, "omo"),
        (project, "claude"),
        (default, "generic"),
        (pinned, "omp"),
    ] {
        let row = delta.rows.iter().find(|row| row.task.id == id).unwrap();
        assert_eq!(row.resolved_profile.as_deref(), Some(profile));
    }
}

#[test]
fn served_project_branches_are_local_recency_ordered_and_bounded() {
    use std::{
        io::Write,
        process::{Command, Stdio},
    };

    let fixture = Fixture::new();
    fixture.project();
    let repo = fixture.shared.root().join("repo");
    let old = Command::new("git")
        .current_dir(&repo)
        .args(["commit-tree", "HEAD^{tree}", "-m", "old"])
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_DATE", "2000-01-01T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2000-01-01T00:00:00Z")
        .output()
        .unwrap();
    assert!(old.status.success());
    let old = std::str::from_utf8(&old.stdout).unwrap().trim();
    git(
        &repo,
        &["update-ref", "refs/remotes/origin/unlisted", "HEAD"],
    );

    let mut refs = Command::new("git")
        .current_dir(&repo)
        .args(["update-ref", "--stdin"])
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    {
        let mut input = refs.stdin.take().unwrap();
        for n in 0..100 {
            writeln!(input, "create refs/heads/archive/{n:03} {old}").unwrap();
        }
    }
    assert!(refs.wait().unwrap().success());

    let SubDelta::Projects { delta } = subscribe(&fixture.shared, Topic::Projects) else {
        panic!("expected projects");
    };
    let branches = delta.rows[0].local_branches.as_ref().unwrap();
    assert_eq!(branches.len(), 100);
    assert_eq!(branches[0], "main");
    assert_eq!(branches[1], "archive/000");
    assert_eq!(branches[99], "archive/098");
}
