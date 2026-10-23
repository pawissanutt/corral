//! Tier promotion: a main-bound approval of a reduced-tier receipt carries onto the full landing
//! receipt of the same candidate, or of corral's clean, equal-patch replay of it, together with
//! its criterion evidence. Corral re-derives the approved identity under the current inputs, so
//! any changed config, spec, verification input, environment, evidence or review route refuses.
use anyhow::{Context, Result};
use corral_core::{
    CandidateIdentity, CarryMethod, EvidenceCarry, FULL_TIER, GatePromotion, GateResult, Run, Task,
    Verdict,
};
use serde_json::{Value, json};

use crate::completion::evidence_carry::{CarryRefused, CarrySource, establish};
use crate::gate_tier::{DestinationKind, GatePhase, MAIN_LANDING_TIER};
use crate::{Ctx, verification};

/// The only fields the full landing receipt may differ in from the re-derived approval: one
/// snapshot of every input, so the policy digest differs exactly by the tier binding.
const PROMOTED: [&str; 3] = ["attempt", "gate_tier", "policy_digest"];
/// The only fields corral's own recorded replay may change in the re-derived approval.
const REPLAYED: [&str; 3] = ["head", "tree", "integration_base"];

/// A validated carry of the current approval onto the current full receipt.
struct Promotion {
    method: CarryMethod,
    from_tier: String,
    approved: CandidateIdentity,
    rederived: CandidateIdentity,
    full: CandidateIdentity,
    origin: GatePromotion,
    evidence: Value,
    evidence_carry: Option<EvidenceCarry>,
}

/// Whether the current approval would be promoted now: a readback for PM observation.
pub(crate) fn promotable(ctx: &Ctx, task: &Task) -> Result<bool> {
    Ok(check(ctx, task)?.is_some())
}

/// Rebinds a promotable approval to the full receipt, records the evidence relation and clears
/// the promotion origin in one update, after logging `verdict_carried`. False when any check
/// fails; the caller then needs a new review or reports the evidence failure.
pub(crate) fn promote(ctx: &Ctx, id: &str) -> Result<bool> {
    let mut carried = None;
    ctx.store.try_update(id, |task| {
        let Some(promotion) = check(ctx, task)? else {
            return Ok(false);
        };
        let run = task.current_run_mut().context("run disappeared")?;
        let mut review = run.review.clone().context("approval disappeared")?;
        let detail = json!({"run": run.id, "method": promotion.method,
            "from_tier": promotion.from_tier, "to_tier": FULL_TIER,
            "old_sha": promotion.approved.head, "new_sha": promotion.full.head,
            "old_identity": promotion.approved, "rederived_identity": promotion.rederived,
            "new_identity": promotion.full, "reviewer": review.reviewer,
            "promotion_origin": promotion.origin, "evidence": promotion.evidence,
            "evidence_carry": promotion.evidence_carry});
        review.sha = promotion.full.head.clone();
        review.identity = Some(promotion.full);
        run.review = Some(review);
        run.completion.evidence_carry = promotion.evidence_carry.map(Box::new);
        run.promotion_origin = None;
        // Persist provenance before the rebound approval can authorize a landing.
        ctx.store
            .log_event(Some(id), "verdict_carried", detail.clone())?;
        carried = Some(detail);
        Ok(true)
    })?;
    let Some(detail) = carried else {
        return Ok(false);
    };
    crate::messages::report(
        Some(id),
        "review",
        format!(
            "promoted the {} approval from {} to the full receipt at {} ({})",
            detail["from_tier"], detail["old_sha"], detail["new_sha"], detail["method"]
        ),
    );
    Ok(true)
}

fn check(ctx: &Ctx, task: &Task) -> Result<Option<Promotion>> {
    let Some(run) = task.current_run().filter(|run| !run.gate_rules.is_legacy()) else {
        return Ok(None);
    };
    // An Approve bound to reduced-tier identity A, and the run's origin for exactly A.
    let Some(review) = run
        .review
        .as_ref()
        .filter(|review| review.verdict == Some(Verdict::Approve))
    else {
        return Ok(None);
    };
    let Some(approved) = review.identity.as_ref() else {
        return Ok(None);
    };
    let Some(quick) = approved.gate_tier.as_deref() else {
        return Ok(None);
    };
    let Some(origin) = run
        .promotion_origin
        .as_deref()
        .filter(|origin| origin.identity == *approved)
    else {
        return Ok(None);
    };
    // A full PASS that copied that origin when it started.
    let Some(gate) = run.gate.as_ref().filter(|gate| {
        gate.result == GateResult::Pass
            && gate.tier.as_ref().is_some_and(|tier| {
                tier.tier == MAIN_LANDING_TIER && tier.promoted_from.as_ref() == Some(origin)
            })
    }) else {
        return Ok(None);
    };
    let Some(full) = gate.identity.as_ref().filter(|i| i.gate_tier.is_none()) else {
        return Ok(None);
    };
    // q is still the run's review-phase binding on main.
    let review_phase = crate::gate_tier::select(ctx, task, run, GatePhase::Review)?;
    if review_phase.destination.kind != DestinationKind::Main
        || review_phase.requirement.binding.as_ref() != Some(quick)
    {
        return Ok(None);
    }
    // The full receipt re-derives with its own environment and satisfies the landing tier.
    if !corral_git::is_clean(&run.worktree)?
        || !crate::gate::gate_log_passed(ctx, task, run)?
        || !crate::review::landing_gate_fresh(ctx, task, run, gate)?
    {
        return Ok(None);
    }
    let rederived = verification::identity_with(
        ctx,
        task,
        run,
        &approved.attempt,
        approved.environment.clone(),
        Some(Box::new(quick.clone())),
    )?;
    let Some((method, evidence)) = replay(ctx, task, run, approved, &rederived)? else {
        return Ok(None);
    };
    if full.environment != approved.environment
        || full
            .differences(&rederived)
            .iter()
            .any(|change| !PROMOTED.contains(&change.field.as_str()))
    {
        return Ok(None);
    }
    if review.evidence_digest.as_deref() != Some(verification::evidence_digest(run)?.as_str()) {
        return Ok(None);
    }
    if let Some(route) = review.route.as_deref()
        && !crate::review_packet::route_changes(ctx, task, run, review, route, false)?.is_empty()
    {
        return Ok(None);
    }
    let source = CarrySource {
        identity: approved,
        window_start: origin.gate_at,
    };
    let evidence_carry = match establish(ctx, task, run, source, full, method) {
        Ok(carry) => carry,
        Err(error) if error.is::<CarryRefused>() => return Ok(None),
        Err(error) => return Err(error),
    };
    Ok(Some(Promotion {
        method,
        from_tier: quick.tier.clone(),
        approved: approved.clone(),
        rederived,
        full: full.clone(),
        origin: origin.clone(),
        evidence,
        evidence_carry,
    }))
}

/// Same head: nothing may differ. Otherwise only corral's recorded clean replay of the approved
/// head, with ancestry and an equal patch, may have moved head, tree and integration base.
fn replay(
    ctx: &Ctx,
    task: &Task,
    run: &Run,
    approved: &CandidateIdentity,
    rederived: &CandidateIdentity,
) -> Result<Option<(CarryMethod, Value)>> {
    let changes = approved.differences(rederived);
    if changes.is_empty() {
        let evidence = json!({"method": "identity_rederivation", "permitted": PROMOTED});
        return Ok(Some((CarryMethod::TierPromotion, evidence)));
    }
    if approved.head == rederived.head
        || changes
            .iter()
            .any(|change| !REPLAYED.contains(&change.field.as_str()))
        || !super::clean_replay(ctx, task, run, &approved.head, &rederived.head)?
        || !corral_git::is_ancestor(
            &run.worktree,
            &approved.integration_base,
            &rederived.integration_base,
        )?
        || !corral_git::is_ancestor(&run.worktree, &rederived.integration_base, &rederived.head)?
    {
        return Ok(None);
    }
    let old_base =
        corral_git::merge_base(&run.worktree, &approved.integration_base, &approved.head)?;
    let old_patch = corral_git::review_patch(&run.worktree, &old_base, &approved.head)?;
    let new_patch =
        corral_git::review_patch(&run.worktree, &rederived.integration_base, &rederived.head)?;
    if old_patch != new_patch {
        return Ok(None);
    }
    let evidence = json!({"method": "git_diff_unified_0", "old_base": old_base,
        "new_base": rederived.integration_base,
        "old_patch_sha256": verification::digest(old_patch.as_bytes()),
        "new_patch_sha256": verification::digest(new_patch.as_bytes()),
        "ignored": ["blob_ids", "hunk_headers"], "clean_replay": true, "permitted": PROMOTED});
    Ok(Some((CarryMethod::CleanReplayTierPromotion, evidence)))
}
