use super::*;
use crate::model::board::Mark;
use crate::model::daemon::{Feed, Group, Step, Task};
use crate::model::effect::Conn;
use crate::model::glance::StatIcon;
use crate::model::projects::{NowGroup, NowScope, ProjectFilter};
use crate::model::{Form, Selection};
use corral_protocol::app::{CHANGES_V1, ChangeKind, ChangeLine};
use corral_protocol::rows::*;

fn task(id: &str, project: &str, group: Group, marks: [Mark; 4]) -> Task {
    Task {
        id: id.into(),
        title: id.into(),
        project: Some(project.into()),
        plan: None,
        goal: None,
        group,
        rail: marks.map(|mark| Step {
            mark,
            sha7: None,
            at: None,
        }),
        now: String::new(),
        now_code: String::new(),
        age_since: None,
        since: None,
        held_reason: None,
        questions: 0,
        attention: Vec::new(),
        agent: None,
        cost: None,
        cost_estimated: false,
        rev: 1,
    }
}

fn owner_move(id: &str, task: Option<&str>, project: &str, kind: InboxKind) -> InboxRow {
    InboxRow {
        id: id.into(),
        kind,
        task: task.map(str::to_owned),
        plan: None,
        project: Some(project.into()),
        title: id.into(),
        from: "worker".into(),
        asked_at: None,
        needed_by: None,
        blocking: true,
        state: InboxState::Open,
        question_rev: None,
        answerable: true,
        unanswerable_reason: None,
        attention: Some(AttentionMove {
            whose_move: AttentionActor::Owner,
            reason: "blocked".into(),
            next_step: "decide".into(),
            command: None,
            destination: AttentionDestination::Now,
        }),
        rev: 1,
    }
}

fn feed(tasks: Vec<Task>) -> Feed {
    let mut feed = Feed::default();
    feed.board_received(1, tasks, &[], true);
    feed.link = daemon::Link::Up;
    feed.capabilities.insert(CHANGES_V1.into());
    feed
}

fn page(state: &State, feed: &Feed, filter: &ProjectFilter) -> NowPage {
    now_page(
        state,
        feed,
        &Selection::default(),
        Form::Desktop,
        filter,
        NowGroup::State,
    )
}

fn task_ids(page: &NowPage) -> Vec<&str> {
    page.sections
        .iter()
        .flat_map(|section| &section.rows)
        .filter_map(|row| match row {
            NowRow::Task(row) => Some(row.line.id.as_str()),
            _ => None,
        })
        .collect()
}

#[test]
fn canonical_owner_moves_become_single_action_asks_without_promoted_task_duplicates() {
    let mut feed = feed(vec![
        task(
            "T-1",
            "a",
            Group::Verifying,
            [Mark::Done, Mark::Active, Mark::Next, Mark::Next],
        ),
        task(
            "T-2",
            "a",
            Group::Producing,
            [Mark::Active, Mark::Next, Mark::Next, Mark::Next],
        ),
        task(
            "T-3",
            "a",
            Group::Producing,
            [Mark::Active, Mark::Next, Mark::Next, Mark::Next],
        ),
    ]);
    for (index, kind) in [
        InboxKind::WorkerQuestion,
        InboxKind::PmAsk,
        InboxKind::AssistantAsk,
        InboxKind::PlanAcceptance,
        InboxKind::Review,
        InboxKind::OwnerDecision,
    ]
    .into_iter()
    .enumerate()
    {
        let mut row = owner_move(
            &format!("q-{index}"),
            Some(if index == 0 { "T-1" } else { "T-2" }),
            "a",
            kind,
        );
        if kind == InboxKind::PlanAcceptance {
            row.plan = Some("P-1".into());
        }
        feed.inbox.insert(row.id.as_str().into(), row);
    }
    let page = page(&State::default(), &feed, &ProjectFilter::All);
    let asks: Vec<_> = page
        .sections
        .iter()
        .flat_map(|section| &section.rows)
        .filter_map(|row| match row {
            NowRow::Ask(row) => Some(row),
            _ => None,
        })
        .collect();
    assert_eq!(asks.len(), 6);
    for ask in asks {
        let source = &feed.inbox[&ask.key];
        assert_eq!(
            ask.button.action,
            crate::model::inbox::target(source).map(Press::Open)
        );
        assert!(ask.button.enabled);
    }
    assert_eq!(task_ids(&page), ["T-3"]);
    assert_eq!(
        page.stats
            .iter()
            .find(|stat| stat.icon == StatIcon::NeedsYou)
            .map(|stat| stat.value.as_str()),
        Some("6")
    );
}

#[test]
fn loop_counts_use_the_first_unpassed_rail_stage_not_the_daemon_bucket() {
    let mut feed = feed(vec![
        task(
            "T-1",
            "a",
            Group::Producing,
            [Mark::Active, Mark::Next, Mark::Next, Mark::Next],
        ),
        task(
            "T-2",
            "a",
            Group::Verifying,
            [Mark::Done, Mark::Stale, Mark::Next, Mark::Next],
        ),
        task(
            "T-3",
            "a",
            Group::Verifying,
            [Mark::Done, Mark::Done, Mark::Active, Mark::Next],
        ),
        task(
            "T-4",
            "a",
            Group::Verifying,
            [Mark::Done, Mark::Done, Mark::Next, Mark::Next],
        ),
    ]);
    let ask = owner_move("verdict", Some("T-4"), "a", InboxKind::Review);
    feed.inbox.insert(ask.id.as_str().into(), ask);
    let page = page(&State::default(), &feed, &ProjectFilter::All);
    let count = |stage, icon| {
        page.loop_band
            .stages
            .iter()
            .find(|row| row.key == stage)
            .and_then(|row| row.stats.iter().find(|stat| stat.icon == icon))
            .map(|stat| stat.value.as_str())
    };
    assert_eq!(count("run", StatIcon::Moving), Some("1"));
    assert_eq!(count("gate", StatIcon::Stale), Some("1"));
    assert_eq!(count("gate", StatIcon::Moving), None);
    assert_eq!(count("review", StatIcon::Moving), Some("1"));
    assert_eq!(count("review", StatIcon::NeedsYou), Some("1"));
    assert_eq!(
        page.loop_band
            .stages
            .iter()
            .flat_map(|stage| &stage.stats)
            .count(),
        4
    );
}

#[test]
fn project_scope_reaches_asks_tasks_waiting_loop_and_digest_and_keeps_the_escape_to_all() {
    let mut feed = feed(vec![
        task(
            "T-1",
            "a",
            Group::Producing,
            [Mark::Active, Mark::Next, Mark::Next, Mark::Next],
        ),
        task(
            "T-2",
            "b",
            Group::Producing,
            [Mark::Active, Mark::Next, Mark::Next, Mark::Next],
        ),
        task("T-3", "a", Group::Queued, [Mark::Next; 4]),
        task("T-4", "b", Group::Queued, [Mark::Next; 4]),
    ]);
    for (key, project) in [("ask-a", "a"), ("ask-b", "b")] {
        let ask = owner_move(key, None, project, InboxKind::PmAsk);
        feed.inbox.insert(key.into(), ask);
    }
    for (seq, task) in [(1, "T-1"), (2, "T-2")] {
        feed.changes.insert(
            seq,
            ChangeLine {
                seq,
                at: "2026-11-16T18:40:00Z".into(),
                class: "work".into(),
                kind: ChangeKind::Work,
                from: None,
                to: None,
                failed: false,
                task: Some(task.into()),
                text: "worker updated".into(),
                actor: String::new(),
                via: String::new(),
            },
        );
    }
    for project in ["a", "b"] {
        feed.plans.insert(
            project.into(),
            PlanRow {
                id: project.into(),
                title: project.into(),
                role: Some(PlanRole::Tracker),
                linked: None,
                state: PlanState::Executing,
                revision: None,
                delivery: None,
                members_total: 1,
                members_landed: 0,
                blockers: Vec::new(),
                accepted_at: None,
                projects: vec![project.into()],
                rev: 1,
            },
        );
    }
    let page = page(
        &State::default(),
        &feed,
        &ProjectFilter::Project("a".into()),
    );
    assert_eq!(task_ids(&page), ["T-1"]);
    assert!(
        page.sections
            .iter()
            .flat_map(|section| &section.rows)
            .all(|row| match row {
                NowRow::Ask(row) => row.project.as_deref() == Some("a"),
                NowRow::Task(row) => row.project.as_deref() == Some("a"),
            })
    );
    assert_eq!(page.waiting.total, 1);
    assert_eq!(
        page.scope_action.as_ref().map(|action| &action.filter),
        Some(&ProjectFilter::All)
    );
    assert!(page.scope_note.is_some());
    let NowPanel::Digest(digest) = &page.panel else {
        panic!("expected unselected digest");
    };
    assert_eq!(digest.events.count, 1);
    assert_eq!(
        digest
            .rows
            .iter()
            .filter_map(|row| row.task.as_deref())
            .collect::<Vec<_>>(),
        ["T-1"]
    );
    assert_eq!(page.loop_band.stages[0].stats[0].value, "1");
    assert_eq!(page.loop_band.stages[1].stats[0].value, "1");
}

#[test]
fn mixed_keys_walk_asks_then_tasks_then_open_waiting_and_escape_restores_digest() {
    let mut feed = feed(vec![
        task(
            "T-1",
            "a",
            Group::Producing,
            [Mark::Active, Mark::Next, Mark::Next, Mark::Next],
        ),
        task("T-2", "a", Group::Queued, [Mark::Next; 4]),
    ]);
    for key in ["q-1", "q-2"] {
        let ask = owner_move(key, None, "a", InboxKind::PmAsk);
        feed.inbox.insert(key.into(), ask);
    }
    let mut state = State::default();
    let mut selection = Selection::default();
    let env = Env {
        conn: Conn::Healthy,
        form: Form::Desktop,
    };
    let filter = ProjectFilter::All;
    let scope = NowScope {
        filter: &filter,
        grouping: NowGroup::State,
    };
    let walk = vec![
        NowWalk::Ask("q-1".into()),
        NowWalk::Ask("q-2".into()),
        NowWalk::Task("T-1".into()),
    ];
    assert_eq!(
        now_walk_order(&state, &feed, env.form, &filter, scope.grouping),
        walk
    );
    state.act(Action::ToggleWaiting);
    let mut open = walk.clone();
    open.push(NowWalk::Task("T-2".into()));
    assert_eq!(
        now_walk_order(&state, &feed, env.form, &filter, scope.grouping),
        open
    );
    for expected in [
        NowWalk::Ask("q-1".into()),
        NowWalk::Ask("q-2".into()),
        NowWalk::Task("T-1".into()),
    ] {
        let effects = state
            .key_now_page("j", None, &selection, &feed, env, scope)
            .unwrap_or_default();
        for effect in effects {
            if let Effect::Select(live) = effect {
                selection.set(&live);
            }
        }
        match expected {
            NowWalk::Ask(key) => assert_eq!(state.selected_ask, Some(key)),
            NowWalk::Task(id) => {
                assert_eq!(selection.task, Some(id));
                assert!(state.selected_ask.is_none());
            }
        }
    }
    state.key_now_page("k", None, &selection, &feed, env, scope);
    assert_eq!(state.selected_ask.as_deref(), Some("q-2"));
    state.key_now_page("Escape", None, &selection, &feed, env, scope);
    assert!(matches!(
        now_page(&state, &feed, &selection, env.form, &filter, scope.grouping).panel,
        NowPanel::Digest(_)
    ));
}

#[test]
fn project_grouped_keys_follow_each_projects_ask_then_task() {
    let mut feed = feed(vec![
        task(
            "T-1",
            "a",
            Group::Producing,
            [Mark::Active, Mark::Next, Mark::Next, Mark::Next],
        ),
        task(
            "T-2",
            "b",
            Group::Verifying,
            [Mark::Done, Mark::Active, Mark::Next, Mark::Next],
        ),
    ]);
    for (key, project) in [("q-a", "a"), ("q-b", "b")] {
        let ask = owner_move(key, None, project, InboxKind::PmAsk);
        feed.inbox.insert(key.into(), ask);
    }
    let mut state = State::default();
    let mut selection = Selection::default();
    let env = Env {
        conn: Conn::Healthy,
        form: Form::Desktop,
    };
    let filter = ProjectFilter::All;
    let scope = NowScope {
        filter: &filter,
        grouping: NowGroup::Project,
    };
    let expected = vec![
        NowWalk::Ask("q-a".into()),
        NowWalk::Task("T-1".into()),
        NowWalk::Ask("q-b".into()),
        NowWalk::Task("T-2".into()),
    ];
    let page = now_page(&state, &feed, &selection, env.form, &filter, scope.grouping);
    let rendered: Vec<_> = page
        .sections
        .into_iter()
        .flat_map(|section| section.rows)
        .map(|row| match row {
            NowRow::Ask(row) => NowWalk::Ask(row.key),
            NowRow::Task(row) => NowWalk::Task(row.line.id.into()),
        })
        .collect();
    assert_eq!(rendered, expected);
    assert_eq!(
        now_walk_order(&state, &feed, env.form, &filter, scope.grouping),
        expected
    );
    for expected in expected {
        let effects = state
            .key_now_page("j", None, &selection, &feed, env, scope)
            .unwrap_or_default();
        for effect in effects {
            if let Effect::Select(live) = effect {
                selection.set(&live);
            }
        }
        match expected {
            NowWalk::Ask(key) => assert_eq!(state.selected_ask, Some(key)),
            NowWalk::Task(id) => {
                assert_eq!(selection.task, Some(id));
                assert!(state.selected_ask.is_none());
            }
        }
    }
}
