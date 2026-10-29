//! Complete stopping evidence using only the already-frozen neighborhood.
use super::*;
use corral_core::agent::HostHealth;
use corral_core::{Run, awaited_task};

pub(super) fn planner_owns_stop(task: &Task, run: &corral_core::PlanRun) -> bool {
    run.state == PlanRunState::AwaitingOwner
        || task
            .plans
            .last()
            .filter(|plan| plan.approval.is_none())
            .is_none_or(|plan| {
                plan.run.as_deref() == Some(run.id.as_str())
                    || run.submitted.as_deref() == Some(plan.id.as_str())
            })
}

pub(super) fn load(ctx: &Ctx, n: &mut Neighborhood<'_>) -> Result<()> {
    n.missing_reviewers = crate::pane_liveness::recorded_missing(ctx)?;
    for task in n
        .tasks
        .iter()
        .filter(|task| !matches!(task.status, Status::Done | Status::Dropped))
    {
        if let Some(refusal) = crate::plan_review::prepared::blocker(ctx, task)? {
            n.prepared_refusals.insert(task.id.clone(), refusal);
        }
    }
    Ok(())
}

pub(super) fn conditions(task: &Task, n: &Neighborhood<'_>, now: Timestamp, a: &mut Assessment) {
    if matches!(task.status, Status::Idea | Status::Done | Status::Dropped) {
        return;
    }
    native_waits(task, n, a);
    if let Some((at, reason)) = n.prepared_refusals.get(&task.id) {
        let mut condition = row(
            task,
            now,
            "authority_blocked",
            WaitClass::OwnerDecision,
            Responsible::Owner,
            (
                Some(crate::verification::digest(reason.as_bytes())),
                Some(*at),
            ),
            reason.clone(),
        );
        condition.resume.push(check(
            "authority_granted",
            &task.id,
            Some("PlanReview".into()),
            false,
        ));
        a.conditions.push(condition);
        a.progress = None;
    }
    reviewers(task, n, now, a);
    if task.status != Status::NeedsMe {
        classify::escalation(task, now, &mut a.conditions);
    }
    // An old working observation is not evidence of progress past a native suspension.
    if let Some(run) = task.current_run().filter(|run| run.outcome.is_none()) {
        let question = run.agent_host.as_ref().is_some_and(|host| {
            host.questions
                .iter()
                .any(|question| question.blocking && question.recorded_pending())
        });
        if question || unavailable(run, n) {
            a.progress = None;
        }
        if unavailable(run, n) && !a.conditions.iter().any(|row| row.kind == "agent_lost") {
            let mut condition = row(
                task,
                now,
                "agent_lost",
                WaitClass::PmAction,
                Responsible::Pm,
                (Some(run.id.clone()), task.status_since.or(run.idle_since)),
                format!("Run {} has recorded lost host or pane evidence", run.id),
            );
            condition
                .resume
                .push(check("run_recovered", &run.id, None, false));
            a.conditions.push(condition);
        }
    }
    if a.conditions
        .iter()
        .any(|condition| condition.kind == "obligation_wait")
    {
        a.progress = None;
    }
}

fn unavailable(run: &Run, n: &Neighborhood<'_>) -> bool {
    run.agent_host
        .as_ref()
        .is_some_and(|host| matches!(host.binding.health, HostHealth::Lost | HostHealth::Fenced))
        || (n.inventory_known && run.agent_state.as_deref() == Some("gone"))
        || run.herdr.as_ref().is_some_and(|herdr| {
            n.missing_reviewers
                .contains_key(&format!("{}@{}", herdr.pane_id, run.started))
        })
}

fn native_waits(task: &Task, n: &Neighborhood<'_>, a: &mut Assessment) {
    let Some(run) = task.current_run() else {
        return;
    };
    let waits = std::iter::once(run)
        .filter(|run| run.outcome.is_none())
        .chain(
            run.review_runs
                .iter()
                .filter(|run| run.outcome.is_none() && run.ended.is_none()),
        )
        .flat_map(|run| &run.obligations.waits)
        .filter(|wait| wait.receipt.is_none());
    for (condition, wait) in a
        .conditions
        .iter_mut()
        .filter(|condition| condition.kind == "obligation_wait")
        .zip(waits)
    {
        let Some(dependency) = awaited_task(&wait.request.handle) else {
            continue;
        };
        let status = n
            .task(dependency)
            .map_or("missing", |task| task.status.as_str());
        let done = n
            .task(dependency)
            .is_some_and(|task| task.status == Status::Done);
        condition.next_action.recipient = Some(format!("{dependency} ({status})"));
        condition
            .resume
            .push(check("task_status", dependency, Some("done".into()), done));
        condition.reason = format!(
            "{}; {dependency} ({status}); {}receipt {}",
            wait.request.condition,
            if done { "missing " } else { "awaiting " },
            wait.request.id
        );
        if done {
            condition.reason_code = "wait_receipt_missing".into();
            condition.responsible = Responsible::Pm;
            condition.next_action.command = Some("corral sync".into());
            condition.next_action.description =
                "Reconcile the native wait receipt before resuming the parked run".into();
        }
    }
}

fn missing(
    task: &Task,
    now: Timestamp,
    kind: &str,
    id: &str,
    since: Timestamp,
    reason: String,
) -> Condition {
    let mut condition = row(
        task,
        now,
        kind,
        WaitClass::PmAction,
        Responsible::Pm,
        (Some(id.into()), Some(since)),
        reason,
    );
    condition
        .resume
        .push(check("reviewer_recovered", id, None, false));
    condition.next_action.command = Some(format!("corral plan show {} --json", task.id));
    condition.next_action.description =
        "Inspect the recorded review and recover its independent reviewer".into();
    condition
}

fn reviewers(task: &Task, n: &Neighborhood<'_>, now: Timestamp, a: &mut Assessment) {
    if task.current_run().is_none_or(|run| run.outcome.is_some())
        && let Some(plan) = task.plans.last().filter(|plan| plan.approval.is_none())
        && let Some(review) = plan
            .reviews
            .last()
            .filter(|review| review.verdict.is_none())
        && (review.herdr.is_none()
            || review.herdr.as_ref().is_some_and(|herdr| {
                n.missing_reviewers
                    .contains_key(&format!("{}@{}", herdr.pane_id, review.at))
            }))
        && !a.conditions.iter().any(|row| {
            matches!(
                row.kind.as_str(),
                "authority_blocked"
                    | "plan_review_capacity"
                    | "plan_failed"
                    | "plan_awaiting_owner"
            )
        })
    {
        a.progress = None;
        a.conditions.push(missing(
            task,
            now,
            "plan_reviewer_missing",
            &format!("{}:{}", plan.id, review.id),
            review
                .herdr
                .as_ref()
                .and_then(|herdr| {
                    n.missing_reviewers
                        .get(&format!("{}@{}", herdr.pane_id, review.at))
                })
                .copied()
                .unwrap_or(review.at),
            "Recorded plan review has no live independent reviewer".into(),
        ));
    }
    if task.tracker
        && let Some(attempt) = task.aggregate_attempts.last()
        && !attempt.gate.as_ref().is_some_and(|gate| gate.holds_slot())
        && let Some(review) = attempt
            .reviews
            .last()
            .filter(|review| review.verdict.is_none())
        && (review.herdr.is_none()
            || review.abandoned.is_some()
            || review.herdr.as_ref().is_some_and(|herdr| {
                n.missing_reviewers
                    .contains_key(&format!("{}@{}", herdr.pane_id, review.requested))
            }))
    {
        a.progress = None;
        a.conditions
            .retain(|row| row.kind != "aggregate_awaiting_owner");
        if let Some(condition) = authority(task, n, now, Action::AcceptAggregate) {
            a.conditions.push(condition);
        } else {
            a.conditions.push(missing(
                task,
                now,
                "aggregate_reviewer_missing",
                &format!("{}:{}", attempt.id, review.id),
                review
                    .herdr
                    .as_ref()
                    .and_then(|herdr| {
                        n.missing_reviewers
                            .get(&format!("{}@{}", herdr.pane_id, review.requested))
                    })
                    .copied()
                    .or(review.at)
                    .unwrap_or(review.requested),
                review.abandoned.clone().unwrap_or_else(|| {
                    "Recorded aggregate review has no launched independent reviewer".into()
                }),
            ));
        }
    }
}
