use corral_engine::{Actor, EventKind, Role, Run, Status, Task, events::NewEvent};
use corral_protocol::detail::{DetailBody, DetailGet, DetailRef, TimelineBy};

use crate::views::{detail, tests::Fixture};

#[test]
fn timeline_keeps_unworked_children_and_out_of_window_dependencies_without_stale_landings() {
    let fixture = Fixture::new();
    let engine = fixture.shared.engine().unwrap();
    let last_landing = "2026-02-01T12:00:00Z".parse().unwrap();
    engine
        .store
        .transaction(|store| {
            for (id, title, status, parent, depends_on) in [
                ("T-1", "Plan", Status::Backlog, None, vec![]),
                (
                    "T-2",
                    "Worked child",
                    Status::Backlog,
                    Some("T-1"),
                    vec!["T-4"],
                ),
                (
                    "T-3",
                    "Unworked child",
                    Status::Backlog,
                    Some("T-1"),
                    vec![],
                ),
                ("T-4", "Dependency", Status::Done, None, vec![]),
                ("T-5", "Reopened", Status::Backlog, None, vec![]),
            ] {
                store.insert_task(&Task {
                    id: id.into(),
                    title: title.into(),
                    status,
                    parent: parent.map(str::to_owned),
                    depends_on: depends_on.into_iter().map(str::to_owned).collect(),
                    ..Task::default()
                })?;
            }
            // A saved run seeds T-2 independently of any in-window events for it.
            store.insert_run(&Run {
                id: "R1".into(),
                task: "T-2".into(),
                role: Role::Worker,
                profile: "worker".into(),
                harness: String::new(),
                model: None,
                effort: None,
                worktree: Default::default(),
                branch: String::new(),
                base: String::new(),
                base_sha: String::new(),
                session: None,
                started: "2026-02-02T08:00:00Z".parse().unwrap(),
                ended: Some("2026-02-02T09:00:00Z".parse().unwrap()),
                outcome: None,
                summary: None,
                head_sha: None,
                quiet_since: None,
            })?;
            for (task, kind, at) in [
                (
                    "T-4",
                    EventKind::Landed,
                    "2026-02-01T08:00:00Z".parse().unwrap(),
                ),
                ("T-4", EventKind::Landed, last_landing),
                (
                    "T-5",
                    EventKind::Landed,
                    "2026-02-02T12:00:00Z".parse().unwrap(),
                ),
                (
                    "T-5",
                    EventKind::Reopened,
                    "2026-02-03T12:00:00Z".parse().unwrap(),
                ),
            ] {
                store.append_event_at(
                    &NewEvent {
                        task: Some(task.into()),
                        run: None,
                        kind,
                        actor: Actor::Owner,
                        detail: serde_json::Value::Null,
                    },
                    at,
                )?;
            }
            Ok(())
        })
        .unwrap();

    let reply = detail(
        &fixture.shared,
        DetailGet {
            target: DetailRef::Timeline {
                from: "2026-02-02T00:00:00Z".into(),
                to: "2026-02-03T00:00:00Z".into(),
                by: TimelineBy::Task,
            },
            page: None,
            etag: None,
        },
    )
    .unwrap();
    let Some(DetailBody::Timeline(page)) = reply.body else {
        panic!("expected timeline");
    };
    let tasks: Vec<_> = page
        .tasks
        .iter()
        .map(|task| (task.id.as_str(), task.landed))
        .collect();
    assert_eq!(
        tasks,
        [
            ("T-1", None),
            ("T-2", None),
            ("T-3", None),
            ("T-4", Some(last_landing)),
            ("T-5", None),
        ]
    );
}
