use corral_core::agent::{AgentQuestion, HostHealth, QuestionState};
use corral_core::attention::*;
use corral_core::{
    Accept, AutoStage, GateResult, OperatingActionKind as Action, Run, RunOutcome, Status, Task,
    Verdict,
};
use jiff::Timestamp;

use super::{Neighborhood, aggregate, plan, ready, row};

pub(super) fn conditions(
    task: &Task,
    n: &Neighborhood<'_>,
    now: Timestamp,
    assessment: &mut Assessment,
) {
    let out = &mut assessment.conditions;
    if task.status == Status::Idea || (task.status == Status::Backlog && !task.queued) {
        out.push(wait(
            task,
            now,
            "not_scheduled",
            WaitClass::NotScheduled,
            Responsible::Owner,
            "No execution is scheduled".into(),
        ));
        return;
    }
    if let Some(run) = task.current_run() {
        if run.outcome.is_none() {
            run_conditions(task, run, now, out);
        }
        for reviewer in run
            .review_runs
            .iter()
            .filter(|reviewer| reviewer.ended.is_none() && reviewer.outcome.is_none())
        {
            run_conditions(task, reviewer, now, out);
        }
    }
    if let Some(infra) = &task.infra_wait {
        let held = infra.held_at.is_some();
        let mut condition = row(
            task,
            now,
            if held { "infra_hold" } else { "infra_wait" },
            WaitClass::ExternalOperation,
            if held {
                Responsible::Owner
            } else {
                Responsible::External
            },
            (
                held.then(|| infra.first_seen.to_string()),
                Some(infra.first_seen),
            ),
            format!(
                "Git {:?}: {} at {}",
                infra.class,
                infra.marker,
                infra.path.display()
            ),
        );
        condition.resume.push(check(
            "git_marker_absent",
            &infra.path.display().to_string(),
            None,
            false,
        ));
        condition.next_action.command = None;
        condition.next_action.description =
            "Wait for the Git operation to finish; inspect the marker without deleting it".into();
        out.push(condition);
    }
    if let Some(episode) = n.availability.get(&task.id) {
        let current = episode.current().and_then(|attempt| attempt.open_failure());
        if current.is_some() || episode.exhausted.is_some() {
            let since = episode
                .exhausted
                .as_ref()
                .map(|entry| entry.at)
                .or_else(|| current.map(|failure| failure.at));
            let mut condition = row(
                task,
                now,
                "provider_wait",
                WaitClass::ExternalOperation,
                Responsible::External,
                (None, since),
                episode.exhausted.as_ref().map_or_else(
                    || "Provider recovery is in progress".into(),
                    |entry| entry.reason.clone(),
                ),
            );
            condition
                .resume
                .push(check("provider_recovered", &episode.id, None, false));
            condition.next_action.command = None;
            condition.next_action.description =
                "Wait for the provider or its approved recovery executor".into();
            out.push(condition);
        }
    }
    if plan(task, n, now, assessment) {
        return;
    }
    if aggregate(task, n, now, assessment) {
        return;
    }
    if let Some(launch) = &task.launch {
        if let Some(error) = &launch.error {
            let run = task.current_run();
            let mut condition = row(
                task,
                now,
                "launch_failed",
                WaitClass::PmAction,
                Responsible::Pm,
                (
                    Some(format!(
                        "{}:{}",
                        run.map_or("none", |run| run.id.as_str()),
                        crate::verification::digest(error.as_bytes())
                    )),
                    run.map(|run| run.started).or(task.status_since),
                ),
                error.clone(),
            );
            condition
                .resume
                .push(check("launch_repaired", &task.id, None, false));
            assessment.conditions.push(condition);
            return;
        }
        if launch.reserved {
            assessment.progress = Some(progress(
                "launcher",
                task.current_run()
                    .map(|run| run.started)
                    .or(task.status_since),
                format!("Launch reservation {:?}", launch.stage),
                "recorded",
            ));
            return;
        }
    }
    match task.status {
        Status::Ready | Status::Backlog => ready(task, n, now, assessment),
        Status::Running | Status::NeedsMe => {
            if let Some(run) = task.current_run().filter(|run| run.outcome.is_none())
                && !(lost(run) && (n.inventory_known || task.status == Status::NeedsMe))
                && !run.idle_no_report
            {
                assessment.progress = Some(progress(
                    "worker",
                    Some(run.started),
                    format!(
                        "Worker {} is {}",
                        run.id,
                        run.agent_state.as_deref().unwrap_or("unknown")
                    ),
                    if n.inventory_known {
                        run.agent_state.as_deref().unwrap_or("unknown")
                    } else {
                        "unknown"
                    },
                ));
            }
            if task.status == Status::NeedsMe {
                escalation(task, now, &mut assessment.conditions);
                if assessment.conditions.is_empty() {
                    let mut condition = keyed_status(
                        task,
                        now,
                        "needs_me_other",
                        WaitClass::OwnerDecision,
                        "Task needs an owner decision without a more specific recorded condition"
                            .into(),
                    );
                    condition
                        .resume
                        .push(check("owner_disposition", &task.id, None, false));
                    assessment.conditions.push(condition);
                }
            }
        }
        Status::Review => review(task, n, now, assessment),
        Status::Idea | Status::Done | Status::Dropped => {}
    }
}

pub(super) fn progress(
    executor: &str,
    since: Option<Timestamp>,
    reason: String,
    liveness: &str,
) -> Progress {
    Progress {
        executor: executor.into(),
        since,
        reason,
        liveness: liveness.into(),
    }
}

pub(super) fn check(
    kind: &str,
    target: &str,
    expected: Option<String>,
    satisfied: bool,
) -> ResumeCheck {
    ResumeCheck {
        kind: kind.into(),
        target: Some(target.into()),
        expected,
        description: format!("{kind}: {target}"),
        machine_checkable: true,
        satisfied: Some(satisfied),
    }
}

pub(super) fn wait(
    task: &Task,
    now: Timestamp,
    kind: &str,
    class: WaitClass,
    by: Responsible,
    reason: String,
) -> Condition {
    row(
        task,
        now,
        kind,
        class,
        by,
        (None, task.status_since),
        reason,
    )
}

pub(super) fn keyed_status(
    task: &Task,
    now: Timestamp,
    kind: &str,
    class: WaitClass,
    reason: String,
) -> Condition {
    row(
        task,
        now,
        kind,
        class,
        if class == WaitClass::OwnerDecision {
            Responsible::Owner
        } else {
            Responsible::Pm
        },
        (
            Some(
                task.status_since
                    .map_or_else(|| "legacy".into(), |at| at.to_string()),
            ),
            task.status_since,
        ),
        reason,
    )
}

fn lost(run: &Run) -> bool {
    run.agent_state.as_deref() == Some("gone")
        || run.agent_host.as_ref().is_some_and(|host| {
            matches!(host.binding.health, HostHealth::Lost | HostHealth::Fenced)
        })
}

fn question(task: &Task, question: &AgentQuestion, now: Timestamp) -> Condition {
    let owner = question.state == QuestionState::Escalated || question.unattended_at.is_some();
    let kind = if question.unattended_at.is_some() {
        "question_unattended"
    } else if owner {
        "question_escalated"
    } else {
        "question_open"
    };
    let since = if owner {
        question
            .unattended_at
            .or_else(|| {
                question
                    .triage
                    .iter()
                    .rev()
                    .find(|triage| {
                        triage.revision == question.revision && triage.action == "escalate"
                    })
                    .map(|triage| triage.at)
            })
            .unwrap_or(question.asked.at)
    } else {
        question.asked.at
    };
    let mut condition = row(
        task,
        now,
        kind,
        if owner {
            WaitClass::OwnerDecision
        } else {
            WaitClass::PmAction
        },
        if owner {
            Responsible::Owner
        } else {
            Responsible::Pm
        },
        (
            Some(format!("{}@{}", question.key, question.revision)),
            Some(since),
        ),
        question.text.clone(),
    );
    condition.resume.push(check(
        "question_answered",
        &question.key,
        Some(question.revision.to_string()),
        false,
    ));
    condition.next_action.command = Some(format!("corral question show {} --json", question.key));
    condition.next_action.description =
        "Inspect and answer the exact question revision through its question workflow".into();
    condition
}

fn run_conditions(task: &Task, run: &Run, now: Timestamp, out: &mut Vec<Condition>) {
    if let Some(host) = &run.agent_host {
        out.extend(
            host.questions
                .iter()
                .filter(|question| question.recorded_pending())
                .map(|pending| question(task, pending, now)),
        );
    }
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
        .filter(|wait| wait.receipt.is_none())
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
    if task.status == Status::NeedsMe && lost(run) {
        let since = task.status_since.or(run.idle_since);
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
            .push(check("run_recovered", &run.id, None, false));
        out.push(condition);
    }
    if run.idle_no_report {
        let since = task.status_since.or(run.idle_since);
        let mut condition = row(
            task,
            now,
            "silent_turn",
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
            format!("Run {} ended a turn without a report", run.id),
        );
        condition
            .resume
            .push(check("run_reported", &run.id, None, false));
        out.push(condition);
    }
    if let Some(stuck) = &run.stuck {
        // Linux start ticks identify the process but are not a wall-clock timestamp.
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

fn escalation(task: &Task, now: Timestamp, out: &mut Vec<Condition>) {
    let Some(stage) = task
        .current_run()
        .and_then(|run| run.autopilot.as_ref())
        .filter(|stage| stage.stage == AutoStage::Escalated)
    else {
        return;
    };
    let reason = stage.reason.as_deref().unwrap_or("Autopilot escalated");
    let infra = reason.starts_with("integration preflight error");
    let mut condition = row(
        task,
        now,
        if infra {
            "infra_escalation"
        } else {
            "escalation"
        },
        if infra {
            WaitClass::PmAction
        } else {
            WaitClass::OwnerDecision
        },
        if infra {
            Responsible::Pm
        } else {
            Responsible::Owner
        },
        (Some(stage.at.to_string()), Some(stage.at)),
        reason.into(),
    );
    condition
        .resume
        .push(check("escalation_recovered", &task.id, None, false));
    if infra {
        let gate = task.current_run().and_then(|run| run.gate.as_ref());
        condition.next_action.description = format!(
            "Inspect infrastructure and use the T-377 round-free recovery path when installed; retained gate receipt: {}",
            gate.map_or_else(
                || "none".into(),
                |gate| format!("{} {:?} at {}", gate.sha, gate.result, gate.at)
            )
        );
        condition.next_action.command = Some(format!("corral show {} --json", task.id));
    }
    out.push(condition);
}

pub(super) fn authority(
    task: &Task,
    n: &Neighborhood<'_>,
    now: Timestamp,
    action: Action,
) -> Option<Condition> {
    if n.allows(task, action) {
        return None;
    }
    let reason = n
        .policy
        .tasks
        .get(&task.id)
        .and_then(|policy| policy.refusals.get(&action))
        .map(|reasons| reasons.join("; "))
        .unwrap_or_else(|| {
            if n.policy.reasons.is_empty() {
                "Task is outside the confirmed operating grant".into()
            } else {
                n.policy.reasons.join("; ")
            }
        });
    let mut condition = keyed_status(
        task,
        now,
        "authority_blocked",
        WaitClass::OwnerDecision,
        reason,
    );
    condition.key = Some(ConditionKey(format!(
        "{}/authority_blocked/{}",
        task.id,
        crate::verification::digest(condition.reason.as_bytes())
    )));
    condition.resume.push(check(
        "authority_granted",
        &task.id,
        Some(format!("{action:?}")),
        false,
    ));
    Some(condition)
}

fn review(task: &Task, n: &Neighborhood<'_>, now: Timestamp, a: &mut Assessment) {
    let Some(run) = task.current_run() else {
        return;
    };
    if run.outcome.is_none() {
        let mut condition = keyed_status(
            task,
            now,
            "manual_review_open_run",
            WaitClass::OwnerDecision,
            format!("Review contains open worker {}", run.id),
        );
        condition.key = Some(ConditionKey(format!(
            "{}/manual_review_open_run/{}",
            task.id, run.id
        )));
        condition
            .resume
            .push(check("run_reported", &run.id, None, false));
        a.conditions.push(condition);
    }
    if let Some(work) = task.review_wait {
        let mut condition = row(
            task,
            now,
            "review_capacity",
            WaitClass::Capacity,
            Responsible::Executor,
            (
                None,
                run.autopilot
                    .as_ref()
                    .map(|stage| stage.at)
                    .or(task.status_since),
            ),
            run.autopilot
                .as_ref()
                .and_then(|stage| stage.reason.clone())
                .unwrap_or_else(|| format!("Waiting for {work:?} capacity")),
        );
        condition.resume.push(check(
            "review_capacity",
            &task.id,
            Some(format!("{work:?}")),
            false,
        ));
        a.conditions.push(condition);
        return;
    }
    if let Some(gate) = run.gate.as_ref().filter(|gate| gate.holds_slot()) {
        a.progress = Some(progress(
            "gate",
            Some(gate.at),
            "Gate is running".into(),
            "recorded",
        ));
        return;
    }
    if let Some(reviewer) = run
        .review_runs
        .iter()
        .rev()
        .find(|reviewer| reviewer.outcome.is_none() && reviewer.ended.is_none())
        .filter(|_| {
            run.review
                .as_ref()
                .is_some_and(|review| review.verdict.is_none() && !review.process_lost)
        })
    {
        a.progress = Some(progress(
            "reviewer",
            Some(reviewer.started),
            "Independent reviewer is running".into(),
            if n.inventory_known {
                reviewer.agent_state.as_deref().unwrap_or("unknown")
            } else {
                "unknown"
            },
        ));
        return;
    }
    if let Some(review) = run
        .review
        .as_ref()
        .filter(|review| review.holds_slot() && review.pane.is_some())
    {
        a.progress = Some(progress(
            "reviewer",
            Some(review.at),
            "Independent reviewer is running".into(),
            "recorded",
        ));
        return;
    }
    if run.outcome != Some(RunOutcome::Done) {
        return;
    }
    let stage = run.autopilot.as_ref();
    if let Some(blocked) = stage.filter(|stage| stage.stage == AutoStage::Blocked) {
        let mut condition = row(
            task,
            now,
            "landing_blocked",
            WaitClass::ExternalOperation,
            Responsible::Owner,
            (Some(blocked.at.to_string()), Some(blocked.at)),
            blocked
                .reason
                .clone()
                .unwrap_or_else(|| "Landing is blocked".into()),
        );
        condition
            .resume
            .push(check("landing_precondition", &task.id, None, false));
        a.conditions.push(condition);
        return;
    }
    if stage.is_some_and(|stage| stage.stage == AutoStage::Reviewed)
        && n.config.task_review_policy(task).accept == Accept::Owner
    {
        let since = run
            .review
            .as_ref()
            .map(|review| review.at)
            .or_else(|| stage.map(|stage| stage.at));
        let mut condition = row(
            task,
            now,
            "owner_accept",
            WaitClass::OwnerDecision,
            Responsible::Owner,
            (
                Some(since.map_or_else(|| run.id.clone(), |at| at.to_string())),
                since,
            ),
            "Verified candidate awaits owner acceptance".into(),
        );
        condition
            .resume
            .push(check("candidate_accepted", &run.id, None, false));
        a.conditions.push(condition);
        return;
    }
    if task.infra_wait.is_some() {
        return;
    }
    let actionable = match stage.map(|stage| stage.stage) {
        None => Some(Action::Gate),
        Some(AutoStage::Changes) => Some(Action::Repair),
        Some(AutoStage::Reviewing)
            if run
                .review
                .as_ref()
                .is_some_and(|review| review.verdict.is_some()) =>
        {
            Some(Action::Review)
        }
        Some(AutoStage::Reviewed) => Some(Action::Accept),
        Some(AutoStage::Gating)
            if run
                .gate
                .as_ref()
                .is_some_and(|gate| gate.result != GateResult::Running) =>
        {
            Some(Action::Gate)
        }
        Some(AutoStage::Landing) => Some(Action::Accept),
        Some(
            AutoStage::Gating
            | AutoStage::Reviewing
            | AutoStage::Done
            | AutoStage::Escalated
            | AutoStage::Blocked,
        ) => None,
    };
    if let Some(action) = actionable {
        // Reviewing consumes the current run's recorded verdict under Review authority.
        // Sending Changes back additionally requires Repair, unless exhausted rounds
        // make send_request escalate before its Repair guard.
        let blocked = authority(task, n, now, action).or_else(|| {
            (action == Action::Review
                && run.review.as_ref().and_then(|review| review.verdict) == Some(Verdict::Changes)
                && task.rounds < n.config.task_review_policy(task).max_rounds)
                .then(|| authority(task, n, now, Action::Repair))
                .flatten()
        });
        if let Some(condition) = blocked {
            a.conditions.push(condition);
        } else {
            a.progress = Some(progress(
                "autopilot",
                stage.map(|stage| stage.at).or(task.status_since),
                "Authorized review progression".into(),
                "recorded",
            ));
        }
    }
}

#[cfg(test)]
#[path = "review_tests.rs"]
mod tests;
