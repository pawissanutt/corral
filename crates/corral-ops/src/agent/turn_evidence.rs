//! Bounded causal turn evidence shared by worker/reviewer OD11. No reconstructed host state.
#[cfg(all(test, unix))]
#[path = "turn_evidence_tests.rs"]
mod tests;

use anyhow::Result;
use corral_core::TaskRef;
use corral_core::agent::{AgentGeneration, AgentTurn, ContainerRef, TurnClass};

pub(crate) fn latest_silent<T: TaskRef + ?Sized>(
    task: &T,
    container: &ContainerRef,
    generation: u32,
    class: TurnClass,
) -> Result<Option<AgentTurn>> {
    if task.has_open_agent_turn(container, generation)? {
        return Ok(None);
    }
    Ok(task
        .latest_agent_turn(container, generation)?
        .filter(|turn| turn.class == Some(class)))
}

/// The original nudge's first append-order matching turn, or the recorded end if evicted.
pub(crate) fn later_than_nudge<T: TaskRef + ?Sized>(
    task: &T,
    container: &ContainerRef,
    current: &AgentGeneration,
    latest: &AgentTurn,
) -> Result<bool> {
    let Some(nudged) = current.nudged_at else {
        return Ok(false);
    };
    let original = task.agent_turn_ended_at(container, current.n, nudged)?;
    Ok(match original {
        Some(turn) => latest.seq > turn.seq,
        None => latest.ended.is_some_and(|ended| ended > nudged),
    })
}
