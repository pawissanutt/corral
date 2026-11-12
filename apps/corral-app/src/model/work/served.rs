//! The who-acts-next list and its keyboard selection in the Daemon data state, built from the
//! daemon's board: its groups in the fixture's order and anatomy, with the real rows and counts.

use crate::model::board::TaskLine;
use crate::model::daemon::{self, Link};
use crate::model::effect::{Effect, Page};
use crate::model::nav::Form;

use super::{
    Action, Button, ButtonKind, GROUPS, GroupKey, GroupView, GroupedList, Row, State, TaskRow,
};

/// Said instead of the list while the link is down and no board has arrived.
pub const NO_BOARD: &str = "No board received from corrald yet.";
/// Said instead of the list when the daemon's board holds no task.
pub const EMPTY_BOARD: &str = "Nothing on the board.";

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

/// The fixture's heading for a group.
pub fn group_label(key: GroupKey) -> &'static str {
    GROUPS
        .iter()
        .find(|group| group.key == key)
        .map_or_else(|| key.key(), |group| group.label)
}

/// The non-empty groups of `tasks`, in display order, each keeping the board's order.
fn grouped(tasks: &[daemon::Task]) -> Vec<(GroupKey, Vec<&daemon::Task>)> {
    ORDER
        .iter()
        .map(|(key, members)| {
            let rows: Vec<&daemon::Task> = tasks
                .iter()
                .filter(|task| members.contains(&task.group))
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
fn visible(state: &State, tasks: &[daemon::Task], form: Form) -> Vec<&'static str> {
    grouped(tasks)
        .into_iter()
        .flat_map(|(key, rows)| {
            let limit = limit(state, key, form);
            rows.into_iter().take(limit).map(|task| task.id)
        })
        .collect()
}

/// The selected task: the state's, while it is on the board, otherwise the first visible row.
fn selection(state: &State, tasks: &[daemon::Task], form: Form) -> Option<&'static str> {
    if tasks.iter().any(|task| task.id == state.sel) {
        Some(state.sel)
    } else {
        visible(state, tasks, form).first().copied()
    }
}

/// The task the daemon's Now treats as selected on `board`, if the board holds any.
pub fn daemon_selection(state: &State, board: &daemon::Board, form: Form) -> Option<&'static str> {
    selection(state, board.tasks(), form)
}

/// Who acts next in the Daemon state: loading placeholders while the first board is on its way,
/// one quiet line while there is none or it is empty, else the groups.
pub fn daemon_list(state: &State, feed: &daemon::Feed, form: Form) -> GroupedList {
    let board = &feed.board;
    if !board.complete {
        return match feed.link {
            Link::Down => GroupedList {
                notice: Some(NO_BOARD),
                ..GroupedList::new(form)
            },
            Link::Connecting | Link::Up => GroupedList::loading(form),
        };
    }
    list(state, board.tasks(), feed.now, form)
}

fn list(state: &State, tasks: &[daemon::Task], now: u64, form: Form) -> GroupedList {
    let mut view = GroupedList::new(form);
    if tasks.is_empty() {
        view.notice = Some(EMPTY_BOARD);
        return view;
    }
    let phone = form == Form::Phone;
    let selected = selection(state, tasks, form);
    for (key, members) in grouped(tasks) {
        let limit = limit(state, key, form);
        let mut rows: Vec<Row> = members
            .iter()
            .take(limit)
            .map(|task| {
                Row::Task(TaskRow {
                    line: TaskLine::daemon(task, now),
                    selected: !phone && selected == Some(task.id),
                    action: if phone {
                        Action::OpenTask(task.id)
                    } else {
                        Action::Select(task.id)
                    },
                })
            })
            .collect();
        let hidden = members.len().saturating_sub(limit);
        if hidden != 0 {
            let noun = match key {
                GroupKey::Parked => "parked",
                GroupKey::Held => "held",
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
            label: group_label(key),
            count: u32::try_from(members.len()).unwrap_or(u32::MAX),
            hue: key.hue(),
            detail: (!phone && key == GroupKey::Corral).then_some("gating · reviewing · landing"),
            rows,
        });
    }
    view
}

impl State {
    /// Now's keys in the Daemon state: j/k and the arrows walk the visible rows in display
    /// order (a collapsed group's hidden rows are skipped), Tab cycles the inspector, Enter says
    /// sessions are not served. `page` keeps keys away from another domain's pushed page.
    pub fn key_daemon(
        &mut self,
        key: &str,
        page: Option<Page>,
        board: &daemon::Board,
        form: Form,
    ) -> Option<Vec<Effect>> {
        self.key_tasks(key, page, board.tasks(), form)
    }

    fn key_tasks(
        &mut self,
        key: &str,
        page: Option<Page>,
        tasks: &[daemon::Task],
        form: Form,
    ) -> Option<Vec<Effect>> {
        if page.is_some_and(|page| !matches!(page, Page::Task(_))) {
            return None;
        }
        match key {
            "j" | "down" | "ArrowDown" | "k" | "up" | "ArrowUp" => {
                let ids = visible(self, tasks, form);
                let at = selection(self, tasks, form)
                    .and_then(|current| ids.iter().position(|id| *id == current));
                let next = match at {
                    Some(index) if matches!(key, "j" | "down" | "ArrowDown") => {
                        ids.get(index + 1).or(ids.get(index))
                    }
                    Some(index) => ids.get(index.saturating_sub(1)),
                    None => ids.first(),
                };
                if let Some(id) = next {
                    self.select(*id);
                }
                Some(Vec::new())
            }
            "enter" | "Enter" => Some(vec![Effect::Toast(
                "Sessions are not served by corrald yet".into(),
            )]),
            "tab" | "Tab" => {
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
    use crate::model::effect::{Conn, DataMode, Env};

    fn task(id: &'static str, group: daemon::Group) -> daemon::Task {
        daemon::Task {
            id,
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
            held_reason: None,
            questions: 0,
            attention: Vec::new(),
            agent: None,
            cost: None,
            cost_estimated: false,
            rev: 1,
        }
    }

    fn ids(list: &GroupedList, key: GroupKey) -> Vec<&'static str> {
        list.groups
            .iter()
            .filter(|group| group.key == key)
            .flat_map(|group| group.rows.iter())
            .filter_map(|row| match row {
                Row::Task(task) => Some(task.line.id),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn daemon_list_groups_rows_by_who_acts_next_and_puts_other_under_queued() {
        let tasks = [
            task("T-1", daemon::Group::Other),
            task("T-2", daemon::Group::Producing),
            task("T-3", daemon::Group::NeedsYou),
            task("T-4", daemon::Group::Queued),
        ];
        let list = list(&State::default(), &tasks, 0, Form::Desktop);
        let keys: Vec<GroupKey> = list.groups.iter().map(|group| group.key).collect();
        assert_eq!(keys, [GroupKey::You, GroupKey::Worker, GroupKey::Queued]);
        assert_eq!(ids(&list, GroupKey::Queued), ["T-1", "T-4"]);
        assert_eq!(ids(&list, GroupKey::You), ["T-3"]);
    }

    #[test]
    fn collapsed_queued_rows_stay_out_of_keyboard_reach_until_expanded() {
        let names: Vec<&'static str> = (1..=COLLAPSED + 2)
            .map(|n| crate::model::daemon::id(&format!("T-{n}")))
            .collect();
        let tasks: Vec<daemon::Task> = names
            .iter()
            .map(|id| task(*id, daemon::Group::Queued))
            .collect();
        let mut state = State {
            sel: names[0],
            ..State::default()
        };
        for _ in 0..names.len() {
            state.key_tasks("j", None, &tasks, Form::Desktop);
        }
        assert_eq!(state.sel, names[COLLAPSED - 1]);
        let env = Env {
            conn: Conn::Healthy,
            mode: DataMode::Live,
            form: Form::Desktop,
        };
        state.act(Action::ShowAll(GroupKey::Queued), env);
        for _ in 0..names.len() {
            state.key_tasks("j", None, &tasks, Form::Desktop);
        }
        assert_eq!(state.sel, names[names.len() - 1]);
    }

    #[test]
    fn a_selection_that_left_the_board_falls_back_to_the_first_visible_row() {
        let tasks = [
            task("T-5", daemon::Group::Queued),
            task("T-6", daemon::Group::Verifying),
        ];
        let state = State {
            sel: "T-404",
            ..State::default()
        };
        let list = list(&state, &tasks, 0, Form::Desktop);
        let selected: Vec<&'static str> = list
            .groups
            .iter()
            .flat_map(|group| group.rows.iter())
            .filter_map(|row| match row {
                Row::Task(task) if task.selected => Some(task.line.id),
                _ => None,
            })
            .collect();
        assert_eq!(selected, ["T-6"]);
        let mut state = state;
        state.key_tasks("j", None, &tasks, Form::Desktop);
        assert_eq!(state.sel, "T-5");
    }
}
