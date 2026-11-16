//! What the PM must hold to land a candidate itself. Overriding a completed check, a
//! reviewer's verdict or an explicit `accept = "owner"` stays with the owner.
use anyhow::{Context, Result, ensure};
use corral_core::{Store, TaskRef, Verdict};

use crate::{Ctx, verification};

/// Whether this invocation is the PM's rather than the owner's.
pub(super) fn caller(invoker: &crate::command::Invoker) -> bool {
    invoker.pm_marker() && invoker.owner_action.is_none()
}

/// What no later carry or tier promotion can change, refused before the approval is
/// prepared: explicit owner acceptance, and any review that did not end in approve.
pub(super) fn preflight<T: TaskRef + ?Sized>(ctx: &Ctx, store: &Store, task: &T) -> Result<()> {
    let run = task.current_run_head()?.context("run disappeared")?;
    ensure!(
        !store
            .config
            .explicit_owner_acceptance(&task.shell_task()?, false),
        "pm_landing_refused: this project or task sets accept = \"owner\"; the owner lands it"
    );
    let approved = match &run.review {
        // A review that exists binds the PM, whether or not policy required it.
        Some(review) => review.verdict == Some(Verdict::Approve),
        None => !super::review_needed(&verification::policy(ctx, task)?, task),
    };
    ensure!(
        approved,
        "pm_landing_refused: no independent approval of this candidate; only the owner overrides a verdict"
    );
    Ok(())
}

/// The executor's evidence bar, applied in-lock to a PM-run `corral approve` once any
/// carry or promotion has run.
pub(super) fn require<T: TaskRef + ?Sized>(
    ctx: &Ctx,
    store: &Store,
    id: &str,
    task: &T,
) -> Result<()> {
    preflight(ctx, store, task)?;
    let run = task.current_run_head()?.context("run disappeared")?;
    let run = &*run;
    ensure!(
        verification::policy(ctx, task)?.gate.is_empty()
            || super::landing_gate_passed(ctx, task, run)?,
        "pm_landing_refused: the configured gate has not passed on this candidate; only the owner overrides a check"
    );
    ensure!(
        run.review.is_none() || verification::review_fresh(ctx, task, run)?,
        "pm_landing_refused: the approval does not cover this candidate; only the owner overrides a verdict"
    );
    crate::gate_triage::require_pass(store, id, run.gate.as_ref())
}
