use anyhow::Result;
use corral_core::{GateResult, Limits, ReviewWork, Status, Store, Task};
use serde::{Deserialize, Serialize};

use crate::Ctx;

/// Component counts are visible even when excluded by an explicit counting switch.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlotUsage {
    /// Independent scheduler counters when opted in; legacy components remain available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub independent: Option<crate::capacity::CapacityUsage>,
    pub tasks: usize,
    /// Durable coding work in progress, independent of slot ownership.
    pub wip: usize,
    pub reviews: usize,
    pub gates: usize,
    pub reservations: usize,
    /// Open runs parked on another task (or woken and queued): shown, never counted.
    pub parked: usize,
    count_reviews: bool,
    count_gates: bool,
    #[serde(default)]
    shared_gates: usize,
}

impl SlotUsage {
    pub fn total(&self) -> usize {
        if let Some(used) = &self.independent {
            return used.agents.total();
        }
        self.tasks
            + self.reservations
            + if self.count_reviews { self.reviews } else { 0 }
            + if self.count_gates {
                self.slot_gates()
            } else {
                0
            }
    }

    /// Live gates that hold a slot of their own: every live gate except a current coding run's,
    /// which shares its worker's slot.
    pub fn slot_gates(&self) -> usize {
        self.gates.saturating_sub(self.shared_gates)
    }

    pub fn from_tasks(tasks: &[Task], limits: &Limits) -> Self {
        let mut usage = Self::from_iter(tasks.iter(), limits);
        if limits.capacity.is_some() {
            let independent = crate::capacity::CapacityUsage::from_tasks(tasks);
            usage.wip = independent.unfinished.total();
            usage.independent = Some(independent);
        }
        usage
    }

    fn from_iter<'a>(tasks: impl Iterator<Item = &'a Task>, limits: &Limits) -> Self {
        let mut usage = Self {
            count_reviews: limits.count_reviews,
            count_gates: limits.count_gates,
            ..Self::default()
        };
        for task in tasks {
            usage.wip += usize::from(crate::wip::task_in_flight(task));
            let sharing = gate_shares_dispatch_slot(task);
            usage.tasks += usize::from(task.status.holds_slot() && task.holds_dispatch_slot());
            usage.parked += usize::from(
                task.status.holds_slot()
                    && task
                        .current_run()
                        .is_some_and(|r| r.releases_slot(&task.id)),
            );
            usage.reservations += usize::from(
                !task.status.holds_slot() && task.launch.as_ref().is_some_and(|l| l.reserved),
            );
            for run in &task.runs {
                usage.shared_gates += usize::from(
                    sharing
                        && task
                            .current_run()
                            .is_some_and(|current| current.id == run.id)
                        && run
                            .gate
                            .as_ref()
                            .is_some_and(corral_core::GateRecord::holds_slot),
                );
                usage.gates += usize::from(
                    run.gate
                        .as_ref()
                        .is_some_and(corral_core::GateRecord::holds_slot),
                );
                usage.reviews += usize::from(
                    run.review
                        .as_ref()
                        .is_some_and(corral_core::ReviewRecord::holds_slot),
                );
            }
            usage.reviews += task
                .plans
                .iter()
                .flat_map(|p| &p.reviews)
                .filter(|r| r.verdict.is_none() && !r.cancelled)
                .count();
            if let Some(work) = task.review_reservation {
                let recorded = task.current_run().is_some_and(|r| match work {
                    ReviewWork::Gate => r
                        .gate
                        .as_ref()
                        .is_some_and(corral_core::GateRecord::holds_slot),
                    ReviewWork::Reviewer => r
                        .review
                        .as_ref()
                        .is_some_and(corral_core::ReviewRecord::holds_slot),
                });
                let counted = match work {
                    ReviewWork::Gate => limits.count_gates && !sharing,
                    ReviewWork::Reviewer => limits.count_reviews,
                };
                usage.reservations += usize::from(counted && !recorded);
            }
        }
        usage
    }
}

/// The soft coding-slot target automatic admission fills up to, never above the hard limit.
pub fn soft_target(limits: &Limits) -> usize {
    if let Some(capacity) = &limits.capacity {
        return capacity.normal_agents();
    }
    limits
        .running_target
        .map_or(limits.running, |target| target.min(limits.running))
}

/// P0 tasks and red-main fixes may take automatic coding slots in the headroom up to the hard limit.
pub(crate) fn uses_headroom(task: &Task) -> bool {
    task.priority == "P0" || corral_core::red_main(task)
}

/// Only the current coding run can reuse a slot already held by its worker.
pub(crate) fn gate_shares_dispatch_slot(task: &Task) -> bool {
    task.status.holds_slot() && task.holds_dispatch_slot() && task.current_run().is_some()
}

pub fn slot_usage(ctx: &Ctx) -> Result<SlotUsage> {
    Ok(SlotUsage::from_heads(
        &crate::pm_next::heads::open(&ctx.store)?,
        &ctx.store.config.limits,
    ))
}

/// Called inside Store::dispatch_update for both coding and review admissions.
pub(crate) fn capacity_reason<T: corral_core::TaskFieldsRef + ?Sized>(
    store: &Store,
    task: &T,
    credit: usize,
) -> Result<Option<String>> {
    if store.config.limits.capacity.is_some() {
        return crate::capacity::admission::agent_reason(store, task, credit);
    }
    let tasks = crate::pm_next::heads::open(store)?;
    let limits = &store.config.limits;
    let used = SlotUsage::from_heads(&tasks, limits)
        .total()
        .saturating_sub(credit);
    if used >= limits.running {
        return Ok(Some(format!(
            "waiting for a slot ({used}/{} tasks, reviews, gates or reservations)",
            limits.running
        )));
    }
    if let Some(project) = task.project()
        && let Some(cap) = limits.per_project.get(project)
    {
        let project_tasks = tasks
            .iter()
            .filter(|t| t.project == *task.project())
            .cloned()
            .collect::<Vec<_>>();
        let used = SlotUsage::from_heads(&project_tasks, limits)
            .total()
            .saturating_sub(credit);
        if used >= *cap {
            return Ok(Some(format!(
                "waiting for a slot for project {project} ({used}/{cap})"
            )));
        }
    }
    Ok(None)
}

/// A waiting finishing step gets first refusal, but a saturated project cannot block others.
pub(crate) fn finishing_first(store: &Store, candidate: &str) -> Result<Option<String>> {
    let _span = corral_core::timing::span("queue.finishing_first");
    if !store
        .effective_operating_policy()
        .is_ok_and(|policy| policy.mode == corral_core::OperatingMode::Autopilot && !policy.paused)
    {
        return Ok(None);
    }
    let heads = crate::pm_next::heads::open(store)?;
    let usage = SlotUsage::from_heads(&heads, &store.config.limits);
    if store.config.limits.capacity.is_none()
        && usage.reviews + usage.gates >= store.config.limits.review
    {
        return Ok(None);
    }
    // Only unheld Review heads can wait on finishing work; the shell copies these head
    // fields, so skip every other head before building its compatibility task.
    for head in heads
        .iter()
        .filter(|head| head.hold.is_none() && head.status == Status::Review && head.id != candidate)
    {
        let task = head.to_shell_task()?;
        if crate::operating::require(store, &task, corral_core::OperatingActionKind::Gate).is_err()
        {
            continue;
        }
        let policy = store.config.task_review_policy(&task);
        let Some(run) = task
            .current_run()
            .filter(|r| r.outcome == Some(corral_core::RunOutcome::Done))
        else {
            continue;
        };
        if task.review_reservation.is_some()
            || run
                .gate
                .as_ref()
                .is_some_and(|g| g.result == GateResult::Running)
            || run.review.as_ref().is_some_and(|r| r.verdict.is_none())
        {
            continue;
        }
        // Determine immediately eligible finishing work as well as persisted waits.
        // Otherwise another sync/manual claim could steal the gap between gate and review.
        let work = task.review_wait.or_else(|| {
            use corral_core::AutoStage;
            match run.autopilot.as_ref().map(|s| s.stage) {
                None | Some(AutoStage::Changes) => {
                    if !policy.gate.is_empty()
                        && run
                            .gate
                            .as_ref()
                            .is_none_or(|g| g.result != GateResult::Pass)
                    {
                        Some(ReviewWork::Gate)
                    } else if policy.reviewer != corral_core::Reviewer::None
                        && run.review.as_ref().is_none_or(|r| r.verdict.is_none())
                    {
                        Some(ReviewWork::Reviewer)
                    } else {
                        None
                    }
                }
                Some(AutoStage::Gating) => match run.gate.as_ref().map(|g| g.result) {
                    None | Some(GateResult::Cancelled) => Some(ReviewWork::Gate),
                    Some(GateResult::Pass)
                        if policy.reviewer != corral_core::Reviewer::None
                            && run.review.is_none() =>
                    {
                        Some(ReviewWork::Reviewer)
                    }
                    Some(
                        GateResult::Running
                        | GateResult::Pass
                        | GateResult::Fail
                        | GateResult::Stale,
                    ) => None,
                },
                Some(AutoStage::Reviewing) if run.review.is_none() => Some(ReviewWork::Reviewer),
                Some(
                    AutoStage::Reviewing
                    | AutoStage::Landing
                    | AutoStage::Blocked
                    | AutoStage::Reviewed
                    | AutoStage::Escalated
                    | AutoStage::Done,
                ) => None,
            }
        });
        match work {
            Some(ReviewWork::Gate) if policy.gate.is_empty() => continue,
            Some(ReviewWork::Reviewer) if policy.reviewer == corral_core::Reviewer::None => {
                continue;
            }
            Some(ReviewWork::Gate | ReviewWork::Reviewer) => {}
            None => continue,
        }
        // Independent gates cannot compete with a new agent claim.
        if store.config.limits.capacity.is_some() && work == Some(ReviewWork::Gate) {
            continue;
        }
        if let Some(kind) = work
            && crate::review_budget::retained_preparation(store, &task.id, &run.id, kind)?
        {
            continue;
        }
        if capacity_reason(store, &task, 0)?.is_none() {
            return Ok(Some(format!("waiting for finishing work on {}", task.id)));
        }
    }
    Ok(None)
}

impl SlotUsage {
    /// Slot accounting over bounded current containers and folded history counts.
    pub fn from_heads(heads: &[corral_core::TaskHead], limits: &Limits) -> Self {
        let mut usage = Self {
            count_reviews: limits.count_reviews,
            count_gates: limits.count_gates,
            ..Self::default()
        };
        usage.wip = crate::wip::count(heads);
        for head in heads {
            let run = head.current_run.as_ref();
            let reserved = head.launch.as_ref().is_some_and(|l| l.reserved);
            usage.tasks += usize::from(head.status.holds_slot() && head.holds_dispatch_slot());
            usage.parked += usize::from(
                head.status.holds_slot() && run.is_some_and(|r| r.releases_slot(&head.id)),
            );
            let sharing = head.status.holds_slot() && head.holds_dispatch_slot() && run.is_some();
            usage.reservations += usize::from(!head.status.holds_slot() && reserved);
            let gate = run
                .and_then(|r| r.gate.as_ref())
                .is_some_and(corral_core::GateRecord::holds_slot);
            let review = run
                .and_then(|r| r.review.as_ref())
                .is_some_and(corral_core::ReviewRecord::holds_slot);
            usage.gates +=
                usize::try_from(head.summary.held_gates).unwrap_or(usize::MAX) + usize::from(gate);
            usage.shared_gates += usize::from(sharing && gate);
            usage.reviews += usize::try_from(
                head.summary.held_reviews
                    + head.summary.pending_plan_reviews
                    + head
                        .last_plan
                        .as_ref()
                        .map_or(0, |p| p.facts.pending_children),
            )
            .unwrap_or(usize::MAX)
                + usize::from(review);
            if let Some(work) = head.review_reservation {
                let (recorded, counted) = match work {
                    ReviewWork::Gate => (gate, limits.count_gates && !sharing),
                    ReviewWork::Reviewer => (review, limits.count_reviews),
                };
                usage.reservations += usize::from(counted && !recorded);
            }
        }
        if limits.capacity.is_some() {
            let independent = crate::capacity::CapacityUsage::from_heads(heads);
            usage.wip = independent.unfinished.total();
            usage.independent = Some(independent);
        }
        usage
    }
}
