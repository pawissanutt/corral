//! Tracker Plans: member prerequisites, then aggregate verification and its disposition.
use corral_core::attention::{Assessment, Responsible, WaitClass};
use corral_core::{OperatingActionKind as Action, Status, TaskHeadFields};
use jiff::Timestamp;

use super::classify::{check, keyed_status, progress, wait};
use super::{Neighborhood, Phases, dependency_labels, row};

pub(super) fn aggregate(
    task: &TaskHeadFields,
    phases: &Phases,
    n: &Neighborhood<'_>,
    now: Timestamp,
    a: &mut Assessment,
) -> bool {
    if !task.tracker {
        return false;
    }
    let pending = task
        .children
        .iter()
        .filter(|child| {
            n.task(child)
                .is_none_or(|task| *task.status() != Status::Done)
        })
        .collect::<Vec<_>>();
    if !pending.is_empty() {
        let labels = pending
            .iter()
            .map(|id| dependency_labels::label(n, id))
            .collect::<Vec<_>>()
            .join(", ");
        let mut condition = wait(
            task,
            now,
            "aggregate_prerequisite",
            WaitClass::Prerequisite,
            Responsible::Executor,
            format!("Waiting for Plan members: {labels}"),
        );
        condition.next_action.recipient = Some(labels);
        condition.resume.extend(
            pending
                .into_iter()
                .map(|id| check("task_status", id, Some("done".into()), false)),
        );
        a.conditions.push(condition);
        return true;
    }
    if let Some(attempt) = phases.aggregate.as_ref() {
        if let Some(gate) = attempt.gate.as_ref().filter(|gate| gate.holds_slot()) {
            a.progress = Some(progress(
                "aggregate_gate",
                Some(gate.at),
                "Aggregate gate is running".into(),
                "recorded",
            ));
        } else if let Some(review) = attempt
            .current_review
            .as_ref()
            .filter(|review| review.pending())
        {
            a.progress = Some(progress(
                "aggregate_reviewer",
                Some(review.requested),
                "Aggregate independent review is pending".into(),
                "recorded",
            ));
        } else {
            let since = attempt
                .current_review
                .as_ref()
                .and_then(|review| review.at)
                .unwrap_or(attempt.started);
            let mut condition = row(
                task,
                now,
                "aggregate_awaiting_owner",
                WaitClass::PmAction,
                Responsible::Pm,
                (Some(attempt.id.clone()), Some(since)),
                format!("Aggregate attempt {} awaits disposition", attempt.id),
            );
            condition
                .resume
                .push(check("aggregate_accepted", &attempt.id, None, false));
            condition.next_action.command = Some(format!("corral plan show {} --json", task.id));
            condition.next_action.description = "Inspect aggregate gate and review evidence, then run the plan verification or acceptance step explicitly".into();
            a.conditions.push(condition);
        }
    } else if n.allows(task, Action::AcceptAggregate) {
        a.progress = Some(progress(
            "aggregate",
            task.status_since,
            "Aggregate verification is pending".into(),
            "recorded",
        ));
    } else {
        let mut condition = keyed_status(
            task,
            now,
            "aggregate_awaiting_owner",
            WaitClass::PmAction,
            "Plan members are complete; the PM can run aggregate verification explicitly".into(),
        );
        condition
            .resume
            .push(check("aggregate_accepted", &task.id, None, false));
        condition.next_action.command = Some(format!("corral plan show {} --json", task.id));
        condition.next_action.description =
            "Inspect the plan and run its aggregate verification explicitly".into();
        a.conditions.push(condition);
    }
    true
}
