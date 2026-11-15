//! The who-acts-next list and its keyboard selection in the Daemon data state, built from the
//! daemon's board: its groups in the fixture's order and anatomy, with the real rows and counts.
//! The selected task is the root's live selection, by its owned id.

use std::collections::BTreeSet;

use crate::model::board::TaskLine;
use crate::model::daemon::{self, Link};
use crate::model::effect::{Effect, Env};
use crate::model::nav::Form;
use crate::model::{Id, Live, Selection};

use super::{
    Action, Button, ButtonKind, GroupKey, GroupView, GroupedList, Press, Row, State, TaskRow,
};

/// Said instead of the list while the link is down and no board has arrived.
pub const NO_BOARD: &str = "No board received from corrald yet.";
/// Said instead of the list when the daemon's board holds no task.
pub const EMPTY_BOARD: &str = "Nothing on the board.";
/// The board failed before producing any rows; the status banner carries the reason and retry.
pub(super) const BOARD_UNAVAILABLE: &str = "Board unavailable.";
/// Said instead of the list when no task on the board serves the chosen goal.
pub const NO_GOAL_TASKS: &str = "No task on the board serves this goal.";

/// Rows a collapsed Queued or Parked group shows on desktop and terminal, and on phone.
const COLLAPSED: usize = 12;
const COLLAPSED_PHONE: usize = 4;

/// The display order of the groups, and the daemon groups each one holds; tasks the daemon
/// cannot place wait with the queued ones.
const ORDER: [(GroupKey, &[daemon::Group]); 6] = [
    (GroupKey::You, &[daemon::Group::NeedsYou]),
    (GroupKey::Corral, &[daemon::Group::Verifying]),
    (GroupKey::Worker, &[daemon::Group::Producing]),
    (
        GroupKey::Queued,
        &[daemon::Group::Queued, daemon::Group::Other],
    ),
    (GroupKey::Held, &[daemon::Group::Held]),
    (GroupKey::Parked, &[daemon::Group::Parked]),
];

/// The list group a daemon task is shown under.
pub fn group_key(group: daemon::Group) -> GroupKey {
    ORDER
        .iter()
        .find(|(_, members)| members.contains(&group))
        .map_or(GroupKey::Queued, |(key, _)| *key)
}

/// The heading of a group in the Daemon state: the fixture's, except the bucket of tasks no one
/// is acting on yet, which also holds ideas and backlog nobody queued and so is "Upcoming".
pub fn group_label(key: GroupKey) -> &'static str {
    if key == GroupKey::Queued {
        return UPCOMING;
    }
    match key {
        GroupKey::You => "Needs you",
        GroupKey::Corral => "Corral is verifying",
        GroupKey::Worker => "Worker runs in progress",
        GroupKey::Queued => UPCOMING,
        GroupKey::Held => "Held",
        GroupKey::Parked => "Parked",
    }
}

/// The Daemon state's heading of the broad bucket `GroupKey::Queued`.
pub const UPCOMING: &str = "Upcoming";

/// The board's tasks the list shows: all of them, or those serving the chosen goal.
fn shown<'a>(state: &State, tasks: &'a [daemon::Task]) -> Vec<&'a daemon::Task> {
    tasks
        .iter()
        .filter(|task| {
            state
                .goal
                .as_deref()
                .is_none_or(|goal| task.goal.as_deref() == Some(goal))
        })
        .collect()
}

pub(super) fn task_group(feed: &daemon::Feed, task: &daemon::Task) -> GroupKey {
    if crate::model::inbox::owner_moves(feed).any(|row| {
        row.task.as_deref() == Some(task.id.as_str())
            || row.plan.as_deref() == Some(task.id.as_str())
    }) {
        GroupKey::You
    } else if task.group == daemon::Group::NeedsYou {
        GroupKey::Queued
    } else {
        group_key(task.group)
    }
}

/// The non-empty groups of `tasks`, in display order, each keeping the board's order.
fn grouped<'a>(
    tasks: &[&'a daemon::Task],
    feed: &daemon::Feed,
) -> Vec<(GroupKey, Vec<&'a daemon::Task>)> {
    ORDER
        .iter()
        .map(|(key, _)| {
            let rows: Vec<&daemon::Task> = tasks
                .iter()
                .copied()
                .filter(|task| task_group(feed, task) == *key)
                .collect();
            (*key, rows)
        })
        .filter(|(_, rows)| !rows.is_empty())
        .collect()
}

/// How many rows of a group show before its `+ N more` line.
fn limit(state: &State, key: GroupKey, form: Form) -> usize {
    let expanded = match key {
        GroupKey::Queued => state.all_queued,
        GroupKey::Held => state.all_held,
        GroupKey::Parked => state.all_parked,
        GroupKey::You | GroupKey::Corral | GroupKey::Worker => return usize::MAX,
    };
    match (expanded, form) {
        (true, _) => usize::MAX,
        (false, Form::Phone) => COLLAPSED_PHONE,
        (false, Form::Desktop | Form::Terminal) => COLLAPSED,
    }
}

/// The task ids the list shows, in display order: collapsed groups hide their rest.
fn visible<'a>(
    state: &State,
    tasks: &[&'a daemon::Task],
    feed: &daemon::Feed,
    form: Form,
) -> Vec<&'a Id> {
    grouped(tasks, feed)
        .into_iter()
        .flat_map(|(key, rows)| {
            let limit = limit(state, key, form);
            rows.into_iter().take(limit).map(|task| &task.id)
        })
        .collect()
}

/// The selected task: the root's, while the list shows it, otherwise the first visible row.
fn selection<'a>(
    state: &State,
    chosen: Option<&Id>,
    tasks: &[&'a daemon::Task],
    feed: &daemon::Feed,
    form: Form,
) -> Option<&'a Id> {
    chosen
        .and_then(|id| tasks.iter().copied().find(|task| task.id == *id))
        .map(|task| &task.id)
        .or_else(|| visible(state, tasks, feed, form).first().copied())
}

/// The task the daemon's Now treats as selected on the board, if the list shows any.
pub fn daemon_selection<'a>(
    state: &State,
    selected: &Selection,
    feed: &'a daemon::Feed,
    form: Form,
) -> Option<&'a daemon::Task> {
    let tasks = shown(state, feed.board.tasks());
    let id = selection(state, selected.task.as_ref(), &tasks, feed, form)?;
    feed.board.task(id)
}

/// The tasks with an event in the changes feed past this device's seen cursor.
pub fn unseen_tasks(feed: &daemon::Feed) -> BTreeSet<&str> {
    let Some(seen) = &feed.seen else {
        return BTreeSet::new();
    };
    feed.changes
        .range(seen.through.saturating_add(1)..)
        .filter_map(|(_, line)| line.task.as_deref())
        .collect()
}

/// Who acts next in the Daemon state: loading placeholders while the first board is on its way,
/// one quiet line while there is none or it is empty, else the groups.
pub fn daemon_list(
    state: &State,
    feed: &daemon::Feed,
    selected: &Selection,
    form: Form,
) -> GroupedList {
    if let Some(line) = feed.device.board_notice() {
        return GroupedList {
            notice: Some(line),
            ..GroupedList::new(form)
        };
    }
    let board = &feed.board;
    if !board.complete {
        if super::served_facts::board_problem(feed).is_some() {
            return GroupedList {
                notice: Some(BOARD_UNAVAILABLE),
                ..GroupedList::new(form)
            };
        }
        return match feed.link {
            Link::Down => GroupedList {
                notice: Some(NO_BOARD),
                ..GroupedList::new(form)
            },
            Link::Connecting | Link::Up => GroupedList::loading(form),
        };
    }
    if board.tasks().is_empty() {
        return GroupedList {
            notice: Some(EMPTY_BOARD),
            ..GroupedList::new(form)
        };
    }
    let tasks = shown(state, board.tasks());
    if tasks.is_empty() {
        return GroupedList {
            notice: Some(NO_GOAL_TASKS),
            ..GroupedList::new(form)
        };
    }
    list(state, &tasks, selected.task.as_ref(), feed, form)
}

fn list(
    state: &State,
    tasks: &[&daemon::Task],
    chosen: Option<&Id>,
    feed: &daemon::Feed,
    form: Form,
) -> GroupedList {
    let mut view = GroupedList::new(form);
    let phone = form == Form::Phone;
    let selected = selection(state, chosen, tasks, feed, form);
    let unseen = unseen_tasks(feed);
    for (key, members) in grouped(tasks, feed) {
        let limit = limit(state, key, form);
        let mut rows: Vec<Row> = members
            .iter()
            .take(limit)
            .map(|task| {
                Row::Task(TaskRow {
                    line: TaskLine::daemon(task, feed.now, unseen.contains(task.id.as_str())),
                    selected: !phone && selected == Some(&task.id),
                    action: if phone {
                        Press::Open(Live::Task(task.id.clone()))
                    } else {
                        Press::Select(task.id.clone())
                    },
                })
            })
            .collect();
        let hidden = members.len().saturating_sub(limit);
        if hidden != 0 {
            let noun = match key {
                GroupKey::Parked => "parked",
                GroupKey::Held => "held",
                _ => "upcoming",
            };
            rows.push(Row::More(Button::new(
                format!("+ {hidden} more {noun}"),
                Action::ShowAll(key),
                ButtonKind::Quiet,
            )));
        }
        view.groups.push(GroupView {
            key,
            label: group_label(key),
            count: if key == GroupKey::You && state.goal.is_none() {
                u32::try_from(crate::model::inbox::attention_counts(feed).now).unwrap_or(u32::MAX)
            } else {
                u32::try_from(members.len()).unwrap_or(u32::MAX)
            },
            hue: key.hue(),
            detail: (!phone && key == GroupKey::Corral).then_some("gating · reviewing · landing"),
            rows,
        });
    }
    view
}

/// The session a task's Enter opens: its current run's, from the detail or the board.
pub fn task_session(feed: &daemon::Feed, task: &daemon::Task) -> Option<Id> {
    super::loaded_task(feed, &task.id)
        .and_then(|detail| detail.run.as_ref()?.session.clone())
        .or_else(|| task.agent.as_ref()?.session.clone())
        .map(Id::from)
}

/// The open question asked on a task, by its key, when the inbox serves it.
pub fn task_question(feed: &daemon::Feed, task: &str) -> Option<Id> {
    use corral_protocol::rows::InboxState;
    feed.inbox
        .values()
        .find(|row| {
            row.task.as_deref() == Some(task)
                && row.state == InboxState::Open
                && crate::model::inbox::is_question(row)
        })
        .map(|row| Id::from(row.id.as_str()))
}

/// Reuse Inbox's canonical routing for every kind of open task attention.
pub(super) fn task_attention(feed: &daemon::Feed, task: &str) -> Option<Live> {
    crate::model::inbox::ordered(feed)
        .into_iter()
        .filter(|row| row.task.as_deref() == Some(task) && crate::model::inbox::is_open(row))
        .find_map(crate::model::inbox::target)
}

pub(super) fn attention_button(feed: &daemon::Feed, task: &str) -> Option<Button> {
    let target = task_attention(feed, task)?;
    let label = match &target {
        Live::Question(id) => format!("Answer {id}"),
        Live::Review(_) => "Review candidate".into(),
        Live::Plan(_) => "Review plan".into(),
        _ => "Open task".into(),
    };
    Some(Button::new(label, Press::Open(target), ButtonKind::Primary))
}

impl State {
    /// Now's keys in the Daemon state: j/k and the arrows walk the visible rows in display
    /// order (a collapsed group's hidden rows are skipped), ] cycles the inspector, Enter
    /// opens canonical attention or shows the session in the same inspector.
    /// `top` keeps keys away from another domain's pushed page.
    pub fn key_live(
        &mut self,
        key: &str,
        top: Option<&Live>,
        selected: &Selection,
        feed: &daemon::Feed,
        env: Env,
    ) -> Option<Vec<Effect>> {
        if top.is_some_and(|live| !matches!(live, Live::Task(_))) {
            return None;
        }
        let tasks = shown(self, feed.board.tasks());
        self.key_tasks(key, selected.task.as_ref(), &tasks, feed, env.form)
    }

    fn key_tasks(
        &mut self,
        key: &str,
        chosen: Option<&Id>,
        tasks: &[&daemon::Task],
        feed: &daemon::Feed,
        form: Form,
    ) -> Option<Vec<Effect>> {
        match key {
            "j" | "down" | "ArrowDown" | "k" | "up" | "ArrowUp" => {
                let ids = visible(self, tasks, feed, form);
                let at = selection(self, chosen, tasks, feed, form)
                    .and_then(|current| ids.iter().position(|id| *id == current));
                let next = match at {
                    Some(index) if matches!(key, "j" | "down" | "ArrowDown") => {
                        ids.get(index + 1).or(ids.get(index))
                    }
                    Some(index) => ids.get(index.saturating_sub(1)),
                    None => ids.first(),
                };
                Some(
                    next.map(|id| Effect::Select(Live::Task((*id).clone())))
                        .into_iter()
                        .collect(),
                )
            }
            "." => {
                let id = selection(self, chosen, tasks, feed, form)?;
                Some(vec![Effect::VerbMenu(super::Record::Task(id.clone()))])
            }
            "enter" | "Enter" => {
                let task = selection(self, chosen, tasks, feed, form)
                    .and_then(|id| tasks.iter().find(|task| task.id == *id))?;
                if let Some(attention) = task_attention(feed, &task.id) {
                    Some(vec![Effect::OpenLive(attention)])
                } else {
                    self.now_tab = super::NowTab::Session;
                    Some(Vec::new())
                }
            }
            "]" => {
                self.cycle_tab();
                Some(Vec::new())
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::board::Mark;
    use crate::model::effect::Conn;

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
            let list = daemon_list(&state, &feed, &Selection::default(), Form::Desktop);
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
        let list = list(&State::default(), &refs, None, &feed, Form::Desktop);
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
            let effects = state.key_tasks("j", current.as_ref(), &refs, &feed, Form::Desktop);
            selected_after(effects, &mut current);
        }
        assert_eq!(current, Some(tasks[COLLAPSED - 1].id.clone()));
        state.act(Action::ShowAll(GroupKey::Queued));
        for _ in 0..tasks.len() {
            let effects = state.key_tasks("j", current.as_ref(), &refs, &feed, Form::Desktop);
            selected_after(effects, &mut current);
        }
        assert_eq!(current, Some(tasks[tasks.len() - 1].id.clone()));
    }

    #[test]
    fn a_selection_that_left_the_board_falls_back_to_the_first_visible_row() {
        let tasks = [
            task("T-5", daemon::Group::Queued),
            task("T-6", daemon::Group::Verifying),
        ];
        let refs: Vec<&daemon::Task> = tasks.iter().collect();
        let gone = Id::from("T-404");
        let state = State::default();
        let list = list(
            &state,
            &refs,
            Some(&gone),
            &daemon::Feed::default(),
            Form::Desktop,
        );
        let selected: Vec<String> = list
            .groups
            .iter()
            .flat_map(|group| group.rows.iter())
            .filter_map(|row| match row {
                Row::Task(task) if task.selected => Some(task.line.id.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(selected, ["T-6"]);
        let mut state = state;
        let mut current = Some(gone);
        let effects = state.key_tasks(
            "j",
            current.clone().as_ref(),
            &refs,
            &daemon::Feed::default(),
            Form::Desktop,
        );
        selected_after(effects, &mut current);
        assert_eq!(current.as_deref(), Some("T-5"));
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
        let list = daemon_list(
            &State::default(),
            &feed,
            &Selection::default(),
            Form::Desktop,
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
                            InboxKind::Review => {
                                corral_protocol::rows::AttentionDestination::Review
                            }
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
                .key_tasks("Enter", None, &tasks, &feed, Form::Desktop)
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
        let list = daemon_list(&state, &feed, &selected, Form::Desktop);
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
            super::super::daemon_live(&state, &feed, &selected, Form::Desktop)
                .selected_note
                .is_some()
        );
        assert!(
            super::super::daemon_plan(&state, &feed, &selected, Form::Desktop)
                .empty
                .is_some()
        );

        feed.board_received(1, vec![task("T-1", daemon::Group::Queued)], &[], true);
        assert!(
            !daemon_list(&state, &feed, &selected, Form::Desktop)
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
            state.key_tasks("]", None, &[&row], &feed, Form::Desktop),
            Some(vec![])
        );
        assert_eq!(state.now_tab, super::super::NowTab::Evidence);
        assert!(
            state
                .key_tasks("Tab", None, &[&row], &feed, Form::Desktop)
                .is_none()
        );
        let effects = state.key_tasks("Enter", None, &[&row], &feed, Form::Desktop);
        assert_eq!(effects, Some(vec![]));
        assert_eq!(state.now_tab, super::super::NowTab::Session);
    }
}
