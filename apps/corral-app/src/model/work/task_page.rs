//! A task opened by id that the open-task topic does not list, a closed task above all: shown
//! from the task detail corrald serves, never through another task's row. Its story reads the
//! detail's task with its latest run and checks the way a served row carries them, so the
//! stepper and facts read the same either way. Also the phone's task page, which shows a board
//! task's story or, off the board, this one.

use corral_protocol::detail::TaskDetail;
use corral_protocol::rows::TaskRow;
use corral_types::{Check, CheckKind};

use crate::model::daemon::{self, Feed, Link};
use crate::model::nav::Form;
use crate::model::{Id, Selection};

use super::{
    Button, Inspector, LivePlan, NO_BOARD, NowTab, State, Tail, detail_note, loaded_task,
    plan_context, task_inspector, task_ref,
};

/// `id`, while the complete board does not list it.
pub fn off_board_id<'a>(feed: &Feed, id: &'a Id) -> Option<&'a Id> {
    (feed.board.complete && feed.board.task(id).is_none()).then_some(id)
}

/// The task the root selects, while the complete board does not list it.
pub fn off_board<'a>(selected: &'a Selection, feed: &Feed) -> Option<&'a Id> {
    selected.task.as_ref().and_then(|id| off_board_id(feed, id))
}

/// The newest check of `kind` in the detail.
fn latest(detail: &TaskDetail, kind: CheckKind) -> Option<Check> {
    detail
        .checks
        .iter()
        .filter(|check| check.kind == kind)
        .max_by_key(|check| check.at)
        .cloned()
}

/// The detail's task as a served row would carry it: its latest run, gate and review checks.
/// No `ready` event is known off the board, so it is never claimed.
fn detail_task(detail: &TaskDetail) -> daemon::Task {
    daemon::Task::new(TaskRow {
        task: detail.task.clone(),
        run: detail.runs.iter().max_by_key(|run| run.started).cloned(),
        gate: latest(detail, CheckKind::Gate),
        review: latest(detail, CheckKind::Review),
        ready: false,
    })
}

/// The inspector's `tab` for a task off the board: placeholders while its detail loads, the
/// daemon's reason when it cannot be served, else the detail's story, artifacts or run.
pub fn off_board_inspector(
    state: &State,
    feed: &Feed,
    id: &Id,
    tab: NowTab,
    form: Form,
) -> Inspector {
    let Some(detail) = loaded_task(feed, id) else {
        let failed = feed.link == Link::Down
            || feed
                .detail(&task_ref(id))
                .is_some_and(|cache| cache.error.is_some());
        return match detail_note(feed, id) {
            Some(note) if failed => Inspector::Unavailable(note),
            _ => Inspector::Loading,
        };
    };
    task_inspector(state, feed, &detail_task(detail), tab, form)
}

/// What the phone's page shows under "Live": a run's terminal capture, a note that none is
/// loaded, or nothing at all for a task that never ran.
#[derive(Debug, Clone, PartialEq)]
pub enum TaskLog {
    /// No run, so no terminal to speak of.
    Absent,
    /// A run without a loaded capture.
    Empty,
    Lines(Tail),
}

impl TaskLog {
    fn of(tail: Option<Tail>) -> Self {
        match tail.filter(|tail| !tail.lines.is_empty()) {
            Some(tail) => Self::Lines(tail),
            None => Self::Empty,
        }
    }
}

/// The phone's page for a task opened by id.
#[derive(Debug, Clone, PartialEq)]
pub enum TaskPage {
    /// What shows instead of the task: why there is no board yet, its detail loading, or why
    /// corrald cannot serve it.
    Waiting(Inspector),
    /// The task's story, its Session/Review buttons and its terminal capture.
    Ready {
        story: Box<Inspector>,
        buttons: Vec<Button>,
        log: TaskLog,
    },
}

/// The phone's task page: the board row's story while the board lists the task, its detail's
/// once the complete board does not.
pub fn task_page(state: &State, feed: &Feed, id: &Id) -> TaskPage {
    let page = |task: &daemon::Task| TaskPage::Ready {
        story: Box::new(task_inspector(
            state,
            feed,
            task,
            NowTab::Story,
            Form::Phone,
        )),
        buttons: super::task_buttons(feed, task, true),
        // A task that never ran has no run, so no terminal either way.
        log: if task.row.run.is_some() || task.rail[0].at.is_some() {
            TaskLog::of(super::task_tail(feed, task))
        } else {
            TaskLog::Absent
        },
    };
    if let Some(task) = feed.board.task(id) {
        return page(task);
    }
    if !feed.board.complete {
        return TaskPage::Waiting(Inspector::Quiet(
            feed.device.board_notice().unwrap_or(NO_BOARD),
        ));
    }
    match loaded_task(feed, id) {
        Some(detail) => page(&detail_task(detail)),
        None => TaskPage::Waiting(off_board_inspector(
            state,
            feed,
            id,
            NowTab::Story,
            Form::Phone,
        )),
    }
}

/// The plan graph under the phone's task page: the parent task, from the board row or, off the
/// board, the detail once it has arrived.
pub fn page_plan(feed: &Feed, id: &Id) -> Option<LivePlan> {
    match feed.board.task(id) {
        Some(task) => Some(plan_context(id, task.parent())),
        None => off_board_id(feed, id)
            .and_then(|id| loaded_task(feed, id))
            .map(|detail| plan_context(id, detail.task.parent.as_deref())),
    }
}
