//! The Task tab of a task-bound session (worker, reviewer, planner): the run's task as the
//! engine records it (`DetailRef::Task`), read as Now reads it. This module holds the tab's
//! summary from the board's row and its reading and failed states, and says the tab does not
//! apply to a session with no task run; the task's story, its card and its next steps are this
//! module's to add, with their state in `TaskUi` and their actions in `TaskAction`.

use corral_protocol::detail::DetailRef;

use crate::model::daemon::Feed;
use crate::model::effect::Effect;
use crate::model::hue::Hue;

use super::actions::ActionGroup;
use super::keys::KeyBinding;
use super::tabs::{PartBody, load, part};
use super::{Action, Cx, State};

/// The Task tab's state on this device, per session.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TaskUi {}

impl TaskUi {
    pub const EMPTY: Self = Self {};
}

/// What the Task tab does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskAction {}

#[derive(Debug, Clone, PartialEq)]
pub struct TaskTab {
    pub part: PartBody,
}

/// The run's task record; `Err` when the session has no task run.
pub fn target(cx: &Cx<'_>) -> Result<DetailRef, String> {
    match cx.task.filter(|_| cx.role.task_bound()) {
        Some(task) => Ok(DetailRef::Task {
            id: task.to_string(),
        }),
        None => Err("this session has no task run".into()),
    }
}

/// `T-12 · Fix the gate cache · Running`, as the board's row says it.
pub fn summary(cx: &Cx<'_>) -> String {
    let Some(task) = cx.task else {
        return "no task".into();
    };
    match cx.feed.board.task(task) {
        Some(row) => format!("{task} · {} · {}", row.title(), row.now()),
        None => task.to_string(),
    }
}

pub fn task_tab(cx: &Cx<'_>) -> TaskTab {
    TaskTab {
        part: part(cx, target(cx), None, summary(cx), Hue::Neutral),
    }
}

/// What the tab reads while it shows.
pub fn fetches(cx: &Cx<'_>) -> Vec<Action> {
    load(cx, target(cx), None).into_iter().collect()
}

pub(super) fn act(_: &mut State, action: TaskAction, _: &Feed) -> Vec<Effect> {
    match action {}
}

/// The tab's rows in the session's action catalog.
pub fn actions(_: &Cx<'_>) -> Option<ActionGroup> {
    None
}

/// The tab's own keys.
pub fn keys() -> Vec<KeyBinding> {
    Vec::new()
}

pub fn press(_: &str, _: &Cx<'_>) -> Option<Action> {
    None
}
