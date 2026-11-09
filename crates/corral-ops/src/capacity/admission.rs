//! Independent-pool admission helpers; callers retain their existing claim/launch locks.
use anyhow::Result;
use corral_core::{CapacityLane, CapacityPool, Store, TaskFieldsRef, TaskRef};

use super::{CapacityUsage, unfinished};

fn credit(pool: &mut CapacityPool, lane: CapacityLane, count: usize) {
    let used = match lane {
        CapacityLane::Normal => &mut pool.normal,
        CapacityLane::Adhoc => &mut pool.adhoc,
    };
    *used = used.saturating_sub(count);
}

pub(crate) fn agent_reason<T: TaskFieldsRef + ?Sized>(
    store: &Store,
    task: &T,
    replacing: usize,
) -> Result<Option<String>> {
    let Some(limits) = &store.config.limits.capacity else {
        return Ok(None);
    };
    store.require_capacity_schema()?;
    let heads = crate::pm_next::heads::open(store)?;
    let mut used = CapacityUsage::from_heads(&heads);
    let lane = *task.capacity_lane();
    let owned = heads
        .iter()
        .find(|h| h.id == *task.id())
        .map(|h| {
            CapacityUsage::from_heads(std::slice::from_ref(h))
                .agents
                .total()
        })
        .unwrap_or(0);
    let replacing = replacing.min(owned);
    credit(&mut used.agents, lane, replacing);
    if let Some(reason) = used.agent_reason(limits, lane) {
        return Ok(Some(reason));
    }
    if let Some(project) = task.project()
        && let Some(cap) = store.config.limits.per_project.get(project)
    {
        let project_heads: Vec<_> = heads
            .iter()
            .filter(|h| h.project.as_ref() == Some(project))
            .cloned()
            .collect();
        let total = CapacityUsage::from_heads(&project_heads)
            .agents
            .total()
            .saturating_sub(replacing);
        if total >= *cap {
            return Ok(Some(format!(
                "waiting: agents full for project {project} ({total}/{cap})"
            )));
        }
    }
    Ok(None)
}

pub(crate) fn gate_reason(store: &Store, replacing: usize) -> Result<Option<String>> {
    let Some(limits) = &store.config.limits.capacity else {
        return Ok(None);
    };
    store.require_capacity_schema()?;
    let mut used = CapacityUsage::from_heads(&crate::pm_next::heads::open(store)?);
    used.gates = used.gates.saturating_sub(replacing);
    Ok(used.gate_reason(limits))
}

pub(crate) fn review_reason<T: TaskRef + ?Sized>(
    store: &Store,
    task: &T,
    kind: corral_core::ReviewWork,
) -> Result<Option<String>> {
    let run = task.current_run_head()?;
    match kind {
        corral_core::ReviewWork::Gate => {
            let replacing = run.as_ref().is_some_and(|r| {
                r.gate
                    .as_ref()
                    .is_some_and(|g| g.holds_slot() && g.identity.is_none())
            });
            gate_reason(store, usize::from(replacing))
        }
        corral_core::ReviewWork::Reviewer => {
            let replacing = run
                .as_ref()
                .is_some_and(|r| r.review.as_ref().is_some_and(|r| r.holds_slot()));
            agent_reason(store, task, usize::from(replacing))
        }
    }
}

pub(crate) fn unfinished_reason<T: TaskRef + ?Sized>(
    store: &Store,
    task: &T,
) -> Result<Option<String>> {
    let Some(limits) = &store.config.limits.capacity else {
        return Ok(None);
    };
    store.require_capacity_schema()?;
    let heads = crate::pm_next::heads::open(store)?;
    // An existing task retains its position across replanning, retries, review and resume.
    if heads
        .iter()
        .find(|h| h.id == *task.id())
        .is_some_and(unfinished)
    {
        return Ok(None);
    }
    Ok(CapacityUsage::from_heads(&heads).unfinished_reason(limits, *task.capacity_lane()))
}
