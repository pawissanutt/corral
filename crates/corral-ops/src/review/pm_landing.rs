//! What the PM must hold to land a candidate itself. Overriding a completed check, a
//! reviewer's verdict or an explicit `accept = "owner"` stays with the owner.
use anyhow::{Context, Result, ensure};
use corral_core::{Store, TaskRef, Verdict};

use crate::{Ctx, verification};

/// Whether this process is the PM's shell rather than the owner's.
pub(super) fn caller() -> bool {
    let invoker = crate::command::Invoker::process();
    invoker.pm_marker() && invoker.owner_action.is_none()
}

/// The executor's evidence bar, applied to a PM-run `corral approve`.
pub(super) fn require<T: TaskRef + ?Sized>(
    ctx: &Ctx,
    store: &Store,
    id: &str,
    task: &T,
) -> Result<()> {
    let run = task.current_run_head()?.context("run disappeared")?;
    let run = &*run;
    ensure!(
        !store
            .config
            .explicit_owner_acceptance(&task.shell_task()?, false),
        "pm_landing_refused: this project or task sets accept = \"owner\"; the owner lands it"
    );
    let policy = verification::policy(ctx, task)?;
    ensure!(
        policy.gate.is_empty() || super::landing_gate_passed(ctx, task, run)?,
        "pm_landing_refused: the configured gate has not passed on this candidate; only the owner overrides a check"
    );
    ensure!(
        !super::review_needed(&policy, task)
            || (verification::review_fresh(ctx, task, run)?
                && run.review.as_ref().and_then(|review| review.verdict) == Some(Verdict::Approve)),
        "pm_landing_refused: no fresh independent approval of this candidate; only the owner overrides a verdict"
    );
    crate::gate_triage::require_pass(store, id, run.gate.as_ref())
}
