//! Read-only attention projection. Canonical records, not notifications, define outstanding work.
use std::collections::{BTreeMap, BTreeSet};

use anyhow::Result;
use classify::{authority, check, keyed_status, progress, wait};
use corral_core::attention::{
    Assessment, Condition, ConditionKey, NextAction, Responsible, SinceSource, WaitClass,
};
use corral_core::inbox::{Inbox, InboxState, OwnerEntry, Recipient};
use corral_core::{
    AvailabilityEpisode, Config, EffectivePolicy, OperatingActionKind as Action, PlanRunState,
    Status, Task, Verdict,
};
use jiff::Timestamp;
use serde::Serialize;

use crate::Ctx;

mod classify;
#[cfg(test)]
mod classify_tests;
mod delivery;
mod handling;
mod outbox;
pub use handling::{attend, guard};
pub(crate) use handling::{reconcile, reconcile_projected};
#[cfg(test)]
pub(crate) mod handling_tests;

/// Everything assessment may inspect. Building this view never probes Git or pane inventory.
pub struct Neighborhood<'a> {
    pub tasks: &'a [Task],
    pub config: &'a Config,
    pub policy: &'a EffectivePolicy,
    pub deliveries: BTreeMap<String, Vec<OwnerEntry>>,
    /// False means inventory is unknown, never evidence that a pane disappeared.
    pub inventory_known: bool,
    pub resource_wait: Option<String>,
    pub availability: BTreeMap<String, AvailabilityEpisode>,
    by_id: BTreeMap<&'a str, &'a Task>,
    slots: usize,
    review_slots: usize,
    project_slots: BTreeMap<Option<&'a str>, usize>,
}

impl<'a> Neighborhood<'a> {
    pub fn new(tasks: &'a [Task], config: &'a Config, policy: &'a EffectivePolicy) -> Self {
        let mut slots = 0;
        let mut review_slots = 0;
        let mut project_slots = BTreeMap::new();
        for task in tasks {
            let usage =
                crate::slots::SlotUsage::from_tasks(std::slice::from_ref(task), &config.limits);
            slots += usage.total();
            review_slots += usage.reviews + usage.gates;
            *project_slots.entry(task.project.as_deref()).or_default() += usage.total();
        }
        Self {
            tasks,
            config,
            policy,
            deliveries: BTreeMap::new(),
            inventory_known: false,
            resource_wait: None,
            availability: BTreeMap::new(),
            by_id: tasks.iter().map(|task| (task.id.as_str(), task)).collect(),
            slots,
            review_slots,
            project_slots,
        }
    }

    pub fn task(&self, id: &str) -> Option<&Task> {
        self.by_id.get(id).copied()
    }

    pub fn allows(&self, task: &Task, action: corral_core::OperatingActionKind) -> bool {
        self.policy.allows(&task.id, action)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TaskAssessment {
    pub task: String,
    pub title: String,
    pub project: Option<String>,
    pub status: Status,
    #[serde(flatten)]
    pub assessment: Assessment,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AttentionBoard {
    pub version: u32,
    pub tasks: Vec<TaskAssessment>,
}

/// Assess all conditions, independently of progress and display precedence.
pub fn assess(task: &Task, neighborhood: &Neighborhood<'_>, now: Timestamp) -> Assessment {
    if matches!(task.status, Status::Done | Status::Dropped) {
        return Assessment::default();
    }
    corral_core::work::record(corral_core::work::Work::AttentionVisit, 1);
    let mut assessment = Assessment::default();
    classify::conditions(task, neighborhood, now, &mut assessment);
    delivery::conditions(task, neighborhood, now, &mut assessment.conditions);
    if assessment.progress.is_none() && assessment.conditions.is_empty() {
        let run = task.current_run();
        let stage = run.and_then(|run| run.autopilot.as_ref());
        let anchor = format!(
            "{}:{}:{}",
            task.status.as_str(),
            stage.map_or("none", |stage| stage.stage.as_str()),
            stage.map_or_else(
                || task
                    .status_since
                    .map_or_else(|| "legacy".into(), |at| at.to_string()),
                |stage| stage.at.to_string(),
            )
        );
        assessment.conditions.push(row(
            task,
            now,
            "unexpected_stall",
            WaitClass::PmAction,
            Responsible::Pm,
            (
                Some(anchor),
                stage.map(|stage| stage.at).or(task.status_since),
            ),
            format!(
                "No authorized executor matched status={}, run={}, outcome={:?}, stage={}",
                task.status.as_str(),
                run.map_or("none", |run| run.id.as_str()),
                run.and_then(|run| run.outcome),
                stage.map_or("none", |stage| stage.stage.as_str()),
            ),
        ));
    }
    refresh_overflow(task, &mut assessment, now);
    assessment
}

// Publication changes only ledger capacity, not the canonical inputs of ordinary rows.
// Reuse this projection after reclaim without another board listing or receipt reads.
fn refresh_overflow(task: &Task, assessment: &mut Assessment, now: Timestamp) {
    assessment
        .conditions
        .retain(|condition| condition.kind != "ledger_overflow");
    let retained = task
        .attention
        .records
        .values()
        .filter(|record| record.kind != "ledger_overflow")
        .count();
    let available = 31_usize.saturating_sub(retained);
    let missing = assessment
        .conditions
        .iter()
        .filter_map(|condition| condition.key.as_ref())
        .filter(|key| !task.attention.records.contains_key(*key))
        .collect::<BTreeSet<_>>()
        .len()
        .saturating_sub(available);
    if missing > 0
        && let Some((key, _)) =
            task.attention.records.iter().find(|(_, record)| {
                record.kind == "ledger_overflow" && record.resolved_at.is_none()
            })
    {
        let anchor = key
            .0
            .trim_start_matches(&format!("{}/ledger_overflow/", task.id))
            .to_owned();
        let mut overflow = row(
            task,
            now,
            "ledger_overflow",
            WaitClass::PmAction,
            Responsible::Pm,
            (Some(anchor), None),
            format!(
                "{missing} conditions are listed without ladder records; resolve retained conditions to free capacity"
            ),
        );
        overflow.next_action.command = Some("corral attention --json --all".into());
        assessment.conditions.push(overflow);
    }
    assessment.conditions.sort_by(|a, b| {
        (a.key.is_none(), a.class, a.since, &a.kind, &a.key).cmp(&(
            b.key.is_none(),
            b.class,
            b.since,
            &b.kind,
            &b.key,
        ))
    });
    assessment.primary = assessment.conditions.first().cloned();
}

/// One fresh task listing. Receipt reads are bounded by the existing owner header.
pub fn board(ctx: &Ctx, now: Timestamp) -> Result<AttentionBoard> {
    Ok(board_snapshot(ctx, now)?.1)
}

pub(super) fn board_snapshot(
    ctx: &Ctx,
    now: Timestamp,
) -> Result<(std::sync::Arc<Vec<Task>>, AttentionBoard)> {
    let frozen = ctx.store.read_only_snapshot()?;
    let tasks = frozen.list_shared()?;
    let policy = frozen.effective_operating_policy()?;
    let mut neighborhood = Neighborhood::new(&tasks, &ctx.store.config, &policy);
    neighborhood.resource_wait = crate::resources::read_admission(ctx)?;
    for task in tasks
        .iter()
        .filter(|task| !matches!(task.status, Status::Done | Status::Dropped))
    {
        for (recipient, state) in recipients(task) {
            let inbox = Inbox::new(&ctx.store, recipient)?;
            if !inbox.recipient_ended(task) {
                neighborhood
                    .deliveries
                    .insert(inbox.key().into(), inbox.owner_entries(state)?);
            }
        }
        if let Some(run) = task.current_run().filter(|run| run.outcome.is_none())
            && let Some(episode) =
                crate::availability::load(ctx, &crate::availability::run_episode(&task.id, run))?
        {
            neighborhood.availability.insert(task.id.clone(), episode);
        }
    }
    let board = AttentionBoard {
        version: 1,
        tasks: tasks
            .iter()
            .map(|task| TaskAssessment {
                task: task.id.clone(),
                title: task.title.clone(),
                project: task.project.clone(),
                status: task.status,
                assessment: assess(task, &neighborhood, now),
            })
            .collect(),
    };
    Ok((tasks, board))
}

pub(super) fn recipients(task: &Task) -> Vec<(Recipient, &InboxState)> {
    let mut recipients = Vec::new();
    if let Some(run) = task.current_run() {
        recipients.push((
            Recipient::Run {
                task: task.id.clone(),
                run: run.id.clone(),
            },
            &run.inbox,
        ));
        for review in &run.review_runs {
            if review.outcome.is_none() && review.ended.is_none() {
                recipients.push((
                    Recipient::ReviewRun {
                        task: task.id.clone(),
                        run: run.id.clone(),
                        review: review.id.clone(),
                    },
                    &review.inbox,
                ));
            }
        }
    }
    if let Some(run) = task.plan_runs.last().filter(|run| run.holds_slot()) {
        recipients.push((
            Recipient::PlanRun {
                task: task.id.clone(),
                plan_run: run.id.clone(),
            },
            &run.inbox,
        ));
    }
    for plan in &task.plans {
        if let Some(review) = plan
            .reviews
            .last()
            .filter(|review| review.verdict.is_none())
        {
            recipients.push((
                Recipient::PlanReview {
                    task: task.id.clone(),
                    plan: plan.id.clone(),
                    review: review.id.clone(),
                },
                &review.inbox,
            ));
        }
    }
    recipients
}

pub(super) fn row(
    task: &Task,
    now: Timestamp,
    kind: &str,
    class: WaitClass,
    responsible: Responsible,
    source: (Option<String>, Option<Timestamp>),
    reason: String,
) -> Condition {
    let key = source
        .0
        .map(|anchor| ConditionKey(format!("{}/{kind}/{anchor}", task.id)));
    let canonical = source.1;
    let since = canonical.or_else(|| {
        key.as_ref()
            .and_then(|key| task.attention.records.get(key))
            .map(|record| record.first_recorded)
    });
    Condition {
        key,
        kind: kind.into(),
        class,
        reason_code: kind.into(),
        reason,
        responsible,
        since,
        since_source: if canonical.is_some() {
            SinceSource::Canonical
        } else {
            SinceSource::Discovered
        },
        age_secs: since.map(|since| now.duration_since(since).as_secs().max(0)),
        resume: Vec::new(),
        next_action: NextAction {
            command: Some(format!("corral show {} --json", task.id)),
            description: "Inspect the canonical task and its executor evidence".into(),
            requires_owner: responsible == Responsible::Owner,
            recipient: None,
        },
    }
}

fn ready(task: &Task, n: &Neighborhood<'_>, now: Timestamp, a: &mut Assessment) {
    let mut waits = Vec::new();
    if n.config.dispatch.paused || n.policy.paused {
        let mut condition = wait(
            task,
            now,
            "dispatch_paused",
            WaitClass::Hold,
            Responsible::Owner,
            "Dispatch is paused".into(),
        );
        condition
            .resume
            .push(check("hold_lifted", "dispatch", None, false));
        waits.push(condition);
    }
    if task.dispatch_hold {
        let mut condition = wait(
            task,
            now,
            "dispatch_hold",
            WaitClass::Hold,
            Responsible::Owner,
            "Owner dispatch hold".into(),
        );
        condition
            .resume
            .push(check("hold_lifted", &task.id, None, false));
        waits.push(condition);
    }
    if !n.config.ready_project_allowed(task.project.as_deref()) {
        let mut condition = wait(
            task,
            now,
            "project_not_allowed",
            WaitClass::Hold,
            Responsible::Owner,
            "Project is not allowed for Ready admission".into(),
        );
        condition.resume.push(check(
            "project_allowed",
            task.project.as_deref().unwrap_or("none"),
            None,
            false,
        ));
        waits.push(condition);
    }
    for dependency in &task.depends_on {
        if n.task(dependency)
            .is_none_or(|task| task.status != Status::Done)
        {
            let mut condition = wait(
                task,
                now,
                "prerequisite",
                WaitClass::Prerequisite,
                Responsible::Executor,
                format!(
                    "Waiting on {dependency} ({})",
                    n.task(dependency)
                        .map_or("missing", |task| task.status.as_str())
                ),
            );
            condition
                .resume
                .push(check("task_status", dependency, Some("done".into()), false));
            waits.push(condition);
        }
    }
    let action = if task.dispatch_plan.as_ref().is_some_and(|plan| plan.plan) {
        Action::Plan
    } else {
        Action::Dispatch
    };
    // A one-task explicit queue authorization is independent of automatic policy.
    let explicit = task.queued
        && task
            .dispatch_plan
            .as_ref()
            .and_then(|plan| plan.authorization.as_ref())
            .is_some();
    if !explicit && !n.allows(task, action) {
        let mut condition = wait(
            task,
            now,
            "outside_grant",
            WaitClass::Hold,
            Responsible::Owner,
            "Dispatch needs a confirmed task grant or an explicit queued owner start".into(),
        );
        condition.resume.push(check(
            "authority_granted",
            &task.id,
            Some(format!("{action:?}")),
            false,
        ));
        waits.push(condition);
    }
    let credit = usize::from(task.holds_dispatch_slot());
    let usage = n.slots.saturating_sub(credit);
    let project_usage = n
        .project_slots
        .get(&task.project.as_deref())
        .copied()
        .unwrap_or(0)
        .saturating_sub(credit);
    let project_cap = task
        .project
        .as_ref()
        .and_then(|project| n.config.limits.per_project.get(project));
    if usage >= n.config.limits.running
        || project_cap.is_some_and(|cap| project_usage >= *cap)
        || n.resource_wait.is_some()
    {
        let mut condition = wait(
            task,
            now,
            "launch_capacity",
            WaitClass::Capacity,
            Responsible::Executor,
            n.resource_wait.clone().unwrap_or_else(|| {
                format!(
                    "Waiting for dispatch capacity ({usage}/{})",
                    n.config.limits.running
                )
            }),
        );
        condition
            .resume
            .push(check("dispatch_capacity", &task.id, None, false));
        waits.push(condition);
    }
    if waits.is_empty() {
        a.progress = Some(progress(
            "dispatcher",
            task.status_since,
            "Authorized dispatch is ready".into(),
            "recorded",
        ));
    }
    a.conditions.extend(waits);
}

fn plan(task: &Task, n: &Neighborhood<'_>, now: Timestamp, a: &mut Assessment) -> bool {
    if task.current_run().is_some_and(|run| run.outcome.is_none()) {
        return false;
    }
    let run = task.plan_runs.last();
    if let Some(run) = run.filter(|run| run.holds_slot()) {
        a.progress = Some(progress(
            "planner",
            Some(run.started),
            format!("Planner {} is active", run.id),
            "unknown",
        ));
        return true;
    }
    if let Some(run) = run.filter(|run| {
        matches!(
            run.state,
            PlanRunState::Failed
                | PlanRunState::Escalated
                | PlanRunState::Cancelled
                | PlanRunState::AwaitingOwner
        )
    }) {
        let awaiting = run.state == PlanRunState::AwaitingOwner;
        let since = task
            .plans
            .last()
            .and_then(|plan| plan.reviews.last())
            .map(|review| review.at)
            .or(run.ended);
        let mut condition = row(
            task,
            now,
            if awaiting {
                "plan_awaiting_owner"
            } else {
                "plan_failed"
            },
            WaitClass::OwnerDecision,
            Responsible::Owner,
            (Some(format!("{}:{:?}", run.id, run.state)), since),
            format!("Plan run {} is {:?}", run.id, run.state),
        );
        condition
            .resume
            .push(check("plan_disposition", &run.id, None, false));
        condition.next_action.command = Some(format!("corral plan show {} --json", task.id));
        a.conditions.push(condition);
        return true;
    }
    let Some(plan) = task.plans.last() else {
        return false;
    };
    if let Some(approval) = &plan.approval {
        if task.plan.as_deref() != Some(&format!("{}@{}", task.id, plan.id)) {
            if approval.by == "owner" || n.allows(task, Action::AcceptPlan) {
                a.progress = Some(progress(
                    "plan_pilot",
                    Some(approval.at),
                    "Approved plan awaits decomposition".into(),
                    "recorded",
                ));
            } else if let Some(condition) = authority(task, n, now, Action::AcceptPlan) {
                a.conditions.push(condition);
            }
            return true;
        }
        return false;
    }
    if plan
        .reviews
        .last()
        .is_none_or(|review| review.verdict.is_none() && review.herdr.is_none())
        && n.review_slots >= n.config.limits.review
    {
        let mut condition = row(
            task,
            now,
            "plan_review_capacity",
            WaitClass::Capacity,
            Responsible::Executor,
            (
                None,
                Some(
                    plan.reviews
                        .last()
                        .map_or(plan.artifact.added, |review| review.at),
                ),
            ),
            "Waiting for a plan-review slot".into(),
        );
        condition
            .resume
            .push(check("review_capacity", &task.id, None, false));
        a.conditions.push(condition);
        return true;
    }
    if let Some(review) = plan.reviews.last() {
        match review.verdict {
            None if review.herdr.is_some() => {
                a.progress = Some(progress(
                    "plan_reviewer",
                    Some(review.at),
                    "Independent plan review is running".into(),
                    "unknown",
                ));
            }
            None => {
                if let Some(condition) = authority(task, n, now, Action::PlanReview) {
                    a.conditions.push(condition);
                } else {
                    a.progress = Some(progress(
                        "plan_pilot",
                        Some(review.at),
                        "Plan review launch is pending".into(),
                        "recorded",
                    ));
                }
            }
            Some(Verdict::Approve) if n.allows(task, Action::AcceptPlan) => {
                a.progress = Some(progress(
                    "plan_pilot",
                    Some(review.at),
                    "Plan acceptance is pending".into(),
                    "recorded",
                ));
            }
            Some(Verdict::Approve | Verdict::Escalate) => {
                let mut condition = row(
                    task,
                    now,
                    "plan_awaiting_owner",
                    WaitClass::OwnerDecision,
                    Responsible::Owner,
                    (Some(format!("{}:{}", plan.id, review.id)), Some(review.at)),
                    "Reviewed plan awaits owner disposition".into(),
                );
                condition
                    .resume
                    .push(check("plan_accepted", &plan.id, None, false));
                condition.next_action.command =
                    Some(format!("corral plan show {} --json", task.id));
                a.conditions.push(condition);
            }
            Some(Verdict::Changes) => {
                if let Some(condition) = authority(task, n, now, Action::Plan) {
                    a.conditions.push(condition);
                } else {
                    a.progress = Some(progress(
                        "plan_pilot",
                        Some(review.at),
                        "Planner revision is pending".into(),
                        "recorded",
                    ));
                }
            }
        }
    } else if let Some(condition) = authority(task, n, now, Action::PlanReview) {
        a.conditions.push(condition);
    } else {
        a.progress = Some(progress(
            "plan_pilot",
            Some(plan.artifact.added),
            "Independent plan review is pending".into(),
            "recorded",
        ));
    }
    true
}

fn aggregate(task: &Task, n: &Neighborhood<'_>, now: Timestamp, a: &mut Assessment) -> bool {
    if !task.tracker {
        return false;
    }
    let pending = task
        .children
        .iter()
        .filter(|child| n.task(child).is_none_or(|task| task.status != Status::Done))
        .collect::<Vec<_>>();
    if !pending.is_empty() {
        let mut condition = wait(
            task,
            now,
            "aggregate_prerequisite",
            WaitClass::Prerequisite,
            Responsible::Executor,
            format!(
                "Waiting for Plan members: {}",
                pending
                    .iter()
                    .map(|id| id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        );
        condition.resume.extend(
            pending
                .into_iter()
                .map(|id| check("task_status", id, Some("done".into()), false)),
        );
        a.conditions.push(condition);
        return true;
    }
    if let Some(attempt) = task.aggregate_attempts.last() {
        if let Some(gate) = attempt.gate.as_ref().filter(|gate| gate.holds_slot()) {
            a.progress = Some(progress(
                "aggregate_gate",
                Some(gate.at),
                "Aggregate gate is running".into(),
                "recorded",
            ));
        } else if let Some(review) = attempt.reviews.last().filter(|review| review.pending()) {
            a.progress = Some(progress(
                "aggregate_reviewer",
                Some(review.requested),
                "Aggregate independent review is pending".into(),
                "recorded",
            ));
        } else {
            let since = attempt
                .reviews
                .last()
                .and_then(|review| review.at)
                .unwrap_or(attempt.started);
            let mut condition = row(
                task,
                now,
                "aggregate_awaiting_owner",
                WaitClass::OwnerDecision,
                Responsible::Owner,
                (Some(attempt.id.clone()), Some(since)),
                format!("Aggregate attempt {} awaits disposition", attempt.id),
            );
            condition
                .resume
                .push(check("aggregate_accepted", &attempt.id, None, false));
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
            WaitClass::OwnerDecision,
            "Plan members are complete; aggregate verification needs authorization".into(),
        );
        condition
            .resume
            .push(check("aggregate_accepted", &task.id, None, false));
        a.conditions.push(condition);
    }
    true
}
