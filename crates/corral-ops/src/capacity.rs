//! Independent agent, gate and unfinished-work accounting over bounded task heads.
//! Reservations count before execution; scheduling lanes never authorize an operation.
use corral_core::{CapacityLane, CapacityLimits, CapacityPool, ReviewWork, Status, TaskHead};
use serde::Serialize;

pub(crate) mod admission;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct CapacityUsage {
    pub agents: CapacityPool,
    pub unfinished: CapacityPool,
    pub gates: usize,
}

impl CapacityUsage {
    pub fn merge(&mut self, other: &Self) {
        for lane in [CapacityLane::Normal, CapacityLane::Adhoc] {
            let (agents, unfinished) = match lane {
                CapacityLane::Normal => (other.agents.normal, other.unfinished.normal),
                CapacityLane::Adhoc => (other.agents.adhoc, other.unfinished.adhoc),
            };
            self.agents.add(lane, agents);
            self.unfinished.add(lane, unfinished);
        }
        self.gates = self.gates.saturating_add(other.gates);
    }

    /// Cold complete-snapshot counterpart; hot admission always uses maintained heads.
    pub fn from_tasks(tasks: &[corral_core::Task]) -> Self {
        let mut used = Self::default();
        for task in tasks {
            let lane = task.capacity_lane;
            let run = task.current_run();
            let launch = task
                .launch
                .as_ref()
                .is_some_and(|l| l.reserved || l.stage == corral_core::LaunchStage::Submitting);
            let planner = task.plan_runs.last().is_some_and(|p| p.holds_slot());
            let worker = !planner
                && task.status.holds_slot()
                && task.holds_dispatch_slot()
                && run.is_none_or(|r| r.outcome.is_none());
            used.agents
                .add(lane, usize::from(planner || worker || launch));
            let started = !task.tracker
                && !matches!(task.status, Status::Done | Status::Dropped)
                && (launch || corral_core::capacity::task_committed(task));
            used.unfinished.add(lane, usize::from(started));
            used.gates = used
                .gates
                .saturating_add(usize::from(task.capacity_checkpoint.is_some()));
            for run in &task.runs {
                used.gates = used.gates.saturating_add(usize::from(
                    run.gate.as_ref().is_some_and(|g| g.holds_slot()),
                ));
                used.agents.add(
                    lane,
                    usize::from(run.review.as_ref().is_some_and(|r| r.holds_slot())),
                );
            }
            for review in task.plans.iter().flat_map(|p| &p.reviews) {
                used.agents.add(
                    lane,
                    usize::from(review.verdict.is_none() && !review.cancelled),
                );
            }
            for aggregate in &task.aggregate_attempts {
                used.gates = used.gates.saturating_add(usize::from(
                    aggregate.gate.as_ref().is_some_and(|g| g.holds_slot()),
                ));
                for review in &aggregate.reviews {
                    used.agents.add(lane, usize::from(review.pending()));
                }
            }
            match task.review_reservation {
                Some(ReviewWork::Gate)
                    if run
                        .and_then(|r| r.gate.as_ref())
                        .is_none_or(|g| !g.holds_slot()) =>
                {
                    used.gates = used.gates.saturating_add(1);
                }
                Some(ReviewWork::Reviewer)
                    if run
                        .and_then(|r| r.review.as_ref())
                        .is_none_or(|r| !r.holds_slot()) =>
                {
                    used.agents.add(lane, 1)
                }
                Some(ReviewWork::Gate | ReviewWork::Reviewer) | None => {}
            }
        }
        used
    }

    pub fn from_heads(heads: &[TaskHead]) -> Self {
        let mut used = Self::default();
        for head in heads {
            let lane = head.capacity_lane;
            used.gates = used
                .gates
                .saturating_add(usize::from(head.capacity_checkpoint.is_some()));
            let run = head.current_run.as_ref();
            let launch = head
                .launch
                .as_ref()
                .is_some_and(|l| l.reserved || l.stage == corral_core::LaunchStage::Submitting);
            let planner = head.last_plan_run.as_ref().is_some_and(|p| p.holds_slot());
            let worker = !planner
                && head.status.holds_slot()
                && head.holds_dispatch_slot()
                && run.is_none_or(|r| r.outcome.is_none());
            // A planner/worker claim and its launch reservation describe the same execution.
            used.agents
                .add(lane, usize::from(planner || worker || launch));
            used.unfinished.add(lane, usize::from(unfinished(head)));
            let gate = run
                .and_then(|r| r.gate.as_ref())
                .is_some_and(|g| g.holds_slot());
            let review = run
                .and_then(|r| r.review.as_ref())
                .is_some_and(|r| r.holds_slot());
            used.gates = used
                .gates
                .saturating_add(usize::try_from(head.summary.held_gates).unwrap_or(usize::MAX))
                .saturating_add(usize::from(gate));
            used.gates = used.gates.saturating_add(
                usize::try_from(head.summary.held_aggregate_gates).unwrap_or(usize::MAX),
            );
            let reviews = head
                .summary
                .held_reviews
                .saturating_add(head.summary.pending_aggregate_reviews)
                .saturating_add(head.summary.pending_plan_reviews)
                .saturating_add(
                    head.last_plan
                        .as_ref()
                        .map_or(0, |p| p.facts.pending_children),
                );
            used.agents
                .add(lane, usize::try_from(reviews).unwrap_or(usize::MAX));
            used.agents.add(lane, usize::from(review));
            if let Some(aggregate) = &head.last_aggregate_attempt {
                used.gates = used.gates.saturating_add(usize::from(
                    aggregate.gate.as_ref().is_some_and(|g| g.holds_slot()),
                ));
                used.agents.add(
                    lane,
                    usize::try_from(aggregate.facts.pending_children).unwrap_or(usize::MAX),
                );
            }
            match head.review_reservation {
                Some(ReviewWork::Gate) if !gate => used.gates = used.gates.saturating_add(1),
                Some(ReviewWork::Reviewer) if !review => used.agents.add(lane, 1),
                Some(ReviewWork::Gate | ReviewWork::Reviewer) | None => {}
            }
        }
        used
    }

    pub fn agent_reason(&self, limits: &CapacityLimits, lane: CapacityLane) -> Option<String> {
        pool_reason(
            "agents",
            self.agents,
            limits.agents,
            limits.adhoc_reserve,
            lane,
        )
    }

    pub fn unfinished_reason(&self, limits: &CapacityLimits, lane: CapacityLane) -> Option<String> {
        pool_reason(
            "unfinished tasks",
            self.unfinished,
            limits.unfinished,
            limits.adhoc_reserve,
            lane,
        )
    }

    pub fn gate_reason(&self, limits: &CapacityLimits) -> Option<String> {
        (self.gates >= limits.gates)
            .then(|| format!("waiting: gates full ({}/{})", self.gates, limits.gates))
    }
}

fn pool_reason(
    name: &str,
    used: CapacityPool,
    hard: usize,
    reserve: usize,
    lane: CapacityLane,
) -> Option<String> {
    if used.total() >= hard {
        Some(format!("waiting: {name} full ({}/{hard})", used.total()))
    } else if !used.permits(lane, hard, reserve) {
        Some(format!(
            "waiting: normal {name} allowance full ({}/{}); {reserve} reserved for ad-hoc work",
            used.normal,
            hard.saturating_sub(reserve),
        ))
    } else {
        None
    }
}

/// Task identity, not run identity: checks, pauses and repairs retain one unfinished position.
pub(crate) fn unfinished(head: &TaskHead) -> bool {
    if head.tracker || matches!(head.status, Status::Done | Status::Dropped) {
        return false;
    }
    head.launch
        .as_ref()
        .is_some_and(|l| l.reserved || l.stage == corral_core::LaunchStage::Submitting)
        || corral_core::capacity::head_committed(head)
}

/// Explicit owner designation before dispatch/enqueue. It changes scheduling, not authority.
/// Existing lanes are retained by all ordinary launch/retry/review paths.
pub fn designate_adhoc(ctx: &crate::Ctx, id: &str) -> anyhow::Result<()> {
    crate::require_automation_owner(&ctx.store)?;
    ctx.store.check_runtime()?;
    anyhow::ensure!(
        ctx.store.config.limits.capacity.is_some(),
        "ad-hoc reserve requires independent capacity limits"
    );
    ctx.store.require_capacity_schema()?;
    let id = corral_core::Store::normalize_id(id)?;
    let _launch = ctx
        .store
        .try_lock(&format!("launch-{id}"))?
        .ok_or_else(|| anyhow::anyhow!("dispatch is already in progress for {id}"))?;
    ctx.store.dispatch_update_head(&id, |store, head| {
        if head.capacity_lane == CapacityLane::Adhoc {
            return Ok(());
        }
        anyhow::ensure!(
            !head.tracker,
            "a tracker cannot consume ad-hoc execution capacity"
        );
        anyhow::ensure!(
            matches!(head.status, Status::Ready | Status::Backlog),
            "designate ad-hoc work before starting it"
        );
        anyhow::ensure!(
            !unfinished(head.head()),
            "started work keeps its original scheduling lane"
        );
        head.capacity_lane = CapacityLane::Adhoc;
        store.log_event(
            Some(&id),
            "capacity_lane_selected",
            serde_json::json!({"lane":"adhoc"}),
        )
    })?;
    Ok(())
}

#[cfg(test)]
#[path = "capacity_tests.rs"]
mod tests;
