//! `corral plan add-member`: adds a task whose runs never landed to a Plan's required members, for
//! example a fix after a failed `corral plan verify`. Plan verify and accept then require it like
//! any other member; no gate tier changes. A started member's next run starts from the Plan's
//! delivery branch.
use anyhow::{Result, ensure};
use corral_core::{Status, Store, Task};
use serde::Serialize;

use crate::Ctx;

#[cfg(test)]
#[path = "member_add_tests.rs"]
mod tests;

#[derive(Debug, Clone, Serialize)]
pub struct PlanMemberAdded {
    pub tracker: String,
    pub member: String,
    pub children: Vec<String>,
}

/// Adds `member` to `tracker`.
///
/// # Errors
/// Refuses a finished or accepted Plan, a member that landed, is live or is owned elsewhere, and
/// any existing member with a run in flight, because that run is bound to the current membership.
pub fn plan_add_member(ctx: &Ctx, tracker: &str, member: &str) -> Result<PlanMemberAdded> {
    let saved = ctx
        .store
        .add_plan_member(tracker, member, |store, plan, task| {
            check_open_plan(store, plan)?;
            super::member_move::check_unlanded(store, task)
        })?;
    Ok(PlanMemberAdded {
        tracker: saved.id,
        member: corral_core::Store::normalize_id(member)?,
        children: saved.children,
    })
}

/// A Plan whose membership may change: not finished or accepted, and no member run in flight,
/// because that run is bound to the current membership.
pub(super) fn check_open_plan(store: &Store, plan: &Task) -> Result<()> {
    ensure!(
        !matches!(plan.status, Status::Done | Status::Dropped)
            && plan.aggregate_acceptance().is_none(),
        "{} is {} or already accepted",
        plan.id,
        plan.status.label()
    );
    for id in &plan.children {
        let child = store.get(id)?;
        ensure!(
            child.launch.is_none()
                && !matches!(
                    child.status,
                    Status::Running | Status::NeedsMe | Status::Review
                ),
            "{id} is {}; finish its run before changing {}'s membership",
            child.status.label(),
            plan.id
        );
    }
    Ok(())
}
