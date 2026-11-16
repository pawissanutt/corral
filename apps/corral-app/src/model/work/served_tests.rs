use super::*;
use crate::model::board::Mark;
use crate::model::effect::Conn;

/// The unscoped workspace: no project chosen, grouped by state.
const ALL: ProjectFilter = ProjectFilter::All;
const BY_STATE: NowGroup = NowGroup::State;
const SCOPE: NowScope<'_> = NowScope {
    filter: &ALL,
    grouping: BY_STATE,
};

fn task(id: &str, group: daemon::Group) -> daemon::Task {
    daemon::Task {
        id: Id::from(id),
        title: format!("task {id}"),
        project: None,
        plan: None,
        goal: None,
        group,
        rail: std::array::from_fn(|_| daemon::Step {
            mark: Mark::Next,
            sha7: None,
            at: None,
        }),
        now: String::new(),
        now_code: String::new(),
        since: None,
        age_since: None,
        held_reason: None,
        questions: 0,
        attention: Vec::new(),
        agent: None,
        cost: None,
        cost_estimated: false,
        rev: 1,
    }
}

fn owner_move(task: &str) -> corral_protocol::rows::InboxRow {
    use corral_protocol::rows::*;
    InboxRow {
        id: task.into(),
        kind: InboxKind::OwnerDecision,
        task: Some(task.into()),
        plan: None,
        project: None,
        title: String::new(),
        from: String::new(),
        asked_at: None,
        needed_by: None,
        blocking: true,
        state: InboxState::Open,
        question_rev: None,
        answerable: false,
        unanswerable_reason: None,
        rev: 1,
        attention: Some(AttentionMove {
            whose_move: AttentionActor::Owner,
            reason: "decision".into(),
            next_step: "decide".into(),
            command: None,
            destination: AttentionDestination::Now,
        }),
    }
}

fn ids(list: &GroupedList, key: GroupKey) -> Vec<String> {
    list.groups
        .iter()
        .filter(|group| group.key == key)
        .flat_map(|group| group.rows.iter())
        .filter_map(|row| match row {
            Row::Task(task) => Some(task.line.id.clone()),
            _ => None,
        })
        .collect()
}

fn selected_after(effects: Option<Vec<Effect>>, current: &mut Option<Id>) {
    for effect in effects.unwrap_or_default() {
        if let Effect::Select(Live::Task(id)) = effect {
            *current = Some(id);
        }
    }
}

#[test]
fn unenrolled_or_revoked_device_hides_board_rows() {
    use crate::model::devices::{DeviceLink, ENROL_FIRST, REVOKED};

    let state = State::default();
    let mut feed = daemon::Feed::default();
    feed.board_received(
        1,
        vec![
            task("T-1", daemon::Group::Queued),
            task("T-2", daemon::Group::NeedsYou),
        ],
        &[],
        true,
    );
    for (device, notice) in [
        (DeviceLink::Unenrolled, ENROL_FIRST),
        (DeviceLink::Revoked { id: "dev-1".into() }, REVOKED),
    ] {
        feed.device = device;
        let list = daemon_list_filtered(
            &state,
            &feed,
            &Selection::default(),
            Form::Desktop,
            &ALL,
            BY_STATE,
        );
        assert_eq!(list.notice, Some(notice));
        assert!(list.groups.is_empty());
    }
    feed.revoked("dev-1".into());
    assert_eq!(feed.link, daemon::Link::Down);
    assert!(!feed.board.complete);
    assert!(feed.board.tasks().is_empty());
}

#[test]
fn daemon_list_groups_rows_by_who_acts_next_and_puts_other_under_queued() {
    let tasks = [
        task("T-1", daemon::Group::Other),
        task("T-2", daemon::Group::Producing),
        task("T-3", daemon::Group::NeedsYou),
        task("T-4", daemon::Group::Queued),
    ];
    let refs: Vec<&daemon::Task> = tasks.iter().collect();
    let mut feed = daemon::Feed::default();
    feed.inbox.insert("T-3".into(), owner_move("T-3"));
    let list = list(
        &State::default(),
        &refs,
        None,
        &feed,
        Form::Desktop,
        &ProjectFilter::All,
        NowGroup::State,
    );
    let keys: Vec<GroupKey> = list.groups.iter().map(|group| group.key).collect();
    assert_eq!(keys, [GroupKey::You, GroupKey::Worker, GroupKey::Queued]);
    assert_eq!(ids(&list, GroupKey::Queued), ["T-1", "T-4"]);
    assert_eq!(ids(&list, GroupKey::You), ["T-3"]);
}

#[test]
fn collapsed_queued_rows_stay_out_of_keyboard_reach_until_expanded() {
    let tasks: Vec<daemon::Task> = (1..=COLLAPSED + 2)
        .map(|n| task(&format!("T-{n}"), daemon::Group::Queued))
        .collect();
    let refs: Vec<&daemon::Task> = tasks.iter().collect();
    let feed = daemon::Feed::default();
    let mut state = State::default();
    let mut current = Some(tasks[0].id.clone());
    for _ in 0..tasks.len() {
        let effects = state.key_tasks("j", current.as_ref(), &refs, &feed, Form::Desktop, SCOPE);
        selected_after(effects, &mut current);
    }
    assert_eq!(current, Some(tasks[COLLAPSED - 1].id.clone()));
    state.act(Action::ShowAll(GroupKey::Queued));
    for _ in 0..tasks.len() {
        let effects = state.key_tasks("j", current.as_ref(), &refs, &feed, Form::Desktop, SCOPE);
        selected_after(effects, &mut current);
    }
    assert_eq!(current, Some(tasks[tasks.len() - 1].id.clone()));
}

#[test]
fn an_off_board_selection_shows_its_own_detail_and_never_aliases_the_first_row() {
    use corral_protocol::board::{RailGlyph, RailStep};
    use corral_protocol::detail::{DetailBody, TaskDetail};
    let mut feed = daemon::Feed::default();
    feed.board_received(
        1,
        vec![
            task("T-5", daemon::Group::Queued),
            task("T-6", daemon::Group::Verifying),
        ],
        &[],
        true,
    );
    let target = super::super::task_ref("T-404");
    feed.details.insert(
        daemon::detail_key(&target),
        daemon::DetailCache {
            target,
            pages: Vec::new(),
            loading: false,
            stale: false,
            error: None,
            merged: Some(DetailBody::Task(TaskDetail {
                id: "T-404".into(),
                owner_revision: "r1".into(),
                title: "landed".into(),
                status: "done".into(),
                body_excerpt: String::new(),
                stages: std::array::from_fn(|_| RailStep {
                    glyph: RailGlyph::Pass,
                    sha7: None,
                    at: None,
                }),
                dependencies: Vec::new(),
                progress: None,
                owner_next: None,
                hold: None,
                park: None,
                park_revision: None,
                artifacts: Vec::new(),
                run: None,
                plan_context: None,
                log_tail: Vec::new(),
            })),
        },
    );
    let closed = Id::from("T-404");
    let selected = Selection {
        task: Some(closed.clone()),
        ..Selection::default()
    };
    let mut state = State::default();
    let list = daemon_list_filtered(&state, &feed, &selected, Form::Desktop, &ALL, BY_STATE);
    assert!(
        !list
            .groups
            .iter()
            .flat_map(|group| group.rows.iter())
            .any(|row| matches!(row, Row::Task(row) if row.selected))
    );
    let inspector = super::super::daemon_inspector_filtered(
        &state,
        &feed,
        &selected,
        Form::Desktop,
        &ALL,
        BY_STATE,
    );
    assert!(matches!(
        &inspector,
        super::super::Inspector::LiveStory(story) if story.id == "T-404"
    ));
    // The list keys enter the list from its top.
    let mut current = Some(closed);
    let effects = state.key_live_filtered(
        "j",
        None,
        &selected,
        &feed,
        Env {
            conn: Conn::Healthy,
            form: Form::Desktop,
        },
        SCOPE,
    );
    selected_after(effects, &mut current);
    assert_eq!(current.as_deref(), Some("T-6"));
}

#[test]
fn rows_past_the_seen_cursor_carry_the_changed_mark() {
    use corral_protocol::app::{ChangeLine, SeenCursor};
    let mut feed = daemon::Feed::default();
    feed.board_received(
        1,
        vec![
            task("T-1", daemon::Group::Queued),
            task("T-2", daemon::Group::Queued),
        ],
        &[],
        true,
    );
    for (seq, id) in [(4, "T-1"), (9, "T-2")] {
        feed.changes.insert(
            seq,
            ChangeLine {
                seq,
                at: "2026-11-14T10:00:00Z".into(),
                class: "queue".into(),
                task: Some(id.into()),
                kind: corral_protocol::app::ChangeKind::Other,
                from: None,
                to: None,
                failed: false,
                text: format!("{id} · queued"),
                actor: "corrald".into(),
                via: "daemon".into(),
            },
        );
    }
    feed.seen = Some(SeenCursor {
        through: 5,
        at: None,
    });
    let list = daemon_list_filtered(
        &State::default(),
        &feed,
        &Selection::default(),
        Form::Desktop,
        &ALL,
        BY_STATE,
    );
    let changed: Vec<String> = list
        .groups
        .iter()
        .flat_map(|group| group.rows.iter())
        .filter_map(|row| match row {
            Row::Task(task) if task.line.changed => Some(task.line.id.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(changed, ["T-2"]);
}

#[test]
fn task_attention_routes_inspector_and_enter_without_inventing_questions() {
    use corral_protocol::rows::{InboxKind, InboxRow, InboxState};
    let row_task = task("T-1", daemon::Group::NeedsYou);
    let tasks = [&row_task];
    let mut state = State::default();
    for (kind, expected, question) in [
        (InboxKind::Review, Live::Review(Id::new("T-1")), false),
        (InboxKind::PlanAcceptance, Live::Plan(Id::new("T-9")), false),
        (InboxKind::Stuck, Live::Task(Id::new("T-1")), false),
        (
            InboxKind::WorkerQuestion,
            Live::Question(Id::new("item")),
            true,
        ),
    ] {
        let mut feed = daemon::Feed::default();
        feed.inbox.insert(
            Id::new("item"),
            InboxRow {
                id: "item".into(),
                kind,
                task: Some("T-1".into()),
                plan: Some("T-9".into()),
                project: None,
                title: String::new(),
                from: String::new(),
                asked_at: None,
                needed_by: None,
                blocking: true,
                state: InboxState::Open,
                question_rev: question.then_some(1),
                answerable: question,
                unanswerable_reason: None,
                attention: Some(corral_protocol::rows::AttentionMove {
                    whose_move: corral_protocol::rows::AttentionActor::Owner,
                    reason: "owner action".into(),
                    next_step: "decide".into(),
                    command: None,
                    destination: match kind {
                        InboxKind::Review => corral_protocol::rows::AttentionDestination::Review,
                        InboxKind::PlanAcceptance => {
                            corral_protocol::rows::AttentionDestination::Plans
                        }
                        InboxKind::Stuck => corral_protocol::rows::AttentionDestination::Now,
                        _ => corral_protocol::rows::AttentionDestination::Inbox,
                    },
                }),
                rev: 1,
            },
        );
        assert_eq!(task_question(&feed, "T-1").is_some(), question);
        let story = super::super::live_story(&state, &feed, &row_task, Form::Desktop);
        let button = story
            .facts
            .iter()
            .find_map(|(_, fact)| match fact {
                super::super::Fact::Answer(button) => Some(button),
                _ => None,
            })
            .unwrap();
        assert_eq!(button.action, Some(Press::Open(expected.clone())));
        let effects = state
            .key_tasks("Enter", None, &tasks, &feed, Form::Desktop, SCOPE)
            .unwrap();
        assert_eq!(effects, [Effect::OpenLive(expected)]);
    }
}

#[test]
fn board_topic_failure_stops_loading_and_marks_retained_rows_last_known() {
    use crate::model::daemon::{ClientError, TopicState};
    let mut feed = daemon::Feed {
        link: Link::Up,
        ..daemon::Feed::default()
    };
    let mut state = State::default();
    let selected = Selection::default();
    feed.topics.insert(
        "board".into(),
        TopicState {
            loading: false,
            stale: true,
            error: Some(ClientError {
                code: "failed".into(),
                message: "topic rejected".into(),
            }),
        },
    );
    let list = daemon_list_filtered(&state, &feed, &selected, Form::Desktop, &ALL, BY_STATE);
    assert!(list.notice.is_some());
    assert!(list.groups.is_empty());
    let status = super::super::daemon_status(&feed);
    let banner = status.banner.unwrap();
    assert!(format!("{:?}", banner.text).contains("topic rejected"));
    assert_eq!(banner.actions[0].action, Some(Press::RetryBoard));
    assert_eq!(
        state.press(
            Press::RetryBoard,
            &feed,
            Env {
                conn: Conn::Healthy,
                form: Form::Desktop,
            }
        ),
        [Effect::RetryTopic("board".into())]
    );
    assert!(
        super::super::daemon_live_filtered(&state, &feed, &selected, Form::Desktop, &ALL, BY_STATE)
            .selected_note
            .is_some()
    );
    assert!(
        super::super::daemon_plan_filtered(&state, &feed, &selected, Form::Desktop, &ALL, BY_STATE)
            .empty
            .is_some()
    );

    feed.board_received(1, vec![task("T-1", daemon::Group::Queued)], &[], true);
    assert!(
        !daemon_list_filtered(&state, &feed, &selected, Form::Desktop, &ALL, BY_STATE)
            .groups
            .is_empty()
    );
    assert!(super::super::daemon_status(&feed).dimmed);
    assert!(super::super::daemon_status(&feed).as_of.is_some());
    feed.topics.insert(
        "board".into(),
        TopicState {
            loading: true,
            ..TopicState::default()
        },
    );
    assert!(super::super::daemon_status(&feed).dimmed);
    feed.topics.insert("board".into(), TopicState::default());
    assert!(!super::super::daemon_status(&feed).dimmed);
    assert!(super::super::daemon_status(&feed).banner.is_none());
}

#[test]
fn next_tab_key_and_enter_keep_the_live_session_in_now() {
    let mut state = State::default();
    let mut row = task("T-1", daemon::Group::Producing);
    row.agent = Some(daemon::Agent {
        session: Some("worker-1".into()),
        role: "worker".into(),
        profile: None,
        runtime: None,
        machine: None,
    });
    let feed = daemon::Feed::default();
    assert_eq!(
        state.key_tasks("]", None, &[&row], &feed, Form::Desktop, SCOPE),
        Some(vec![])
    );
    assert_eq!(state.now_tab, super::super::NowTab::Evidence);
    assert!(
        state
            .key_tasks("Tab", None, &[&row], &feed, Form::Desktop, SCOPE)
            .is_none()
    );
    let effects = state.key_tasks("Enter", None, &[&row], &feed, Form::Desktop, SCOPE);
    assert_eq!(effects, Some(vec![]));
    assert_eq!(state.now_tab, super::super::NowTab::Session);
}

#[test]
fn closed_rows_join_the_list_keys_only_while_the_section_is_open() {
    use corral_protocol::app::CLOSED_V1;
    use corral_protocol::detail::{ClosedPage, ClosedTask, DetailBody, DetailRef};
    let mut feed = daemon::Feed::default();
    feed.capabilities.insert(CLOSED_V1.into());
    feed.board_received(1, vec![task("T-1", daemon::Group::Queued)], &[], true);
    let target = DetailRef::Closed { before: None };
    feed.details.insert(
        daemon::detail_key(&target),
        daemon::DetailCache {
            target,
            pages: Vec::new(),
            loading: false,
            stale: false,
            error: None,
            merged: Some(DetailBody::Closed(ClosedPage {
                total: Some(1),
                rows: vec![ClosedTask {
                    id: "T-9".into(),
                    title: "landed".into(),
                    status: "done".into(),
                    updated: "2026-11-14T10:00:00Z".into(),
                    project: None,
                    plan: None,
                    goal: None,
                }],
                older: None,
            })),
        },
    );
    let mut state = State::default();
    let tasks = shown(&state, &feed, &ProjectFilter::All);
    let mut current = Some(Id::from("T-1"));
    let effects = state.key_tasks("j", current.as_ref(), &tasks, &feed, Form::Desktop, SCOPE);
    selected_after(effects, &mut current);
    assert_eq!(current.as_deref(), Some("T-1"));

    state.closed.press(super::super::ClosedPress::Toggle);
    let effects = state.key_tasks("j", current.as_ref(), &tasks, &feed, Form::Desktop, SCOPE);
    selected_after(effects, &mut current);
    assert_eq!(current.as_deref(), Some("T-9"));
    let effects = state.key_tasks(
        "Enter",
        current.as_ref(),
        &tasks,
        &feed,
        Form::Desktop,
        SCOPE,
    );
    assert_eq!(
        effects,
        Some(vec![Effect::OpenLive(Live::Task(Id::from("T-9")))])
    );
    let effects = state.key_tasks("k", current.as_ref(), &tasks, &feed, Form::Desktop, SCOPE);
    selected_after(effects, &mut current);
    assert_eq!(current.as_deref(), Some("T-1"));
}
