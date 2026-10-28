//! `corral plan move-member`: the owner moves an unstarted member, for example one deliberately
//! held in Backlog, out of a tracker Plan into another, so the source can verify and deliver
//! without it. The task keeps its ID, holds, dependencies and criteria; only its Plan changes.
//! Both Plans' operating contracts change, so earlier aggregate attempts and grants go stale.
use anyhow::{Context, Result};
use serde::Serialize;

use super::member_add::{check_open_plan, check_unstarted};
use super::member_gate::OwnerEntry;
use crate::Ctx;

#[derive(Debug, Clone, Serialize)]
pub struct PlanMemberMoved {
    pub member: String,
    pub from: String,
    pub to: String,
    pub from_children: Vec<String>,
    pub to_children: Vec<String>,
}

/// Moves `member` from tracker `from` to tracker `to` as the owner of this OS session.
///
/// # Errors
/// Refuses PM, agent and run-worktree entries, and every move [`plan_move_member_from`] refuses.
pub fn plan_move_member(ctx: &Ctx, from: &str, member: &str, to: &str) -> Result<PlanMemberMoved> {
    plan_move_member_from(ctx, from, member, to, &OwnerEntry::from_env()?)
}

/// [`plan_move_member`] with an explicit entry, for tests that run under an agent's environment.
///
/// # Errors
/// Refuses a non-owner entry, a finished or accepted source or target, a member that is started,
/// not owned by the source or already owned by the target, and any member of either Plan with a
/// run in flight, because that run is bound to the current membership.
#[doc(hidden)]
pub fn plan_move_member_from(
    ctx: &Ctx,
    from: &str,
    member: &str,
    to: &str,
    entry: &OwnerEntry,
) -> Result<PlanMemberMoved> {
    crate::automation_controls::require_owner_entry(&ctx.store, &entry.cwd, entry.agent, entry.pm)
        .context("corral plan move-member is owner-only")?;
    let (from, to) = ctx
        .store
        .move_plan_member(from, member, to, |store, from, to, task| {
            check_open_plan(store, from)?;
            check_open_plan(store, to)?;
            check_unstarted(task)
        })?;
    Ok(PlanMemberMoved {
        member: corral_core::Store::normalize_id(member)?,
        from: from.id,
        to: to.id,
        from_children: from.children,
        to_children: to.children,
    })
}
