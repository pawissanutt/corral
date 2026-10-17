//! Carry only an unchanged, cleanly replayed approval, never its gate result.
use anyhow::{Context, Result, bail};
use corral_core::{GateResult, Run, Task, Verdict};
use corral_git::RebaseOutcome;
use serde_json::json;

use crate::{ApproveStrategy, Ctx, gate_collect, gate_start, gate_wait, verification};

fn clean_replay(ctx: &Ctx, task: &Task, run: &Run, old: &str, new: &str) -> Result<bool> {
    Ok(ctx.store.events()?.iter().rev().any(|event| {
        event.task.as_deref() == Some(&task.id)
            && event.kind == "candidate_rebased"
            && event.detail["run"] == run.id
            && event.detail["review_sha"] == old
            && event.detail["new_sha"] == new
    }))
}

/// Records the successful replay boundary so conflict resolutions or arbitrary
/// later edits cannot acquire the old review merely by rerunning a gate.
pub(crate) fn rebase(ctx: &Ctx, task: &Task, run: &Run, onto: &str) -> Result<RebaseOutcome> {
    let before = corral_git::rev_sha(&run.worktree, "HEAD")?;
    let review_sha = match &run.review {
        Some(review)
            if review.verdict == Some(Verdict::Approve)
                && (review.sha == before
                    || clean_replay(ctx, task, run, &review.sha, &before)?) =>
        {
            Some(review.sha.clone())
        }
        _ => None,
    };
    let outcome = corral_git::rebase(&run.worktree, onto)?;
    if outcome == RebaseOutcome::Rebased {
        let after = corral_git::rev_sha(&run.worktree, "HEAD")?;
        ctx.store.log_event(
            Some(&task.id),
            "candidate_rebased",
            json!({"run":run.id, "old_sha":before, "new_sha":after,
                "base":onto, "review_sha":review_sha}),
        )?;
    }
    Ok(outcome)
}

/// Rebind approval only after the integrated gate passes. Other identity inputs
/// and completion evidence remain fixed; patch comparison is whitespace-sensitive.
pub(crate) fn carry(ctx: &Ctx, id: &str) -> Result<bool> {
    let mut carried = None;
    ctx.store.try_update(id, |task| {
        let run = task.current_run().context("missing run for verdict carry")?;
        let Some(review) = run.review.as_ref().filter(|r| r.verdict == Some(Verdict::Approve)) else {
            return Ok(false);
        };
        let Some(old) = &review.identity else { return Ok(false) };
        if review.sha != old.head || !corral_git::is_clean(&run.worktree)? {
            return Ok(false);
        }
        let policy = verification::policy(ctx, task)?;
        if !policy.gate.is_empty() && !verification::gate_passed(ctx, task, run)? {
            return Ok(false);
        }
        let new = verification::review_identity(
            ctx, task, run,
            run.gate.as_ref().and_then(|g| g.identity.as_ref())
                .map_or(old.attempt.as_str(), |i| i.attempt.as_str()),
        )?;
        if old.head == new.head
            || !clean_replay(ctx, task, run, &old.head, &new.head)?
            || old.differences(&new).iter().any(|change| {
                !matches!(change.field.as_str(), "attempt" | "head" | "tree" | "integration_base")
            })
            || review.evidence_digest.as_deref() != Some(&verification::evidence_digest(run)?)
            || !corral_git::is_ancestor(&run.worktree, &old.integration_base, &new.integration_base)?
            || !corral_git::is_ancestor(&run.worktree, &new.integration_base, &new.head)?
        {
            return Ok(false);
        }
        let old_base = corral_git::merge_base(&run.worktree, &old.integration_base, &old.head)?;
        let old_patch = corral_git::review_patch(&run.worktree, &old_base, &old.head)?;
        let new_patch = corral_git::review_patch(&run.worktree, &new.integration_base, &new.head)?;
        if old_patch != new_patch {
            return Ok(false);
        }
        let detail = json!({"run":run.id, "old_sha":old.head, "new_sha":new.head,
            "old_identity":old, "new_identity":new, "reviewer":review.reviewer,
            "evidence":{"method":"git_diff_unified_0", "old_base":old_base,
                "new_base":new.integration_base, "old_patch_sha256":verification::digest(old_patch.as_bytes()),
                "new_patch_sha256":verification::digest(new_patch.as_bytes()),
                "ignored":["blob_ids", "hunk_headers"], "clean_replay":true}});
        let mut review = review.clone();
        review.sha = new.head.clone();
        review.identity = Some(new);
        task.current_run_mut().context("run disappeared")?.review = Some(review);
        // Persist provenance before the receipt can authorize a landing.
        ctx.store.log_event(Some(id), "verdict_carried", detail.clone())?;
        carried = Some(detail);
        Ok(true)
    })?;
    if let Some(detail) = carried {
        crate::messages::report(
            Some(id),
            "review",
            format!(
                "carried approve verdict from {} to {} after equal patches and a fresh gate",
                detail["old_sha"], detail["new_sha"]
            ),
        );
        return Ok(true);
    }
    Ok(false)
}

/// CLI approval waits for its gate; sync uses the same replay/carry boundaries
/// asynchronously through its existing Gating stage.
pub(crate) fn prepare_approval(ctx: &Ctx, id: &str, strategy: ApproveStrategy) -> Result<()> {
    let task = ctx.store.get(id)?;
    if !task.awaiting_owner() || strategy == ApproveStrategy::KeepBranch {
        return Ok(());
    }
    let run = crate::review::current_run(&task)?;
    let policy = verification::policy(ctx, &task)?;
    let store = corral_core::Store::open(ctx.store.root())?;
    // With no verification to orchestrate, the existing landing transaction
    // already handles the entire manual rebase/merge operation.
    if policy.gate.is_empty()
        && policy.reviewer == corral_core::Reviewer::None
        && run.review.is_none()
        && task.reviewer.is_none()
        && !store
            .config
            .has_review_policy(task.project.as_deref(), task.review)
    {
        return Ok(());
    }
    verification::require_gate(&policy, &task)?;
    let repo = crate::review::main_repo(ctx, &task, &run)?;
    {
        let _lock = corral_git::landing_lock(&repo)?;
        let base = corral_git::rev_sha(&repo, &run.base)?;
        let head = corral_git::rev_sha(&run.worktree, "HEAD")?;
        if !corral_git::is_ancestor(&repo, &base, &head)? {
            if strategy == ApproveStrategy::FastForward {
                bail!(
                    "candidate is behind {}; run `corral approve {id} --merge rebase`",
                    run.base
                );
            }
            if !corral_git::is_clean(&run.worktree)? {
                bail!(
                    "commit changes in {} then run `corral approve {id}`",
                    run.worktree.display()
                );
            }
            match rebase(ctx, &task, &run, &base)? {
                RebaseOutcome::Rebased => {}
                RebaseOutcome::Conflict { paths, .. } => {
                    bail!(
                        "rebase conflicted in {}; run `git -C {} rebase {}` and resolve conflicts, then `corral gate {id}` and `corral review-run {id}`",
                        paths.join(", "),
                        run.worktree.display(),
                        run.base
                    );
                }
            }
        }
    }
    let gate_complete = match &run.gate {
        Some(g) if g.result != GateResult::Running => {
            verification::gate_fresh(ctx, &task, &run, g)?
        }
        _ => false,
    };
    if gate_complete
        && run
            .gate
            .as_ref()
            .is_some_and(|g| g.result == GateResult::Fail)
        && ctx.store.events()?.iter().any(|event| {
            event.task.as_deref() == Some(id)
                && event.kind == "candidate_rebased"
                && event.detail["run"] == run.id
                && run.gate.as_ref().is_some_and(|gate| {
                    event.detail["new_sha"] == gate.sha
                        && gate.identity.as_ref().is_some_and(|identity| {
                            event.detail["base"] == identity.integration_base
                        })
                })
        })
    {
        bail!(
            "gate failed; inspect {} and run `corral gate {id} --wait` after fixing it",
            crate::gate::gate_paths(ctx, id, &run.id).log.display()
        );
    }
    if !policy.gate.is_empty() && !gate_complete {
        if run.gate.is_some() {
            gate_collect(ctx, id)?;
        }
        let task = ctx.store.get(id)?;
        let run = crate::review::current_run(&task)?;
        let running = match &run.gate {
            Some(g) if g.result == GateResult::Running => {
                verification::gate_fresh(ctx, &task, &run, g)?
            }
            _ => false,
        };
        if !running && !verification::gate_passed(ctx, &task, &run)? {
            gate_start(ctx, id)
                .with_context(|| format!("run `corral gate {id}` then `corral approve {id}`"))?;
        }
        let gate = gate_wait(ctx, id)
            .with_context(|| format!("run `corral gate {id}` then `corral approve {id}`"))?;
        if gate.result != GateResult::Pass {
            bail!(
                "integrated gate failed; fix {} then run `corral gate {id}` and `corral approve {id}`",
                crate::gate::gate_paths(ctx, id, &run.id).log.display()
            );
        }
    }
    carry(ctx, id)?;
    Ok(())
}
