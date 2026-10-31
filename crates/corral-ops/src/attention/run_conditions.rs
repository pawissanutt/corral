//! Shared run condition rules over scalar state and active question/obligation rows.
use super::{Condition, Responsible, Timestamp, WaitClass, check, question, row};
use anyhow::{Result, bail};
use corral_core::agent::{AgentQuestion, ContainerRef, HostHealth};
use corral_core::{
    Run, RunHead, RunObligations, Status, StuckProcess, Task, TaskFieldsRef, TaskRef,
};

struct Inputs<'a> {
    id: &'a str,
    questions: &'a [AgentQuestion],
    obligations: &'a RunObligations,
    lost: bool,
    idle_no_report: bool,
    idle_since: Option<Timestamp>,
    silent_reason: Option<String>,
    stuck: Option<&'a StuckProcess>,
    parked: bool,
}

pub(super) fn run_conditions(
    task: &Task,
    run: &Run,
    now: Timestamp,
    out: &mut Vec<Condition>,
    review: bool,
) {
    let inputs = Inputs {
        id: &run.id,
        questions: run
            .agent_host
            .as_ref()
            .map_or(&[][..], |host| host.questions.as_slice()),
        obligations: &run.obligations,
        lost: super::lost(run),
        idle_no_report: run.idle_no_report,
        idle_since: run.idle_since,
        silent_reason: if review {
            Some(crate::round_notice::review_silent_reason(run))
        } else {
            crate::round_notice::worker_silent_reason(run)
        },
        stuck: run.stuck.as_ref(),
        parked: !run.parked_on(&task.id).is_empty(),
    };
    project(task, &inputs, now, out, review);
}

fn one_ref(
    task: &(impl TaskRef + ?Sized),
    run: &RunHead,
    container: &ContainerRef,
    now: Timestamp,
    out: &mut Vec<Condition>,
    review: bool,
) -> Result<()> {
    let questions = if run.agent_host.is_some() {
        task.pending_agent_questions(container)?
    } else {
        Vec::new()
    };
    let obligations = match container {
        ContainerRef::Worker { run, .. } => task.unreceipted_obligations(run)?,
        ContainerRef::Reviewer { run, review, .. } => {
            task.unreceipted_review_obligations(run, review)?
        }
        ContainerRef::Planner { .. }
        | ContainerRef::PlanReviewer { .. }
        | ContainerRef::Pm
        | ContainerRef::Assistant { .. } => {
            bail!("run conditions need a coding container")
        }
    };
    let parked = run.outcome.is_none()
        && !run
            .gate
            .as_ref()
            .is_some_and(corral_core::GateRecord::holds_slot)
        && !run
            .review
            .as_ref()
            .is_some_and(corral_core::ReviewRecord::holds_slot)
        && !obligations.parked_on(task.id()).is_empty();
    let inputs = Inputs {
        id: &run.id,
        questions: &questions,
        obligations: &obligations,
        lost: run.agent_state.as_deref() == Some("gone")
            || run.agent_host.as_ref().is_some_and(|host| {
                matches!(host.binding.health, HostHealth::Lost | HostHealth::Fenced)
            }),
        idle_no_report: run.idle_no_report,
        idle_since: run.idle_since,
        silent_reason: if review {
            Some(crate::round_notice::review_silent_reason_of(
                &run.id,
                run.review.as_ref(),
            ))
        } else {
            crate::round_notice::worker_silent_reason_of(&run.id, run.changes_round.as_ref())
        },
        stuck: run.stuck.as_ref(),
        parked,
    };
    project(task, &inputs, now, out, review);
    Ok(())
}

pub(in crate::attention) fn conditions_ref(
    task: &(impl TaskRef + ?Sized),
    now: Timestamp,
) -> Result<Vec<Condition>> {
    let mut out = Vec::new();
    if let Some(run) = task.current_run_head()? {
        if run.outcome.is_none() {
            one_ref(
                task,
                &run,
                &ContainerRef::Worker {
                    task: task.id().clone(),
                    run: run.id.clone(),
                },
                now,
                &mut out,
                false,
            )?;
        }
        for child in task
            .open_review_children(&run.id)?
            .into_iter()
            .filter(|child| child.ended.is_none() && child.outcome.is_none())
        {
            one_ref(
                task,
                &child,
                &ContainerRef::Reviewer {
                    task: task.id().clone(),
                    run: run.id.clone(),
                    review: child.id.clone(),
                },
                now,
                &mut out,
                true,
            )?;
        }
    }
    Ok(out)
}

fn project(
    task: &(impl TaskFieldsRef + ?Sized),
    run: &Inputs<'_>,
    now: Timestamp,
    out: &mut Vec<Condition>,
    review: bool,
) {
    out.extend(
        run.questions
            .iter()
            .filter(|question| question.recorded_pending())
            .map(|pending| question(task, pending, now)),
    );
    for decision in run
        .obligations
        .decisions
        .iter()
        .filter(|decision| decision.pending())
    {
        let mut condition = row(
            task,
            now,
            "decision_pending",
            WaitClass::OwnerDecision,
            Responsible::Owner,
            (
                Some(format!("{}/{}", run.id, decision.request.id)),
                Some(decision.registered_at),
            ),
            decision.request.question.clone(),
        );
        condition.resume.push(check(
            "decision_answered",
            &decision.request.id,
            None,
            false,
        ));
        condition.next_action.description = decision.request.requested_action.clone();
        out.push(condition);
    }
    for obligation in run
        .obligations
        .waits
        .iter()
        .filter(|wait| wait.receipt.is_none() && !wait.settled_job_exit())
    {
        let mut condition = row(
            task,
            now,
            "obligation_wait",
            WaitClass::ExternalOperation,
            Responsible::Executor,
            (None, Some(obligation.registered_at)),
            obligation.request.condition.clone(),
        );
        condition.reason_code = format!("wait_{}", obligation.observation.state.as_str());
        condition
            .resume
            .push(check("wait_receipt", &obligation.request.id, None, false));
        out.push(condition);
    }
    if *task.status() == Status::NeedsMe && run.lost {
        let since = task.status_since().or(run.idle_since);
        let mut condition = row(
            task,
            now,
            "agent_lost",
            WaitClass::PmAction,
            Responsible::Pm,
            (
                Some(format!(
                    "{}:{}",
                    run.id,
                    since.map_or_else(|| "legacy".into(), |at| at.to_string())
                )),
                since,
            ),
            format!("Run {} has recorded lost host or pane evidence", run.id),
        );
        condition
            .resume
            .push(check("run_recovered", run.id, None, false));
        out.push(condition);
    }
    if run.idle_no_report {
        let since = run.idle_since.or(*task.status_since());
        let reason = run
            .silent_reason
            .clone()
            .unwrap_or_else(|| format!("Run {} ended a turn without a report", run.id));
        let mut condition = row(
            task,
            now,
            if review {
                "review_turn_without_verdict"
            } else {
                "silent_turn"
            },
            WaitClass::PmAction,
            Responsible::Pm,
            (
                Some(format!(
                    "{}:{}",
                    run.id,
                    since.map_or_else(|| "legacy".into(), |at| at.to_string())
                )),
                since,
            ),
            reason,
        );
        condition
            .resume
            .push(check("run_reported", run.id, None, false));
        out.push(condition);
    }
    if let Some(stuck) = run.stuck.filter(|_| !run.parked) {
        let mut condition = row(
            task,
            now,
            "stuck_process",
            WaitClass::PmAction,
            Responsible::Pm,
            (
                Some(format!("{}:{}:{}", run.id, stuck.pid, stuck.started)),
                None,
            ),
            format!(
                "Run {} is blocked by process {} ({})",
                run.id,
                stuck.pid,
                stuck.argv.join(" ")
            ),
        );
        condition.resume.push(check(
            "process_exited",
            &format!("{}:{}", stuck.pid, stuck.started),
            None,
            false,
        ));
        out.push(condition);
    }
}
