//! Legacy activity evidence for the sync status resolver.
use crate::{Ctx, omo};
use corral_core::Run;
use jiff::{SignedDuration, Timestamp};

/// OmO runs only: the newest session file for the run's worktree changed after the run started and
/// less than `window` before `now`.
pub(super) fn omo_recently_active(
    ctx: &Ctx,
    run: &Run,
    now: Timestamp,
    window: SignedDuration,
) -> bool {
    if !omo::is_omo(run) {
        return false;
    }
    let agent = omo::agent_dir(ctx.store.config.sync.omo_agent_dir.as_deref());
    omo::last_activity(&agent, &run.worktree)
        .is_some_and(|t| t >= run.started && now.duration_since(t) < window)
}
