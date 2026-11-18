use super::*;
use crate::views::tests::{Fixture, git};
use corral_engine::{NewTask, Op};
use std::fs;

#[test]
fn plan_child_rows_keep_both_sessions_and_closed_dependency_statuses() {
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
    let (children, facts) = engine
        .store
        .read_snapshot(|| {
            let children = plan_child_rows(&engine, &parent)?;
            let facts = task_facts(&engine, &children)?;
            assert_eq!(
                facts[0].dependencies,
                [
                    (dropped.as_str(), Some(Status::Dropped)),
                    (done.as_str(), Some(Status::Done))
                ]
            );
            let facts = facts
                .into_iter()
                .map(|f| (f.max_rounds, f.reviewer))
                .collect::<Vec<_>>();
            Ok((children, facts))
        })
        .unwrap();
    let [member] = children.as_slice() else {
        panic!("expected just the plan member")
    };
    assert_eq!(member.task.id, id);
    assert_eq!(member.task.status, Status::Review);
    assert_eq!(member.run.as_ref().unwrap().id, worker.id);
    assert_eq!(facts[0].0, 0);
    assert_eq!(facts[0].1.as_ref().unwrap().id, reviewer.id);
    assert_eq!(
        engine
            .config
            .task_max_rounds(&engine.store.require_task(&parent).unwrap()),
        4
    );
}

#[test]
fn project_counts_and_slots_follow_dispatch_and_resolve_unset_base() {
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
branch_prefix = "work/"
max_rounds = 5
[projects.demo]
path = "repo"
auto_dispatch = false
running = 2
[projects.unlimited]
path = "repo"
base = "explicit"
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
    let engine = fixture.shared.engine().unwrap();
    let slot = slots(&engine).unwrap();
    assert_eq!(slot.global, SlotCount { used: 3, limit: 7 });
    assert_eq!(
        slot.projects,
        BTreeMap::from([("demo", SlotCount { used: 2, limit: 2 })])
    );
    let projects = project_facts(&engine).unwrap();
    let demo = &projects["demo"];
    for status in [
        Status::Running,
        Status::Blocked,
        Status::Review,
        Status::Ready,
    ] {
        assert!(
            demo.open_by_status
                .contains(&StatusCount { status, count: 1 })
        );
    }
    assert_eq!(demo.open_by_status.len(), 4);
    assert!(projects["unlimited"].open_by_status.is_empty());
    assert_eq!(demo.effective.base.as_deref(), Some("trunk"));
    assert_eq!(
        projects["unlimited"].effective.base.as_deref(),
        Some("explicit")
    );
    assert_eq!(demo.effective.profile.as_deref(), Some("generic"));
    assert_eq!(demo.effective.branch_prefix, "work/");
    assert!(!demo.effective.auto_dispatch);
    assert!(projects["unlimited"].effective.auto_dispatch);
    assert_eq!(demo.effective.max_rounds, 5);
}
