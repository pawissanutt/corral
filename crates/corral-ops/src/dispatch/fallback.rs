use anyhow::{Context, Result};
use corral_core::{ProfileChoice, RetryPlan};

use super::{DispatchOutcome, retry_with};
use crate::Ctx;

/// A linked continuation on the next approved candidate of the same profile, selected by the
/// availability episode. The old worker is quiescent; its last compatible checkpoint is the
/// handoff because the failed model cannot write a new one. Never a difficulty switch.
pub(crate) fn availability_retry(ctx: &Ctx, id: &str, run: &str) -> Result<DispatchOutcome> {
    let task = ctx.store.get(id)?;
    let previous = task.current_run().context("task has no run to continue")?;
    let plan = RetryPlan {
        source: Some(corral_core::RouteSource::Retry {
            previous: Box::new(
                previous
                    .route
                    .clone()
                    .unwrap_or(corral_core::RouteSource::Fallback),
            ),
            switch: None,
        }),
        overrides: Some(ProfileChoice::default()),
        profile: previous.agent.clone(),
        model: None,
        effort: None,
        switch: None,
    };
    retry_with(
        ctx,
        id,
        &ProfileChoice::default(),
        Some(run),
        false,
        None,
        Some(plan),
        |_| {},
    )
}
