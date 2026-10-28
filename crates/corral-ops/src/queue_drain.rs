//! Keeps the Ready-queue drain from re-attempting work whose admission has not changed (T-505).
use anyhow::Error;
use corral_core::Task;
use jiff::Timestamp;

use crate::dispatch::Admission;
use crate::{Ctx, DispatchOpts};

/// A queued task whose admission check still names a blocker needs no dispatch attempt this pass.
/// The attempt would only take its launch and session-ownership locks, re-resolve its route and log
/// the same `queued` reason again. This is the claim's own check, read without locks; the claim
/// repeats it, so a stale read can only delay a launch by one pass, never admit one.
pub(crate) fn still_blocked(ctx: &Ctx, task: &Task, rank: usize, at: Timestamp) -> bool {
    if !task.queued || task.launch.is_some() || task.dispatch_plan.is_none() {
        return false;
    }
    let admission = Admission::Queued { rank, at };
    let opts = DispatchOpts {
        queue_if_blocked: true,
        ..DispatchOpts::default()
    };
    let opts = DispatchOpts {
        plan: admission.plan_requested(task, &opts),
        ..opts
    };
    matches!(admission.reason(ctx, &ctx.store, task, &opts), Ok(Some(_)))
}

/// Lock contention with another sync pass or launch clears by itself, and the next pass retries,
/// so it is not a dispatch failure worth an event.
pub(crate) fn contended(error: &Error) -> bool {
    const MARKERS: [&str; 2] = [
        "pane inventory is being reconciled",
        "dispatch is already in progress",
    ];
    error
        .chain()
        .any(|cause| MARKERS.iter().any(|m| cause.to_string().contains(m)))
}
