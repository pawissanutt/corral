//! Owner-acceptance checks of a task's current run, read from any task form.
use anyhow::Result;
use corral_core::{RunOutcome, Status, TaskRef};

/// [`Task::awaiting_owner`] of any task form: Review, or escalated by the autopilot.
pub(crate) fn awaiting_owner<T: TaskRef + ?Sized>(task: &T) -> Result<bool> {
    Ok(*task.status() == Status::Review
        || (*task.status() == Status::NeedsMe
            && task.current_run_head()?.is_some_and(|r| {
                r.outcome == Some(RunOutcome::Done)
                    && r.autopilot
                        .as_ref()
                        .is_some_and(|a| a.stage == corral_core::AutoStage::Escalated)
            })))
}
