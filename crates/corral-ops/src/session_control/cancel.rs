use anyhow::{Context, Result, ensure};
use serde_json::Value;

use super::Target;
use crate::Ctx;

pub(crate) fn cancel(
    ctx: &Ctx,
    target: &Target,
    generation: Option<u32>,
    turn: Option<u64>,
) -> Result<Value> {
    // Refuse stale UI actions before any connection or host effect.
    target.check_generation(generation)?;
    target.require_bridge()?;
    let current = super::resolve(ctx, &target.id)?;
    current.check_generation(target.generation)?;
    current.require_bridge()?;
    let container = current
        .container
        .as_ref()
        .context("no_bridge: session has no container")?;
    let detail =
        crate::agent::recovery::cancel_container(ctx, container, current.generation, turn)?;
    ensure!(
        detail["cancelled"] == true,
        "no_turn: requested session turn is not open"
    );
    Ok(detail)
}
