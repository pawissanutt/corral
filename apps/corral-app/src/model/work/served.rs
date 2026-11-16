//! The who-acts-next list and its keyboard selection in the Daemon data state, built from the
//! daemon's board: its groups in the fixture's order and anatomy, with the real rows and counts.
//! The selected task is the root's live selection, by its owned id.

use std::collections::BTreeSet;

use crate::model::board::TaskLine;
use crate::model::daemon::{self, Link};
use crate::model::effect::{Effect, Env};
use crate::model::nav::Form;
use crate::model::projects::{NowGroup, NowScope, ProjectFilter, project_chip};
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
pub const NO_PROJECT_TASKS: &str = "No task on the board matches this project and goal.";

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
fn shown<'a>(
    state: &State,
    feed: &'a daemon::Feed,
    filter: &ProjectFilter,
) -> Vec<&'a daemon::Task> {
    let filter = filter.resolved(feed);
    feed.board
        .tasks()
        .iter()
        .filter(|task| filter.matches(task.project.as_deref()))
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
    grouping: NowGroup,
) -> Vec<(GroupKey, String, Vec<&'a daemon::Task>)> {
    // Project sections show all their tasks: state-specific expansion controls cannot
    // describe a mixed-state project's truncation, and keys must walk exactly these rows.
    if grouping == NowGroup::Project {
        let mut ordered = tasks.to_vec();
        ordered.sort_by_cached_key(|task| {
            let task: &'a daemon::Task = task;
            let rank = ORDER
                .iter()
                .position(|(key, _)| *key == task_group(feed, task));
            (task.project.is_none(), task.project.as_deref(), rank)
        });
        let mut groups: Vec<(GroupKey, String, Vec<&daemon::Task>)> = Vec::new();
        for task in ordered {
            let same = groups
                .last()
                .and_then(|(_, _, rows)| rows.first())
                .is_some_and(|last| last.project == task.project);
            if same {
                if let Some((_, _, rows)) = groups.last_mut() {
                    rows.push(task);
                }
            } else {
                groups.push((
                    GroupKey::Queued,
                    task.project.clone().unwrap_or_else(|| "Unassigned".into()),
                    vec![task],
                ));
            }
        }
        return groups;
    }
    ORDER
        .iter()
        .map(|(key, _)| {
            let rows: Vec<&daemon::Task> = tasks
                .iter()
                .copied()
                .filter(|task| task_group(feed, task) == *key)
                .collect();
            (*key, group_label(*key).to_owned(), rows)
        })
        .filter(|(_, _, rows)| !rows.is_empty())
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
    grouping: NowGroup,
) -> Vec<&'a Id> {
    grouped(tasks, feed, grouping)
        .into_iter()
        .flat_map(|(key, _, rows)| {
            let limit = if grouping == NowGroup::Project {
                usize::MAX
            } else {
                limit(state, key, form)
            };
            rows.into_iter().take(limit).map(|task| &task.id)
        })
        .collect()
}

/// The selected task: the root's, while the list shows it; none while the board does not list
/// it at all (Now shows its detail, and no other row stands in for it); otherwise, when nothing
/// is chosen or the goal filter hides the choice, the first visible row.
fn selection<'a>(
    state: &State,
    chosen: Option<&Id>,
    tasks: &[&'a daemon::Task],
    feed: &daemon::Feed,
    form: Form,
    grouping: NowGroup,
) -> Option<&'a Id> {
    if let Some(id) = chosen {
        if let Some(task) = tasks.iter().copied().find(|task| task.id == *id) {
            return Some(&task.id);
        }
        feed.board.task(id)?;
    }
    visible(state, tasks, feed, form, grouping).first().copied()
}

/// The task the daemon's Now treats as selected on the board, if the list shows any.
/// `filter` and `grouping` are the Now screen's own scope.
pub fn daemon_selection_filtered<'a>(
    state: &State,
    selected: &Selection,
    feed: &'a daemon::Feed,
    form: Form,
    filter: &ProjectFilter,
    grouping: NowGroup,
) -> Option<&'a daemon::Task> {
    let tasks = shown(state, feed, filter);
    let id = selection(state, selected.task.as_ref(), &tasks, feed, form, grouping)?;
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
/// `filter` scopes the rows to one project, Unassigned or All; `grouping` heads them by state or
/// by project.
pub fn daemon_list_filtered(
    state: &State,
    feed: &daemon::Feed,
    selected: &Selection,
    form: Form,
    filter: &ProjectFilter,
    grouping: NowGroup,
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
    let tasks = shown(state, feed, filter);
    if tasks.is_empty() {
        return GroupedList {
            notice: Some(if filter.resolved(feed) == ProjectFilter::All {
                NO_GOAL_TASKS
            } else {
                NO_PROJECT_TASKS
            }),
            ..GroupedList::new(form)
        };
    }
    list(
        state,
        &tasks,
        selected.task.as_ref(),
        feed,
        form,
        filter,
        grouping,
    )
}

fn list(
    state: &State,
    tasks: &[&daemon::Task],
    chosen: Option<&Id>,
    feed: &daemon::Feed,
    form: Form,
    filter: &ProjectFilter,
    grouping: NowGroup,
) -> GroupedList {
    let mut view = GroupedList::new(form);
    let phone = form == Form::Phone;
    let selected = selection(state, chosen, tasks, feed, form, grouping);
    let unseen = unseen_tasks(feed);
    for (key, label, members) in grouped(tasks, feed, grouping) {
        let limit = if grouping == NowGroup::Project {
            usize::MAX
        } else {
            limit(state, key, form)
        };
        let mut rows: Vec<Row> = members
            .iter()
            .take(limit)
            .map(|task| {
                Row::Task(TaskRow {
                    line: TaskLine::daemon(task, feed.now, unseen.contains(task.id.as_str())),
                    project_chip: project_chip(feed, filter, task.project.as_deref()),
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
            label,
            count: if grouping == NowGroup::State
                && key == GroupKey::You
                && state.goal.is_none()
                && filter.resolved(feed) == ProjectFilter::All
            {
                u32::try_from(
                    crate::model::inbox::attention_counts_filtered(feed, &ProjectFilter::All).now,
                )
                .unwrap_or(u32::MAX)
            } else {
                u32::try_from(members.len()).unwrap_or(u32::MAX)
            },
            hue: if grouping == NowGroup::Project {
                crate::model::hue::Hue::Neutral
            } else {
                key.hue()
            },
            detail: (!phone && grouping == NowGroup::State && key == GroupKey::Corral)
                .then_some("gating · reviewing · landing"),
            rows,
        });
    }
    view
}

/// The session a task's Enter opens: its current run's, from the detail or the board.
pub fn task_session(feed: &daemon::Feed, task: &daemon::Task) -> Option<Id> {
    session_for(feed, &task.id, task.agent.as_ref())
}

/// A task's current run's session by id: the detail's, else its board row's agent's.
pub(super) fn session_for(
    feed: &daemon::Feed,
    id: &str,
    agent: Option<&daemon::Agent>,
) -> Option<Id> {
    super::loaded_task(feed, id)
        .and_then(|detail| detail.run.as_ref()?.session.clone())
        .or_else(|| agent?.session.clone())
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
    crate::model::inbox::ordered_filtered(feed, &ProjectFilter::All)
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
    /// order (a collapsed group's hidden rows are skipped), then the open Closed section's
    /// rows, ] cycles the inspector, Enter opens canonical attention or shows the session in
    /// the same inspector, and opens a closed task's own page.
    /// `top` keeps keys away from another domain's pushed page.
    /// `scope` is the shown list's scope, so the keys walk the rows on screen.
    pub fn key_live_filtered(
        &mut self,
        key: &str,
        top: Option<&Live>,
        selected: &Selection,
        feed: &daemon::Feed,
        env: Env,
        scope: NowScope<'_>,
    ) -> Option<Vec<Effect>> {
        if top.is_some_and(|live| !matches!(live, Live::Task(_))) {
            return None;
        }
        let tasks = shown(self, feed, scope.filter);
        self.key_tasks(key, selected.task.as_ref(), &tasks, feed, env.form, scope)
    }

    fn key_tasks(
        &mut self,
        key: &str,
        chosen: Option<&Id>,
        tasks: &[&daemon::Task],
        feed: &daemon::Feed,
        form: Form,
        scope: NowScope<'_>,
    ) -> Option<Vec<Effect>> {
        let NowScope { filter, grouping } = scope;
        match key {
            "j" | "down" | "ArrowDown" | "k" | "up" | "ArrowUp" => {
                let closed = super::closed_ids_filtered(&self.closed, feed, filter);
                let mut ids = visible(self, tasks, feed, form, grouping);
                ids.extend(closed.iter());
                // A closed task is never on the board, so `selection` would not find it.
                let current = chosen
                    .filter(|id| ids.contains(id))
                    .or_else(|| selection(self, chosen, tasks, feed, form, grouping));
                let at = current.and_then(|current| ids.iter().position(|id| *id == current));
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
                let id = selection(self, chosen, tasks, feed, form, grouping)?;
                Some(vec![Effect::VerbMenu(super::Record::Task(id.clone()))])
            }
            "enter" | "Enter" => {
                let closed = super::closed_ids_filtered(&self.closed, feed, filter);
                if let Some(id) = chosen.filter(|id| closed.contains(id)) {
                    return Some(vec![Effect::OpenLive(Live::Task(id.clone()))]);
                }
                let task = selection(self, chosen, tasks, feed, form, grouping)
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
#[path = "served_tests.rs"]
mod tests;
