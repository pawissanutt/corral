//! `corral plan move-member`: the owner moves an unstarted member, for example one deliberately
//! held in Backlog, out of a tracker Plan into another, so the source can verify and deliver
//! without it. The task keeps its ID, holds, dependencies and criteria; only its Plan changes.
//! Both Plans' operating contracts change, so earlier aggregate attempts and grants go stale.
use std::path::Path;

use anyhow::{Context, Result, ensure};
use corral_core::{FULL_TIER, PlanDeliveryTarget, ScopeGate, Store, Task};
use serde::Serialize;

use super::member_add::{check_open_plan, check_unstarted};
use super::member_gate::{OwnerEntry, read_gate};
use crate::Ctx;

/// The owner's gate for the moved member in the target Plan, recorded as its member gate.
pub struct MoveGate<'a> {
    pub file: &'a Path,
    pub reason: &'a str,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlanMemberMoved {
    pub member: String,
    pub from: String,
    pub to: String,
    pub from_children: Vec<String>,
    pub to_children: Vec<String>,
    /// `member-gate:<to>@<revision>` when the target records an owner gate for the member.
    pub member_gate: Option<String>,
}

/// Moves `member` from tracker `from` to tracker `to` as the owner of this OS session.
///
/// # Errors
/// Refuses PM, agent and run-worktree entries, and every move [`plan_move_member_from`] refuses.
pub fn plan_move_member(
    ctx: &Ctx,
    from: &str,
    member: &str,
    to: &str,
    gate: Option<&MoveGate>,
) -> Result<PlanMemberMoved> {
    plan_move_member_from(ctx, from, member, to, gate, &OwnerEntry::from_env()?)
}

/// [`plan_move_member`] with an explicit entry, for tests that run under an agent's environment.
///
/// # Errors
/// Refuses a non-owner entry, a finished or accepted source or target, a member that is started,
/// not owned by the source or already owned by the target, any member of either Plan with a run
/// in flight, because that run is bound to the current membership, and a move that would leave
/// the member without a resolvable gate in a designed integration target.
#[doc(hidden)]
pub fn plan_move_member_from(
    ctx: &Ctx,
    from: &str,
    member: &str,
    to: &str,
    gate: Option<&MoveGate>,
    entry: &OwnerEntry,
) -> Result<PlanMemberMoved> {
    crate::automation_controls::require_owner_entry(&ctx.store, &entry.cwd, entry.agent, entry.pm)
        .context("corral plan move-member is owner-only")?;
    let owner_gate = match gate {
        Some(gate) => {
            ensure!(
                !gate.reason.trim().is_empty(),
                "a move-member gate needs a reason"
            );
            Some((read_gate(ctx, gate.file)?, gate.reason))
        }
        None => None,
    };
    let (from, to) = ctx.store.move_plan_member(
        from,
        member,
        to,
        owner_gate.as_ref().map(|(gate, reason)| (gate, *reason)),
        |store, from, to, task| {
            check_open_plan(store, from)?;
            check_open_plan(store, to)?;
            check_unstarted(task)?;
            let gate = owner_gate.as_ref().map(|(gate, _)| gate);
            check_target_gate(store, from, to, task, gate)
        },
    )?;
    let member = corral_core::Store::normalize_id(member)?;
    Ok(PlanMemberMoved {
        member_gate: to
            .member_gates
            .get(&member)
            .map(|record| format!("member-gate:{}@{}", to.id, record.revision)),
        member,
        from: from.id,
        to: to.id,
        from_children: from.children,
        to_children: to.children,
    })
}

/// A designed integration Plan's members take their gates from its publication journal, which
/// never bound a moved member, so the target needs an owner gate record for it: the one the
/// source carries or the owner's `gate`. Other targets resolve policy or default gates.
fn check_target_gate(
    store: &Store,
    from: &Task,
    to: &Task,
    task: &Task,
    gate: Option<&ScopeGate>,
) -> Result<()> {
    let scope = to.approved_scope.as_ref().context("Plan scope missing")?;
    let integration = matches!(
        scope.delivery_target,
        PlanDeliveryTarget::Integration { .. }
    );
    if let Some(gate) = gate {
        ensure!(
            integration,
            "{} delivers to main; member gates apply only to integration Plans",
            to.id
        );
        let tiers = store
            .config
            .review_policy(to.project.as_deref(), None)
            .gate_tiers;
        ensure!(
            gate.tier == FULL_TIER || tiers.contains_key(&gate.tier),
            "gate tier {:?} is not defined for project {}",
            gate.tier,
            to.project.as_deref().unwrap_or("(none)")
        );
    }
    if let Some(design) = scope.approved_design.as_ref().filter(|_| integration) {
        ensure!(
            gate.is_some() || from.member_gates.contains_key(&task.id),
            "{} would have no gate in {}: its member gates come from the publication of {design}, \
             which never bound {}; rerun with --gate-file <gate.yaml> --reason <text> to record \
             the owner's gate for it",
            task.id,
            to.id,
            task.id
        );
    }
    Ok(())
}
