//! Stopping selection from task facets and exactly named question rows.
use super::{Assessment, Condition, select_blocked};
use crate::{Status, TaskRef};
use anyhow::Result;
use std::collections::BTreeSet;

/// `blocked` over bounded task metadata. A non-blocking question may belong to a retired
/// child of the CURRENT run, but never to a different run or a planning container.
/// Read failures remain errors, not evidence that the task is making progress.
pub fn blocked_ref<'a>(
    task: &(impl TaskRef + ?Sized),
    assessment: &'a Assessment,
) -> Result<Option<&'a Condition>> {
    let status = *task.status();
    if matches!(status, Status::Idea | Status::Done | Status::Dropped) {
        return Ok(None);
    }
    let run = task.current_run_head()?;
    if matches!(status, Status::Backlog | Status::Ready)
        && run.is_none()
        && task.plan_runs_len()? == 0
        && task.plans_len()? == 0
        && !*task.tracker()
        && task.park().is_none()
        && task
            .launch()
            .as_ref()
            .is_none_or(|launch| launch.error.is_none())
    {
        return Ok(None);
    }
    let mut nonblocking = BTreeSet::new();
    if let Some(run) = run {
        let targets = assessment
            .primary
            .iter()
            .chain(&assessment.conditions)
            .filter(|condition| condition.kind == "question_open")
            .flat_map(|condition| &condition.resume)
            .filter(|check| check.kind == "question_answered")
            .filter_map(|check| check.target.as_deref())
            .collect::<BTreeSet<_>>();
        for target in targets {
            if task
                .run_question_by_key(&run.id, target)?
                .is_some_and(|question| !question.blocking)
            {
                nonblocking.insert(target);
            }
        }
    }
    Ok(select_blocked(status, assessment, |condition| {
        condition.kind == "question_open"
            && condition.resume.iter().any(|check| {
                check.kind == "question_answered"
                    && check
                        .target
                        .as_deref()
                        .is_some_and(|target| nonblocking.contains(target))
            })
    }))
}
