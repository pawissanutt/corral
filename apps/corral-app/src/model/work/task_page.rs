//! A task the open-task topic does not list, a closed task above all, read from the task detail
//! corrald serves: its task with its latest run and checks the way a served row carries them,
//! so a page reads it the same either way.

use corral_protocol::detail::TaskDetail;
use corral_protocol::rows::TaskRow;
use corral_types::{Check, CheckKind, Status};

use crate::model::daemon;

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
pub fn detail_task(detail: &TaskDetail) -> daemon::Task {
    daemon::Task::new(TaskRow {
        task: detail.task.clone(),
        run: detail.runs.iter().max_by_key(|run| run.started).cloned(),
        // Detail is history, with no observation of the current worktree HEAD.
        gate: if detail.task.status == Status::Running {
            None
        } else {
            latest(detail, CheckKind::Gate)
        },
        review: latest(detail, CheckKind::Review),
        ready: false,
        ..TaskRow::default()
    })
}
