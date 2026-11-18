use super::*;
use crate::model::daemon::{Feed, Link, Task, bare_task, task_row};
use crate::model::effect::{Conn, Effect, Env};
use crate::model::projects::{NowGroup, NowScope, ProjectFilter};
use crate::model::{Form, Id, Live, Selection};
use corral_types::{Blocked, Next, Op, Status};

fn task(id: &str, project: Option<&str>, status: Status) -> Task {
    let mut task = bare_task(id, id, status, jiff::Timestamp::UNIX_EPOCH);
    task.project = project.map(str::to_owned);
    Task::new(task_row(task))
}

fn blocked(id: &str) -> Task {
    let mut task = bare_task(id, id, Status::Blocked, jiff::Timestamp::UNIX_EPOCH);
    task.blocked = Some(Blocked {
        reason: "dependency T-1 is not done".into(),
        detail: Some("T-1 is still running".into()),
        next: vec![
            Next::run(Op::Reply {
                id: id.into(),
                text: String::new(),
            }),
            Next::force(Op::Dispatch {
                id: id.into(),
                profile: None,
                base: None,
            }),
        ],
    });
    Task::new(task_row(task))
}

fn feed(tasks: Vec<Task>) -> Feed {
    let mut feed = Feed::default();
    feed.board_received(1, tasks, &[], true);
    feed.link = Link::Up;
    feed
}

fn page(state: &State, feed: &Feed, selection: &Selection, filter: &ProjectFilter) -> NowPage {
    now_page(
        state,
        feed,
        selection,
        Form::Desktop,
        filter,
        NowGroup::State,
    )
}

/// Now opens at the blocked task, whose card offers its recommended reply as a reply box and
/// its other next steps as buttons; the running work's heading reads the slots in use.
#[test]
fn a_blocked_task_needs_you_with_its_reply_box_and_its_other_next_steps() {
    let mut feed = feed(vec![
        task("T-1", None, Status::Running),
        blocked("T-2"),
        task("T-3", None, Status::Backlog),
    ]);
    feed.health = Some(corral_protocol::Health {
        slots: Some(corral_protocol::health::Slots {
            global: corral_protocol::health::SlotCount { used: 2, limit: 5 },
            ..Default::default()
        }),
        ..corral_protocol::Health::down()
    });
    let state = State::default();
    let page = page(&state, &feed, &Selection::default(), &ProjectFilter::All);
    assert_eq!(page.opens_at, Some(Id::from("T-2")));
    let selection = Selection {
        task: page.opens_at.clone(),
        ..Selection::default()
    };
    let page = super::now_page(
        &state,
        &feed,
        &selection,
        Form::Desktop,
        &ProjectFilter::All,
        NowGroup::State,
    );
    let keys: Vec<_> = page.sections.iter().map(|s| s.key.as_str()).collect();
    assert_eq!(keys, ["you", "workers"]);
    assert_eq!(page.sections[1].slots.as_deref(), Some("2 of 5 slots"));
    assert_eq!(
        page.sections[0].rows[0].line.now,
        "dependency T-1 is not done"
    );
    assert_eq!(page.waiting.groups[0].key, GroupKey::Backlog);
    let NowPanel::Task(Inspector::LiveStory(story)) = &page.panel else {
        panic!("the selected blocked task shows its story");
    };
    let card = story.blocked.as_ref().expect("blocked card");
    assert_eq!(card.detail.as_deref(), Some("T-1 is still running"));
    let reply = card.reply.as_ref().expect("the reply step is a reply box");
    assert_eq!((reply.index, reply.send.as_str()), (0, "Reply"));
    let next: Vec<_> = card
        .next
        .iter()
        .map(|button| (button.label.as_ref(), button.action.clone()))
        .collect();
    assert_eq!(
        next,
        [(
            "Dispatch anyway",
            Some(Press::Next {
                task: Id::from("T-2"),
                index: 1
            })
        )]
    );
}

/// Each dependency reads its served status, a closed one included; an id corrald does not know
/// is said plainly and opens nothing.
#[test]
fn dependencies_show_their_status_and_an_unknown_id_plainly() {
    let mut waiting = bare_task("T-5", "T-5", Status::Ready, jiff::Timestamp::UNIX_EPOCH);
    waiting.depends_on = vec!["T-1".into(), "T-2".into(), "T-9".into()];
    let mut row = task_row(waiting);
    row.dependencies = Some(vec![
        corral_protocol::rows::DependencyStatus {
            id: "T-1".into(),
            status: Some(Status::Done),
        },
        corral_protocol::rows::DependencyStatus {
            id: "T-2".into(),
            status: Some(Status::Dropped),
        },
        corral_protocol::rows::DependencyStatus {
            id: "T-9".into(),
            status: None,
        },
    ]);
    let feed = feed(vec![Task::new(row)]);
    let selection = Selection {
        task: Some(Id::from("T-5")),
        ..Selection::default()
    };
    let page = page(&State::default(), &feed, &selection, &ProjectFilter::All);
    let NowPanel::Task(Inspector::LiveStory(story)) = &page.panel else {
        panic!("the selected task shows its story");
    };
    let Some((_, Fact::Tasks(needs))) = story.facts.iter().find(|(label, _)| *label == "Needs")
    else {
        panic!("the story lists what the task needs");
    };
    let shown: Vec<_> = needs
        .iter()
        .map(|dependency| {
            (
                dependency.id.as_str(),
                dependency.status,
                dependency.action.is_some(),
            )
        })
        .collect();
    assert_eq!(
        shown,
        [
            ("T-1", "done", true),
            ("T-2", "dropped", true),
            ("T-9", "unknown task", false)
        ]
    );
}

#[test]
fn project_scope_counts_what_waits_elsewhere_and_keeps_the_escape_to_all() {
    let mut blocked_elsewhere = blocked("T-9");
    blocked_elsewhere.row.task.project = Some("site".into());
    let blocked_elsewhere = Task::new(blocked_elsewhere.row);
    let feed = feed(vec![
        task("T-1", Some("corral"), Status::Running),
        blocked_elsewhere,
    ]);
    let scope = ProjectFilter::Project(Id::from("corral"));
    let page = page(&State::default(), &feed, &Selection::default(), &scope);
    let ids: Vec<_> = page
        .sections
        .iter()
        .flat_map(|section| &section.rows)
        .map(|row| row.line.id.as_str())
        .collect();
    assert_eq!(ids, ["T-1"]);
    assert!(page.scope_note.is_some());
    assert_eq!(
        page.scope_action.map(|action| action.filter),
        Some(ProjectFilter::All)
    );
}

#[test]
fn keys_walk_moving_tasks_then_open_waiting_and_escape_restores_the_digest() {
    let feed = feed(vec![
        blocked("T-1"),
        task("T-2", None, Status::Running),
        task("T-3", None, Status::Ready),
    ]);
    let env = Env {
        conn: Conn::Healthy,
        form: Form::Desktop,
    };
    let scope = NowScope {
        filter: &ProjectFilter::All,
        grouping: NowGroup::State,
    };
    let mut state = State::default();
    let mut selection = Selection::default();
    let walk = |state: &mut State, selection: &mut Selection, key: &str| {
        for effect in state
            .key_now_page(key, None, selection, &feed, env, scope)
            .unwrap_or_default()
        {
            if let Effect::Select(Live::Task(id)) = effect {
                selection.task = Some(id);
            }
        }
        selection.task.clone()
    };
    assert_eq!(walk(&mut state, &mut selection, "j"), Some(Id::from("T-1")));
    assert_eq!(walk(&mut state, &mut selection, "j"), Some(Id::from("T-2")));
    // Waiting is folded: the walk stays on the last moving task.
    assert_eq!(walk(&mut state, &mut selection, "j"), Some(Id::from("T-2")));
    state.act(Action::ToggleWaiting);
    assert_eq!(walk(&mut state, &mut selection, "j"), Some(Id::from("T-3")));
    walk(&mut state, &mut selection, "Escape");
    assert!(matches!(
        page(&state, &feed, &selection, &ProjectFilter::All).panel,
        NowPanel::Digest(_)
    ));
}
