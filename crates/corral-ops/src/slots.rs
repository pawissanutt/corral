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
    count_reviews: bool,
    count_gates: bool,
}

impl SlotUsage {
    pub fn total(&self) -> usize {
        self.tasks
            + self.reservations
            + if self.count_reviews { self.reviews } else { 0 }
            + if self.count_gates { self.gates } else { 0 }
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
            usage.tasks += usize::from(task.status.holds_slot());
            usage.reservations += usize::from(
                !task.status.holds_slot() && task.launch.as_ref().is_some_and(|l| l.reserved),
            );
            for run in &task.runs {
                usage.gates += usize::from(
                    run.gate
                        .as_ref()
                        .is_some_and(corral_core::GateRecord::holds_slot),
                );
                usage.reviews +=
                    usize::from(run.review.as_ref().is_some_and(|r| r.verdict.is_none()));
            }
            if let Some(work) = task.review_reservation {
                let recorded = task.current_run().is_some_and(|r| match work {
                    ReviewWork::Gate => r
                        .gate
                        .as_ref()
                        .is_some_and(corral_core::GateRecord::holds_slot),
                    ReviewWork::Reviewer => r.review.as_ref().is_some_and(|r| r.verdict.is_none()),
                });
                let counted = match work {
                    ReviewWork::Gate => limits.count_gates,
                    ReviewWork::Reviewer => limits.count_reviews,
                };
                usage.reservations += usize::from(counted && !recorded);
            }
        }
        usage
    }
}

pub fn slot_usage(ctx: &Ctx) -> Result<SlotUsage> {
    Ok(SlotUsage::from_tasks(
        &ctx.store.list()?,
        &ctx.store.config.limits,
    ))
}

/// Called inside Store::dispatch_update for both coding and review admissions.
pub(crate) fn capacity_reason(store: &Store, task: &Task, credit: usize) -> Result<Option<String>> {
    let tasks = store.list()?;
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
    if !store.config.autopilot_enabled() {
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
        let policy = store
            .config
            .review_policy(task.project.as_deref(), task.review);
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
                    Some(GateResult::Running | GateResult::Pass | GateResult::Fail) => None,
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
        if capacity_reason(store, &task, 0)?.is_none() {
            return Ok(Some(format!("waiting for finishing work on {}", task.id)));
        }
    }
    Ok(None)
}
