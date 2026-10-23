//! `corral plan add-member`: the owner adds an unstarted task to a Plan's required members, for
//! example a fix after a failed `corral plan verify`. Plan verify and accept then require it like
//! any other member; no gate tier changes.
use anyhow::{Context, Result, ensure};
use corral_core::Status;
use serde::Serialize;

use super::member_gate::OwnerEntry;
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

/// Adds `member` to `tracker` as the owner of this OS session.
///
/// # Errors
/// Refuses PM, agent and run-worktree entries, and every addition [`plan_add_member_from`]
/// refuses.
pub fn plan_add_member(ctx: &Ctx, tracker: &str, member: &str) -> Result<PlanMemberAdded> {
    plan_add_member_from(ctx, tracker, member, &OwnerEntry::from_env()?)
}

/// [`plan_add_member`] with an explicit entry, for tests that run under an agent's environment.
///
/// # Errors
/// Refuses a non-owner entry, a finished or accepted Plan, a member that is started or owned
/// elsewhere, and any existing member with a run in flight, because that run is bound to the
/// current membership.
#[doc(hidden)]
pub fn plan_add_member_from(
    ctx: &Ctx,
    tracker: &str,
    member: &str,
    entry: &OwnerEntry,
) -> Result<PlanMemberAdded> {
    crate::automation_controls::require_owner_entry(&ctx.store, &entry.cwd, entry.agent, entry.pm)
        .context("corral plan add-member is owner-only")?;
    let saved = ctx
        .store
        .add_plan_member(tracker, member, |store, plan, task| {
            ensure!(
                !matches!(plan.status, Status::Done | Status::Dropped)
                    && plan.aggregate_acceptance().is_none(),
                "{} is {} or already accepted",
                plan.id,
                plan.status.label()
            );
            ensure!(
                matches!(task.status, Status::Idea | Status::Backlog | Status::Ready)
                    && task.runs.is_empty()
                    && task.launch.is_none(),
                "{} must be unstarted (Idea, Backlog or Ready with no runs)",
                task.id
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
        })?;
    Ok(PlanMemberAdded {
        tracker: saved.id,
        member: corral_core::Store::normalize_id(member)?,
        children: saved.children,
    })
}
