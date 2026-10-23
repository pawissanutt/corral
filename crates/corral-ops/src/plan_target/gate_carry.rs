//! Plan-target gate carry: after corral's own recorded, clean, equal-patch replay of an approved
//! member onto a sibling's landing, the member's receipt is rebound to the replayed head at the
//! same member binding, together with its criterion evidence, instead of running a new gate.
//! Only a run bound to an owned Plan target can reach the carry; main-bound runs never do.
use anyhow::{Context, Result};
use corral_core::{
    CandidateIdentity, CarryMethod, EvidenceCarry, GateCarry, GateRecord, GateResult,
    RunPlanTarget, Task,
};
use serde_json::json;

use crate::completion::evidence_carry::{CarryRefused, CarrySource, establish};
use crate::gate_tier::{DestinationKind, GatePhase};
use crate::{Ctx, verification};

/// The only fields corral's recorded replay may change in the re-derived receipt identity.
const REPLAYED: [&str; 3] = ["head", "tree", "integration_base"];

/// A validated rebinding of the current member receipt onto the replayed head.
struct Rebind {
    old: CandidateIdentity,
    new: CandidateIdentity,
    carry: GateCarry,
    evidence_carry: Option<EvidenceCarry>,
    /// The provenance event is already durable (a retry after a crash before the save).
    logged: bool,
}

/// Rebinds the member's passing receipt and its evidence to the replayed head in one update,
/// after logging `gate_carried`. False when the run is not a tiered Plan member, nothing was
/// replayed, or any check fails; the caller then gates the replayed head as today.
pub(crate) fn gate_carry(ctx: &Ctx, id: &str) -> Result<bool> {
    let mut carried = None;
    ctx.store.try_update(id, |task| {
        let run = task.current_run().context("missing run for gate carry")?;
        // The owned target binding is the only entry: a main-bound run has none.
        let Some(target) = run.plan_target.clone() else {
            return Ok(false);
        };
        let Some(rebind) = check(ctx, task, &target)? else {
            return Ok(false);
        };
        let run = task.current_run_mut().context("run disappeared")?;
        let gate = run.gate.as_mut().context("gate disappeared")?;
        let tier = gate.tier.as_mut().context("member receipt has no tier")?;
        let detail = json!({"run": run.id, "tier": tier.tier,
            "old_sha": rebind.old.head, "new_sha": rebind.new.head,
            "old_identity": rebind.old, "new_identity": rebind.new,
            "gate_carry": rebind.carry, "evidence_carry": rebind.evidence_carry,
            "target": target.branch});
        tier.carried = Some(rebind.carry);
        gate.sha = rebind.new.head.clone();
        gate.identity = Some(rebind.new);
        run.completion.evidence_carry = rebind.evidence_carry.map(Box::new);
        // Persist provenance before the rebound receipt can authorize anything.
        if !rebind.logged {
            ctx.store
                .log_event(Some(id), "gate_carried", detail.clone())?;
        }
        carried = Some(detail);
        Ok(true)
    })?;
    let Some(detail) = carried else {
        return Ok(false);
    };
    crate::messages::report(
        Some(id),
        "gate",
        format!(
            "carried the {} gate from {} to {} after corral's clean, equal-patch replay onto {}",
            detail["tier"], detail["old_sha"], detail["new_sha"], detail["target"]
        ),
    );
    Ok(true)
}

fn check(ctx: &Ctx, task: &Task, target: &RunPlanTarget) -> Result<Option<Rebind>> {
    let run = task.current_run().context("missing run for gate carry")?;
    if run.gate_rules.is_legacy() {
        return Ok(None);
    }
    let Some(gate) = run.gate.as_ref().filter(|g| g.result == GateResult::Pass) else {
        return Ok(None);
    };
    let (Some(old), Some(_)) = (gate.identity.as_ref(), gate.tier.as_ref()) else {
        return Ok(None);
    };
    let Some(binding) = old.gate_tier.as_ref() else {
        return Ok(None);
    };
    // The member binding is still the current member requirement (no dominance for members).
    let contract = crate::gate_tier::select(ctx, task, run, GatePhase::Review)?;
    if contract.destination.kind != DestinationKind::PlanTarget
        || contract.destination.branch != target.branch
        || !crate::gate_tier::satisfies(Some(binding), &contract.requirement)
    {
        return Ok(None);
    }
    if gate.sha != old.head
        || !corral_git::is_clean(&run.worktree)?
        || !crate::gate::gate_log_passed(ctx, task, run)?
    {
        return Ok(None);
    }
    let new = verification::identity_with(
        ctx,
        task,
        run,
        &old.attempt,
        old.environment.clone(),
        old.gate_tier.clone(),
    )?;
    let Some(patches) = replay(ctx, task, gate, old, &new)? else {
        return Ok(None);
    };
    let source = CarrySource {
        identity: old,
        window_start: gate.at,
    };
    let mut evidence_carry = match establish(ctx, task, run, source, &new, CarryMethod::CleanReplay)
    {
        Ok(carry) => carry,
        Err(error) if error.is::<CarryRefused>() => return Ok(None),
        Err(error) => return Err(error),
    };
    let mut carry = GateCarry {
        from_head: old.head.clone(),
        from_tree: old.tree.clone(),
        from_integration_base: old.integration_base.clone(),
        old_patch_sha256: patches.0,
        new_patch_sha256: patches.1,
        at: corral_core::clock::now(),
    };
    let logged = recorded(ctx, task, old, &new, &mut carry, &mut evidence_carry)?;
    Ok(Some(Rebind {
        old: old.clone(),
        new,
        carry,
        evidence_carry,
        logged,
    }))
}

/// Only corral's recorded replay of the receipt's head, with ancestry and an equal patch, may
/// have moved head, tree and integration base. Returns the old and new patch digests.
fn replay(
    ctx: &Ctx,
    task: &Task,
    gate: &GateRecord,
    old: &CandidateIdentity,
    new: &CandidateIdentity,
) -> Result<Option<(String, String)>> {
    let run = task.current_run().context("missing run for gate carry")?;
    if old.head == new.head
        || old
            .differences(new)
            .iter()
            .any(|change| !REPLAYED.contains(&change.field.as_str()))
        || !crate::verdict_carry::clean_replay(ctx, task, run, &gate.sha, &new.head)?
        || !corral_git::is_ancestor(&run.worktree, &old.integration_base, &new.integration_base)?
        || !corral_git::is_ancestor(&run.worktree, &new.integration_base, &new.head)?
    {
        return Ok(None);
    }
    let old_base = corral_git::merge_base(&run.worktree, &old.integration_base, &old.head)?;
    let old_patch = corral_git::review_patch(&run.worktree, &old_base, &old.head)?;
    let new_patch = corral_git::review_patch(&run.worktree, &new.integration_base, &new.head)?;
    if old_patch != new_patch {
        return Ok(None);
    }
    Ok(Some((
        verification::digest(old_patch.as_bytes()),
        verification::digest(new_patch.as_bytes()),
    )))
}

/// A retry after a crash between `gate_carried` and the save rebinds once: when that event
/// already recorded this exact rebinding, its carry times are reused and no event is logged.
fn recorded(
    ctx: &Ctx,
    task: &Task,
    old: &CandidateIdentity,
    new: &CandidateIdentity,
    carry: &mut GateCarry,
    evidence_carry: &mut Option<EvidenceCarry>,
) -> Result<bool> {
    let events = ctx.store.events_shared()?;
    let Some(event) = events.iter().rev().find(|event| {
        event.task.as_deref() == Some(&task.id)
            && event.kind == "gate_carried"
            && event.detail["old_identity"] == json!(old)
            && event.detail["new_identity"] == json!(new)
    }) else {
        return Ok(false);
    };
    let prior_carry: GateCarry = serde_json::from_value(event.detail["gate_carry"].clone())?;
    let prior_evidence: Option<EvidenceCarry> =
        serde_json::from_value(event.detail["evidence_carry"].clone())?;
    let same_carry = GateCarry {
        at: prior_carry.at,
        ..carry.clone()
    } == prior_carry;
    let same_evidence = match (&*evidence_carry, &prior_evidence) {
        (None, None) => true,
        (Some(now), Some(prior)) => {
            EvidenceCarry {
                at: prior.at,
                ..now.clone()
            } == *prior
        }
        _ => false,
    };
    if !same_carry || !same_evidence {
        return Ok(false);
    }
    *carry = prior_carry;
    *evidence_carry = prior_evidence;
    Ok(true)
}

#[cfg(test)]
#[path = "gate_carry_tests.rs"]
mod tests;
