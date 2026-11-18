//! The who-moves-next list and its keyboard selection in the Daemon data state, built from the
//! open tasks corrald serves: the groups the app derives from each row, in the plan's order
//! (needs you, corral, workers, queued, backlog, ideas). The selected task is the root's live
//! selection, by its owned id.

use std::collections::BTreeSet;

use corral_types::Status;

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
pub const NO_BOARD: &str = "No tasks received from corrald yet.";
/// Said instead of the list when corrald serves no open task.
pub const EMPTY_BOARD: &str = "No open tasks.";
/// The tasks topic failed before producing any rows; the status banner carries the reason and
/// retry.
pub(super) const BOARD_UNAVAILABLE: &str = "Tasks unavailable.";
/// Said instead of the list when no open task belongs to the chosen project scope.
pub const NO_PROJECT_TASKS: &str = "No open task matches this project.";

/// Rows a collapsed Queued, Backlog or Ideas group shows on desktop and terminal, and on phone.
const COLLAPSED: usize = 12;
const COLLAPSED_PHONE: usize = 4;

/// The display order of the groups, and the derived groups each one holds; a closed task the
/// topic still carries waits with the queued ones.
const ORDER: [(GroupKey, &[daemon::Group]); 6] = [
    (GroupKey::You, &[daemon::Group::NeedsYou]),
    (GroupKey::Corral, &[daemon::Group::Verifying]),
    (GroupKey::Worker, &[daemon::Group::Producing]),
    (
        GroupKey::Queued,
        &[daemon::Group::Queued, daemon::Group::Other],
    ),
    (GroupKey::Backlog, &[daemon::Group::Backlog]),
    (GroupKey::Ideas, &[daemon::Group::Ideas]),
];

/// The list group a daemon task is shown under.
pub fn group_key(group: daemon::Group) -> GroupKey {
    ORDER
        .iter()
        .find(|(_, members)| members.contains(&group))
        .map_or(GroupKey::Queued, |(key, _)| *key)
}

/// The heading of a group: who moves next.
pub fn group_label(key: GroupKey) -> &'static str {
    match key {
        GroupKey::You => "Needs you",
        GroupKey::Corral => "Corral is verifying",
        GroupKey::Worker => "Worker runs in progress",
        GroupKey::Queued => "Queued",
        GroupKey::Backlog => "Backlog",
        GroupKey::Ideas => "Ideas",
    }
}

/// The open tasks in `filter`'s project scope.
fn shown<'a>(feed: &'a daemon::Feed, filter: &ProjectFilter) -> Vec<&'a daemon::Task> {
    let filter = filter.resolved(feed);
    feed.board
        .tasks()
        .iter()
        .filter(|task| filter.matches(task.project()))
        .collect()
}

/// The non-empty groups of `tasks`, in display order, each keeping the board's order.
fn grouped<'a>(
    tasks: &[&'a daemon::Task],
    grouping: NowGroup,
) -> Vec<(GroupKey, String, Vec<&'a daemon::Task>)> {
    // Project sections show all their tasks: state-specific expansion controls cannot
    // describe a mixed-state project's truncation, and keys must walk exactly these rows.
    if grouping == NowGroup::Project {
        let mut ordered = tasks.to_vec();
        ordered.sort_by_cached_key(|task| {
            let rank = ORDER
                .iter()
                .position(|(key, _)| *key == group_key(task.group));
            (
                task.project().is_none(),
                task.project().map(str::to_owned),
                rank,
            )
        });
        let mut groups: Vec<(GroupKey, String, Vec<&daemon::Task>)> = Vec::new();
        for task in ordered {
            let same = groups
                .last()
                .and_then(|(_, _, rows)| rows.first())
                .is_some_and(|last| last.project() == task.project());
            if same {
                if let Some((_, _, rows)) = groups.last_mut() {
                    rows.push(task);
                }
            } else {
                groups.push((
                    GroupKey::Queued,
                    task.project().unwrap_or("Unassigned").to_owned(),
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
                .filter(|task| group_key(task.group) == *key)
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
        GroupKey::Backlog => state.all_backlog,
        GroupKey::Ideas => state.all_ideas,
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
    form: Form,
    grouping: NowGroup,
) -> Vec<&'a Id> {
    grouped(tasks, grouping)
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
/// is chosen or the project filter hides the choice, the first visible row.
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
    visible(state, tasks, form, grouping).first().copied()
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
    let tasks = shown(feed, filter);
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
        .filter_map(|(_, event)| event.task.as_deref())
        .collect()
}

/// Who moves next in the Daemon state: loading placeholders while the first tasks page is on
/// its way, one quiet line while there is none or it is empty, else the groups.
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
        return GroupedList::quiet(line);
    }
    let board = &feed.board;
    if !board.complete {
        if super::served_facts::board_problem(feed).is_some() {
            return GroupedList::quiet(BOARD_UNAVAILABLE);
        }
        return match feed.link {
            Link::Down => GroupedList::quiet(NO_BOARD),
            Link::Connecting | Link::Up => GroupedList::loading(form),
        };
    }
    if board.tasks().is_empty() {
        return GroupedList::quiet(EMPTY_BOARD);
    }
    let tasks = shown(feed, filter);
    if tasks.is_empty() {
        return GroupedList::quiet(NO_PROJECT_TASKS);
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
    let mut view = GroupedList::default();
    let phone = form == Form::Phone;
    let selected = selection(state, chosen, tasks, feed, form, grouping);
    let unseen = unseen_tasks(feed);
    for (key, label, members) in grouped(tasks, grouping) {
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
                    project_chip: project_chip(feed, filter, task.project()),
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
                GroupKey::Backlog => "in backlog",
                GroupKey::Ideas => "ideas",
                _ => "queued",
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
            count: u32::try_from(members.len()).unwrap_or(u32::MAX),
            hue: if grouping == NowGroup::Project {
                crate::model::hue::Hue::Neutral
            } else {
                key.hue()
            },
            rows,
        });
    }
    view
}

/// The live session of a task's run, by its agents key (`<task>_<run>`): the board row's
/// current run's, else any live run of the task.
pub fn session_for(feed: &daemon::Feed, id: &str) -> Option<Id> {
    let current = feed
        .board
        .task(id)
        .and_then(|task| task.row.run.as_ref())
        .map(|run| Id::new(&format!("{id}_{}", run.id)))
        .filter(|key| feed.agents.contains_key(key));
    current.or_else(|| {
        feed.agents
            .iter()
            .find(|(_, row)| row.run.task == id)
            .map(|(key, _)| key.clone())
    })
}

impl State {
    /// Now's keys in the Daemon state: j/k and the arrows walk the visible rows in display
    /// order (a collapsed group's hidden rows are skipped), then the open Closed section's
    /// rows, ] cycles the inspector, Enter opens a review that waits on the owner or else shows
    /// the session in the same inspector, and opens a closed task's own page.
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
        let tasks = shown(feed, scope.filter);
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
                let mut ids = visible(self, tasks, form, grouping);
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
                if task.group == daemon::Group::NeedsYou && task.status() == Status::Review {
                    Some(vec![Effect::OpenLive(Live::Review(task.id.clone()))])
                } else {
                    self.now_tab = super::NowTab::Session;
                    Some(Vec::new())
                }
            }
            "]" => {
                self.cycle_tab();
                Some(Vec::new())
            }
            "R" => {
                let task = selection(self, chosen, tasks, feed, form, grouping)
                    .and_then(|id| tasks.iter().find(|task| task.id == *id))?;
                Some(if super::reply_box(task).is_some() {
                    self.now_tab = super::NowTab::Story;
                    vec![Effect::Focus(super::REPLY_FIELD)]
                } else {
                    vec![Effect::Toast(format!(
                        "{} has no question waiting for your reply",
                        task.id
                    ))]
                })
            }
            _ => None,
        }
    }
}

#[cfg(test)]
#[path = "served_tests.rs"]
mod tests;
