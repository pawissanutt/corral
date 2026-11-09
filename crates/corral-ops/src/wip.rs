//! Workspace-wide coding work in progress, independent of process and slot liveness.
use anyhow::Result;
use corral_core::{Status, Store, Task, TaskHead};

fn in_flight(status: Status, has_run: bool) -> bool {
    has_run && matches!(status, Status::Running | Status::NeedsMe | Status::Review)
}

fn coding_active(
    status: Status,
    coding: Option<(jiff::Timestamp, Option<corral_core::RunOutcome>)>,
    plan: Option<jiff::Timestamp>,
) -> bool {
    coding.is_some_and(|(started, outcome)| {
        in_flight(status, true)
            && !matches!(
                outcome,
                Some(
                    corral_core::RunOutcome::Superseded
                        | corral_core::RunOutcome::Abandoned
                        | corral_core::RunOutcome::Discarded
                )
            )
            && plan.is_none_or(|plan_started| plan_started < started)
    })
}

pub(crate) fn task_in_flight(task: &Task) -> bool {
    coding_active(
        task.status,
        task.current_run().map(|run| (run.started, run.outcome)),
        task.plan_runs.last().map(|run| run.started),
    )
}

/// Count tasks with a current coding run that has not landed or been dropped.
pub fn count(heads: &[TaskHead]) -> usize {
    heads
        .iter()
        .filter(|h| {
            coding_active(
                h.status,
                h.current_run.as_ref().map(|run| (run.started, run.outcome)),
                h.last_plan_run.as_ref().map(|run| run.started),
            )
        })
        .count()
}

/// The claim lock serializes this check with creation of coding launch reservations.
pub(crate) fn reason(store: &Store, task: &Task) -> Result<Option<String>> {
    if store.config.limits.capacity.is_some() {
        return crate::capacity::admission::unfinished_reason(store, task);
    }
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
        let dir = tempfile::tempdir().unwrap();
        let store = Store::init(dir.path()).unwrap();
        let task = store.create("planning", None, Status::Running, "").unwrap();
        let task = store.update(&task.id, |task| {
            task.runs.push(serde_json::from_value(serde_json::json!({
                "id":"R1", "agent":"fake", "worktree":"/tmp/w", "branch":"b",
                "base":"main", "base_sha":"head", "started":"2026-01-01T00:00:00Z", "outcome":"superseded"
            })).unwrap());
            task.plan_runs.push(serde_json::from_value(serde_json::json!({
                "id":"D1", "profile":"fake", "harness":"fake", "spec_revision":"x",
                "started":"2026-01-02T00:00:00Z", "output_dir":"/tmp/p", "rounds":0, "state":"planning"
            })).unwrap());
            Ok(())
        }).unwrap();
        let head = store.head(&task.id).unwrap();
        assert_eq!(count(std::slice::from_ref(&head)), 0);
        assert_eq!(
            crate::SlotUsage::from_heads(&[head], &store.config.limits).wip,
            0
        );
        assert_eq!(
            crate::SlotUsage::from_tasks(&[task], &store.config.limits).wip,
            0
        );
    }
}
