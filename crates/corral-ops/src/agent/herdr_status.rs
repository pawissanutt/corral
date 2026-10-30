//! The corral side of a Pi-native host's herdr projection (T-605). A bound host reports itself to
//! its pane as agent `corral-pi` (`packages/corral-pi/src/host/herdr.ts`); that status is display
//! only, never evidence of a run's outcome. When sync records a generation lost, corral releases
//! the generation's report so the pane stops showing stale working or blocked state. It never
//! closes the pane or types into it.
use anyhow::Result;
use corral_core::agent::{AgentGeneration, ContainerRef};
use corral_herdr::Herdr;
use jiff::Timestamp;

use crate::Ctx;

pub const SOURCE: &str = "custom:corral-pi";
pub const AGENT: &str = "corral-pi";
/// The metadata tokens a host sets (`TOKENS` in herdr.ts).
pub const TOKENS: [&str; 8] = [
    "corral_role",
    "corral_project",
    "corral_task",
    "corral_run",
    "corral_review",
    "corral_assistant",
    "corral_generation",
    "corral_session",
];
const SEQ_SHIFT: u32 = 20;

/// The last herdr sequence number of a generation hello bound at `bound_at`. Its host reports
/// below it, from `ms << 20`; a later generation, bound at least a millisecond later, reports
/// above it. Herdr ignores a release that is not above the last report it accepted, so this one
/// can never clear a replacement.
pub fn release_seq(bound_at: Timestamp) -> Option<u64> {
    let ms = u64::try_from(bound_at.as_millisecond()).ok()?;
    ms.checked_add(1)?
        .checked_mul(1 << SEQ_SHIFT)?
        .checked_sub(1)
}

/// Releases a lost generation's herdr report and clears its metadata. False when there is
/// nothing to release: the generation never bound, or its pane is gone.
pub fn release_lost(herdr: &Herdr, generation: &AgentGeneration) -> Result<bool> {
    let Some(seq) = generation.bound.as_ref().and_then(|b| release_seq(b.at)) else {
        return Ok(false);
    };
    let pane = generation.pane.as_str();
    Ok(herdr.pane_release_agent(pane, SOURCE, AGENT, seq)?
        && herdr.pane_clear_metadata(pane, SOURCE, &TOKENS, seq)?)
}

/// `release_lost` for sync, which records the loss first: a failure is reported, never fatal.
pub fn release_after_loss(ctx: &Ctx, container: &ContainerRef, generation: &AgentGeneration) {
    if let Err(error) = release_lost(&ctx.herdr, generation) {
        let reason = format!(
            "releasing the herdr status of {} generation {}: {error:#}",
            container.key(),
            generation.n
        );
        crate::messages::report(container.task(), "agent herdr status", reason);
    }
}

#[cfg(all(test, unix))]
#[path = "herdr_status_tests.rs"]
mod tests;
