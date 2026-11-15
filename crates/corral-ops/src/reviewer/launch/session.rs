//! Session-aware admission and optional PM attachment for reviewer launches.
use super::*;

pub(super) fn run_claimed(
    ctx: &Ctx,
    id: &str,
    reviewer: Option<&str>,
    origin: crate::operating::Origin,
    recovery: Option<&corral_core::RecoveryRecord>,
) -> Result<ReviewRecord> {
    let mut pending_paid = false;
    let mut cleanup_paid = false;
    let result = super::review_run_inner(
        ctx,
        id,
        reviewer,
        origin,
        recovery,
        &mut pending_paid,
        &mut cleanup_paid,
    );
    if result
        .as_ref()
        .is_err_and(|error| !crate::operating::refused(error))
        && cleanup_paid
    {
        super::super::fallback::fallback_paid::launch_failed(ctx, id)?;
    }
    result
}

pub(super) fn pm_reference<T: TaskRef + ?Sized>(
    ctx: &Ctx,
    task: &T,
    run: &RunHead,
    pane: Option<&str>,
    launch: &mut crate::review_budget::ReviewLaunch<'_>,
) -> Result<Option<HerdrRef>> {
    let Some(pane) = pane else {
        return Ok(None);
    };
    super::attach_pane(ctx, task, run, pane, false, launch)?;
    let info = ctx
        .herdr
        .pane_get(pane)?
        .context("PM pane disappeared before review")?;
    Ok(Some(HerdrRef {
        workspace_id: info.workspace_id,
        pane_id: pane.to_owned(),
        socket: Some(ctx.herdr.socket_path()?),
    }))
}
