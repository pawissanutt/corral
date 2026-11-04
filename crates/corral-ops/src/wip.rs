//! Workspace-wide coding work in progress, independent of process and slot liveness.
use anyhow::Result;
use corral_core::{Status, Store, Task, TaskHead};

fn in_flight(status: Status, has_run: bool) -> bool {
    has_run && matches!(status, Status::Running | Status::NeedsMe | Status::Review)
}

/// Count tasks with a current coding run that has not landed or been dropped.
pub fn count(heads: &[TaskHead]) -> usize {
    heads
        .iter()
        .filter(|h| in_flight(h.status, h.current_run.is_some()))
        .count()
}

/// The claim lock serializes this check with creation of coding launch reservations.
pub(crate) fn reason(store: &Store, task: &Task) -> Result<Option<String>> {
    let limit = store.config.limits.wip;
    if limit == 0
        || !matches!(task.status, Status::Ready | Status::Backlog | Status::Idea)
        || task.current_run().is_some_and(|run| {
            run.outcome == Some(corral_core::RunOutcome::Superseded) && run.retry.is_some()
        })
    {
        return Ok(None);
    }
    let heads = crate::pm_next::heads::open(store)?;
    let active = count(&heads);
    let reservations = heads
        .iter()
        .filter(|h| {
            h.id != task.id
                && !in_flight(h.status, h.current_run.is_some())
                && h.current_run.as_ref().is_some_and(|run| {
                    h.launch.as_ref().is_some_and(|l| l.reserved)
                        || (h.status == Status::Ready
                            && run.outcome == Some(corral_core::RunOutcome::Superseded)
                            && run.retry.is_some())
                })
        })
        .count();
    let used = active + reservations;
    let cap = limit.saturating_add(usize::from(corral_core::red_main(task)));
    Ok((used >= cap).then(|| format!("waiting: work in progress {used}/{limit}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lifecycle_classification_does_not_depend_on_slot_state() {
        for status in [Status::Running, Status::NeedsMe, Status::Review] {
            assert!(in_flight(status, true));
            assert!(!in_flight(status, false));
        }
        for status in [
            Status::Idea,
            Status::Backlog,
            Status::Ready,
            Status::Done,
            Status::Dropped,
        ] {
            assert!(!in_flight(status, true));
        }
    }
}
