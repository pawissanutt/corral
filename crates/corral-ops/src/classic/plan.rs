//! Plan progress per tracker, cached so that one changed task recomputes only the trackers
//! whose inputs `M(id)` (`tracker::progress_inputs`) contain it.
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use corral_core::{Status, Task};

use crate::feed_delta::FeedDelta;
use crate::{Ctx, PlanProgress};

/// Record areas whose keys name a tracker or one of its members.
const KEYED_AREAS: [&str; 4] = [
    "operating-starts",
    "accepted-proof",
    "plan-checkpoints",
    "plan-cancellations",
];

#[derive(Debug)]
pub(super) struct Tracker {
    pub(super) progress: Result<PlanProgress, String>,
    /// `M(id)`; None when it could not be derived (any task change then recomputes).
    inputs: Option<BTreeSet<String>>,
    project: Option<String>,
    /// A member still waits for admission (Idea/Backlog/Ready), or the progress failed.
    admission_bound: bool,
    pub(super) digest: u64,
}

/// What changed since the cached trackers were computed.
pub(super) struct Changes<'a> {
    pub(super) full: bool,
    pub(super) touched: &'a BTreeSet<String>,
    pub(super) delta: &'a FeedDelta,
    pub(super) git_projects: &'a BTreeSet<String>,
    /// The admission key `(held, reason)`, `slot_usage` or operating policy changed.
    pub(super) admission: bool,
}

/// Trackers in `tasks` with an approved scope, as the board's `plan::tracker::load` selected.
/// A reused tracker keeps its cached `Arc`; a recomputed one gets a new one.
pub(super) fn refresh(
    read: &Ctx,
    tasks: &[&Task],
    previous: &BTreeMap<String, Arc<Tracker>>,
    changes: &Changes<'_>,
) -> anyhow::Result<BTreeMap<String, Arc<Tracker>>> {
    let mut trackers = BTreeMap::new();
    for task in tasks
        .iter()
        .filter(|task| task.tracker && task.approved_scope.is_some())
    {
        if let Some(cached) = previous.get(&task.id)
            && !dirty(read, &task.id, cached, changes)
        {
            let _span = corral_core::timing::span("classic.plan.reuse");
            trackers.insert(task.id.clone(), Arc::clone(cached));
            continue;
        }
        let _span = corral_core::timing::span("classic.plan.tracker");
        let progress = crate::plan_progress_in(read, &read.store, &task.id)
            .map_err(|error| format!("{error:#}"));
        let inputs = crate::tracker::progress_inputs(read, &task.id).ok();
        let admission_bound = match &progress {
            Ok(progress) => progress
                .work
                .iter()
                .any(|work| matches!(work.status, Status::Idea | Status::Backlog | Status::Ready)),
            Err(_) => true,
        };
        let digest = super::digest(&progress)?;
        trackers.insert(
            task.id.clone(),
            Arc::new(Tracker {
                progress,
                inputs,
                project: task.project.clone(),
                admission_bound,
                digest,
            }),
        );
    }
    Ok(trackers)
}

fn dirty(read: &Ctx, id: &str, cached: &Tracker, changes: &Changes<'_>) -> bool {
    if changes.full || changes.delta.event_kinds.contains("approved") {
        return true;
    }
    if cached
        .project
        .as_ref()
        .is_some_and(|project| changes.git_projects.contains(project))
    {
        return true;
    }
    if cached.admission_bound && changes.admission {
        return true;
    }
    let Some(inputs) = &cached.inputs else {
        return !changes.touched.is_empty() || changes.delta.any();
    };
    let named = |key: &str| key.contains(id) || inputs.iter().any(|member| key.contains(member));
    let keyed = KEYED_AREAS.iter().any(|area| {
        changes
            .delta
            .areas
            .get(*area)
            .is_some_and(|keys| keys.iter().any(|key| named(key)))
    });
    if keyed || changes.touched.iter().any(|task| inputs.contains(task)) {
        return true;
    }
    if changes.touched.is_empty() {
        return false;
    }
    // A membership edge from a task outside `M(id)`: re-derive the scopes on this view, as
    // the projection itself does, and recompute when the new inputs name a changed task.
    match crate::tracker::progress_inputs(read, id) {
        Ok(current) => changes.touched.iter().any(|task| current.contains(task)),
        Err(_) => true,
    }
}
