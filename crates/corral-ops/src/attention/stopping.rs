//! Complete stopping evidence using only the already-frozen neighborhood.
use super::*;
#[cfg(test)]
use corral_core::Run;
use corral_core::agent::HostHealth;
use corral_core::{GateResult, WaitHandle, awaited_task};

#[path = "stopping_inputs.rs"]
mod input;
pub(super) use input::Inputs;
use input::RunStop;

#[cfg(test)]
#[path = "stopping_tests.rs"]
mod tests;

pub(super) fn planner_owns_stop(
    plan: Option<&super::phase_inputs::PlanPhase>,
    run: &super::phase_inputs::PlanningPhase,
) -> bool {
    run.state == PlanRunState::AwaitingOwner
        || plan
            .filter(|plan| plan.approval.is_none())
            .is_none_or(|plan| {
                plan.run.as_deref() == Some(run.id.as_str())
                    || run.submitted.as_deref() == Some(plan.id.as_str())
            })
}

pub(super) fn launch_refusal(
    task: &TaskHeadFields,
    run: Option<&super::phase_inputs::RunPhase>,
    now: Timestamp,
    conditions: &mut Vec<Condition>,
) {
    if let Some(run) = run
        && run.outcome == Some(corral_core::RunOutcome::Failed)
        && let Some(reason) = run.blocked_excerpt.as_ref().filter(|text| {
            text.contains("corral agent launch refused: memory_contract_unavailable:")
        })
    {
        let reason = if reason.contains("corral agent install") {
            reason.clone()
        } else {
            format!(
                "{reason}\nFix: run `corral agent install` or update the configured Pi assets, then retry."
            )
        };
        let mut condition =
            keyed_status(task, now, "launch_failed", WaitClass::OwnerDecision, reason);
        condition
            .resume
            .push(check("launch_repaired", &task.id, None, false));
        conditions.push(condition);
    }
}

pub(super) fn load(ctx: &Ctx, n: &mut Neighborhood<'_>) -> Result<()> {
    n.missing_reviewers = crate::pane_liveness::recorded_missing(ctx)?;
    n.queue_refusals = crate::queue_drain::attention_refusals(ctx)?;
    for task in n
        .tasks
        .iter()
        .copied()
        .filter(|task| !matches!(task.status(), Status::Done | Status::Dropped))
    {
        if let Some(refusal) = crate::plan_review::prepared::blocker(ctx, task)? {
            n.prepared_refusals.insert(task.id().clone(), refusal);
        }
    }
    Ok(())
}

pub(super) fn conditions(task: &Task, n: &Neighborhood<'_>, now: Timestamp, a: &mut Assessment) {
    if matches!(task.status, Status::Idea | Status::Done | Status::Dropped) {
        return;
    }
    project(
        &corral_core::task_head_fields(task),
        &Phases::full(task),
        &Inputs::full(task),
        n,
        now,
        a,
    );
}

pub(super) fn project(
    task: &TaskHeadFields,
    phases: &Phases,
    inputs: &Inputs,
    n: &Neighborhood<'_>,
    now: Timestamp,
    a: &mut Assessment,
) {
    if matches!(task.status, Status::Idea | Status::Done | Status::Dropped) {
        return;
    }
    native_waits(inputs, n, a);
    if task.status == Status::Ready
        && let Some((at, reason)) = n.queue_refusals.get(&task.id)
    {
        let mut condition = row(
            task,
            now,
            "launch_failed",
            WaitClass::OwnerDecision,
            Responsible::Owner,
            (
                Some(crate::verification::digest(reason.as_bytes())),
                Some(*at),
            ),
            reason.clone(),
        );
        condition
            .resume
            .push(check("launch_repaired", &task.id, None, false));
        a.conditions.push(condition);
        a.progress = None;
    }
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
    reviewers(task, phases, n, now, a);
    if task.status != Status::NeedsMe {
        classify::escalation(task, phases.run.as_ref(), now, &mut a.conditions);
    }
    // An old working observation is not evidence of progress past a native suspension.
    for run in &inputs.runs {
        let lost = unavailable(run, n);
        if run.blocking_question || lost {
            a.progress = None;
        }
        if lost && !a.conditions.iter().any(|row| row.kind == "agent_lost") {
            let since = run
                .herdr
                .as_ref()
                .and_then(|herdr| {
                    n.missing_reviewers
                        .get(&format!("{}@{}", herdr.pane_id, run.started))
                })
                .copied()
                .or(task.status_since)
                .or(run.idle_since);
            let mut condition = row(
                task,
                now,
                "agent_lost",
                WaitClass::PmAction,
                Responsible::Pm,
                (Some(run.id.clone()), since),
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

fn unavailable(run: &RunStop, n: &Neighborhood<'_>) -> bool {
    run.host_health
        .is_some_and(|health| matches!(health, HostHealth::Lost | HostHealth::Fenced))
        || (n.inventory_known && run.agent_state.as_deref() == Some("gone"))
        || run.herdr.as_ref().is_some_and(|herdr| {
            n.missing_reviewers
                .contains_key(&format!("{}@{}", herdr.pane_id, run.started))
        })
}

fn native_waits(inputs: &Inputs, n: &Neighborhood<'_>, a: &mut Assessment) {
    let waits = inputs
        .runs
        .iter()
        .flat_map(|run| run.waits.iter().map(move |wait| (run, wait)));
    for (condition, (waiting_run, wait)) in a
        .conditions
        .iter_mut()
        .filter(|condition| condition.kind == "obligation_wait")
        .zip(waits)
    {
        let completed = if let Some(dependency) = awaited_task(&wait.request.handle) {
            let status = n
                .task(dependency)
                .map_or("missing", |task| task.status().as_str());
            let done = n
                .task(dependency)
                .is_some_and(|task| *task.status() == Status::Done);
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
            done
        } else if let WaitHandle::Gate { sha, pane } = &wait.request.handle {
            let Some(gate) = waiting_run.gate.as_ref().filter(|gate| {
                &gate.sha == sha
                    && gate.pane.as_ref() == Some(pane)
                    && matches!(
                        gate.result,
                        GateResult::Pass | GateResult::Fail | GateResult::Stale
                    )
            }) else {
                continue;
            };
            condition.resume.push(check(
                "gate_result",
                &format!("{sha}@{pane}"),
                Some(gate.result.as_str().into()),
                true,
            ));
            condition.reason = format!(
                "{}; gate {sha} ({}); missing receipt {}",
                wait.request.condition,
                gate.result.as_str(),
                wait.request.id
            );
            true
        } else {
            continue;
        };
        if completed {
            condition.reason_code = "wait_receipt_missing".into();
            condition.responsible = Responsible::Pm;
            condition.next_action.command = Some("corral sync".into());
            condition.next_action.description =
                "Reconcile the native wait receipt before resuming the parked run".into();
        }
    }
}

fn missing(
    task: &TaskHeadFields,
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

fn reviewers(
    task: &TaskHeadFields,
    phases: &Phases,
    n: &Neighborhood<'_>,
    now: Timestamp,
    a: &mut Assessment,
) {
    if phases.run.as_ref().is_none_or(|run| run.outcome.is_some())
        && let Some(plan) = phases.plan.as_ref().filter(|plan| plan.approval.is_none())
        && let Some(review) = plan
            .current_review
            .as_ref()
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
        && let Some(attempt) = phases.aggregate.as_ref()
        && !attempt.gate.as_ref().is_some_and(|gate| gate.holds_slot())
        && let Some(review) = attempt
            .current_review
            .as_ref()
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
