//! Owning attention reuses only an actual already-holding resource observation.
use crate::{Ctx, attention};
use anyhow::Result;
use jiff::Timestamp;

pub(super) fn reconcile(
    ctx: &Ctx,
    now: Timestamp,
    pm_live: Option<bool>,
    output: Option<&mut Option<attention::AttentionBoard>>,
    held_admission: Option<&str>,
) -> Result<()> {
    if let Some(reason) = held_admission {
        let board = attention::reconcile_sampled(
            ctx,
            now,
            pm_live,
            output.is_some(),
            &Some(reason.into()),
        )?;
        if let Some(output) = output {
            *output = board;
        }
        return Ok(());
    }
    if let Some(output) = output {
        *output = attention::reconcile_projected(ctx, now, pm_live)?;
        Ok(())
    } else {
        attention::reconcile(ctx, now, pm_live)
    }
}
