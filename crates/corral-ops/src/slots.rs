use anyhow::Result;
use corral_core::{GateResult, Limits, ReviewWork, Status, Store, Task};
use serde::Serialize;

use crate::Ctx;

/// Component counts are visible even when excluded by an explicit counting switch.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct SlotUsage {
    pub tasks: usize,
    pub reviews: usize,
    pub gates: usize,
    pub reservations: usize,
    /// Open runs parked on another task (or woken and queued): shown, never counted.
    pub parked: usize,
    count_reviews: bool,
    count_gates: bool,
    #[serde(skip)]
    shared_gates: usize,
}

impl SlotUsage {
    pub fn total(&self) -> usize {
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
        Self::from_iter(tasks.iter(), limits)
    }

    fn from_iter<'a>(tasks: impl Iterator<Item = &'a Task>, limits: &Limits) -> Self {
        let mut usage = Self {
            count_reviews: limits.count_reviews,
            count_gates: limits.count_gates,
            ..Self::default()
        };
        for task in tasks {
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
                .filter(|r| r.verdict.is_none())
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

/// Only the current coding run can reuse a slot already held by its worker.
pub(crate) fn gate_shares_dispatch_slot(task: &Task) -> bool {
    task.status.holds_slot() && task.holds_dispatch_slot() && task.current_run().is_some()
}

pub fn slot_usage(ctx: &Ctx) -> Result<SlotUsage> {
    Ok(SlotUsage::from_tasks(
        &ctx.store.list()?,
        &ctx.store.config.limits,
    ))
}

/// Called inside Store::dispatch_update for both coding and review admissions.
pub(crate) fn capacity_reason(store: &Store, task: &Task, credit: usize) -> Result<Option<String>> {
    let tasks = store.list_shared()?;
    let limits = &store.config.limits;
    let used = SlotUsage::from_tasks(&tasks, limits)
        .total()
        .saturating_sub(credit);
    if used >= limits.running {
        return Ok(Some(format!(
            "waiting for a slot ({used}/{} tasks, reviews, gates or reservations)",
            limits.running
        )));
    }
    if let Some(project) = &task.project
        && let Some(cap) = limits.per_project.get(project)
    {
        let used = SlotUsage::from_iter(tasks.iter().filter(|t| t.project == task.project), limits)
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
    if !store
        .effective_operating_policy()
        .is_ok_and(|policy| policy.mode == corral_core::OperatingMode::Autopilot && !policy.paused)
    {
        return Ok(None);
    }
    let tasks = store.list()?;
    let usage = SlotUsage::from_tasks(&tasks, &store.config.limits);
    if usage.reviews + usage.gates >= store.config.limits.review {
        return Ok(None);
    }
    for task in tasks {
        if task.status != Status::Review || task.id == candidate {
            continue;
        }
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
                    None => Some(ReviewWork::Gate),
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
