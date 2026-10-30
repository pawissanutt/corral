//! The corral side of a Pi-native host's herdr projection (T-605). A bound host reports itself to
//! its pane as agent `corral-pi` (`packages/corral-pi/src/host/herdr.ts`); that status is display
//! only, never evidence of a run's outcome. Corral allocates each binding its herdr sequence
//! epoch, tells a live host about question transitions it did not make itself, and releases a
//! generation sync recorded lost so its pane stops showing stale working or blocked state. It never
//! closes the pane or types into it.
use anyhow::{Context, Result};
use corral_core::agent::{
    self, AgentGeneration, AgentQuestion, AgentRunState, ContainerRef, GenerationBound, HostHealth,
    QuestionKind, QuestionState,
};
use corral_herdr::Herdr;
use jiff::Timestamp;
use serde_json::{Value, json};

use super::bridge::{BridgeClient, BridgeError};
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
const EPOCH_FILE: &str = "herdr-seq-epoch";
/// The newest questions a snapshot names; a bridge frame holds far more than a container asks.
const QUESTIONS_MAX: usize = 128;

/// Allocates a binding's herdr sequence epoch: its hello time in milliseconds, raised above every
/// epoch allocated before for this user. The high-water mark is persisted under the runtime root
/// all of the user's stores share, so a later binding in any pane reports above an earlier one
/// even when the wall clock stepped back or both bind within one millisecond.
#[cfg(unix)]
pub fn allocate_epoch(now: Timestamp) -> Result<u64> {
    let root = super::launch::runtime_root();
    super::launch::ensure_private_dir(&root, &root)?;
    let lock_path = root.join(format!("{EPOCH_FILE}.lock"));
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)
        .with_context(|| format!("opening {}", lock_path.display()))?;
    rustix::fs::flock(&lock, rustix::fs::FlockOperation::LockExclusive)
        .with_context(|| format!("locking {}", lock_path.display()))?;
    let path = root.join(EPOCH_FILE);
    let last = match std::fs::read_to_string(&path) {
        Ok(text) => text
            .trim()
            .parse::<u64>()
            .with_context(|| format!("reading {}", path.display()))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
        Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
    };
    let ms = u64::try_from(now.as_millisecond()).unwrap_or(0);
    let epoch = ms.max(
        last.checked_add(1)
            .context("herdr sequence epochs exhausted")?,
    );
    let staged = root.join(format!("{EPOCH_FILE}.{}", std::process::id()));
    std::fs::write(&staged, epoch.to_string())
        .with_context(|| format!("writing {}", staged.display()))?;
    std::fs::rename(&staged, &path).with_context(|| format!("installing {}", path.display()))?;
    Ok(epoch)
}

#[cfg(not(unix))]
pub fn allocate_epoch(_now: Timestamp) -> Result<u64> {
    anyhow::bail!("herdr sequence epochs need a Unix runtime directory")
}

/// The epoch a bound generation reports from. A generation bound before T-605 has none, and its
/// host never reported, so its hello time stands in.
pub fn bound_epoch(bound: &GenerationBound) -> Option<u64> {
    bound
        .herdr_seq_epoch
        .or_else(|| u64::try_from(bound.at.as_millisecond()).ok())
}

/// The last herdr sequence number of the range that starts at `epoch << 20`. Its host reports
/// below it, and every later epoch's range starts above it; herdr ignores a release that is not
/// above the last report it accepted, so this one can never clear a later generation.
pub fn range_end(epoch: u64) -> Option<u64> {
    epoch
        .checked_add(1)?
        .checked_mul(1 << SEQ_SHIFT)?
        .checked_sub(1)
}

/// Releases a lost generation's herdr report and clears its metadata. False when there is
/// nothing to release: the generation never bound, or its pane is gone.
pub fn release_lost(herdr: &Herdr, generation: &AgentGeneration) -> Result<bool> {
    let Some(seq) = generation
        .bound
        .as_ref()
        .and_then(bound_epoch)
        .and_then(range_end)
    else {
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

/// The container's questions as a host merges them: by key, newest revision wins.
pub fn question_snapshot(state: &AgentRunState) -> Vec<Value> {
    let start = state.questions.len().saturating_sub(QUESTIONS_MAX);
    state.questions[start..]
        .iter()
        .map(|q| {
            let current = q
                .terminal
                .as_ref()
                .map_or(q.state, |terminal| terminal.state);
            json!({"key": q.key, "revision": q.revision, "kind": q.kind,
                   "blocking": q.blocking, "state": current})
        })
        .collect()
}

/// A question the host shows as `blocked` (`holds` in herdr.ts): pending, and blocking or one
/// the owner must answer.
fn holds(question: &AgentQuestion) -> bool {
    question.recorded_pending()
        && (question.blocking
            || matches!(
                question.kind,
                QuestionKind::OwnerOnly | QuestionKind::Permission
            )
            || question.state == QuestionState::Escalated)
}

fn send(client: &mut BridgeClient, state: &AgentRunState) -> Result<()> {
    let questions = question_snapshot(state);
    match client.request("questions", json!({ "questions": questions })) {
        Ok(_) => Ok(()),
        // A host that predates the command has no projection to correct.
        Err(error)
            if error
                .downcast_ref::<BridgeError>()
                .is_some_and(|e| e.code == "unknown_command") =>
        {
            Ok(())
        }
        Err(error) => Err(error),
    }
}

/// Sends a task container's questions to its live host over the authenticated bridge, so its herdr
/// projection follows a transition the host did not make itself (an escalation or cancellation).
/// Nothing happens without a healthy, bound, unfenced current generation.
pub fn push_questions(ctx: &Ctx, container: &ContainerRef) -> Result<()> {
    let Some(id) = container.task() else {
        return Ok(());
    };
    let task = ctx.store.get(id)?;
    let (Some(state), Some(recipient)) = (
        agent::container_state(&task, container),
        crate::questions::recipient(container),
    ) else {
        return Ok(());
    };
    let live = state
        .current_generation()
        .filter(|g| g.bound.is_some() && g.fenced.is_none());
    let Some(generation) = live.filter(|_| state.binding.health == HostHealth::Healthy) else {
        return Ok(());
    };
    let mut client = BridgeClient::connect(&ctx.store, &recipient, generation.n)?;
    send(&mut client, state)
}

/// `push_questions` after a question transition: a failure is reported, and sync's next probe
/// sends the questions again while the host's projection still differs.
pub fn push_after_transition(ctx: &Ctx, container: &ContainerRef) {
    if let Err(error) = push_questions(ctx, container) {
        let reason = format!(
            "telling {}'s host about a question transition: {error:#}",
            container.key()
        );
        crate::messages::report(container.task(), "agent herdr status", reason);
    }
}

/// During a healthy probe: sends the container's questions when the host's projection, as its
/// `get_state` reply names it, holds other questions than corral does.
pub fn sync_questions(
    client: &mut BridgeClient,
    state: &AgentRunState,
    reply: &Value,
) -> Result<()> {
    let Some(held) = reply["herdr"]["blocked_on"].as_array() else {
        return Ok(());
    };
    let mut held: Vec<&str> = held.iter().filter_map(Value::as_str).collect();
    let mut holding: Vec<&str> = state
        .questions
        .iter()
        .filter(|q| holds(q))
        .map(|q| q.key.as_str())
        .collect();
    held.sort_unstable();
    holding.sort_unstable();
    if held == holding {
        return Ok(());
    }
    send(client, state)
}

#[cfg(all(test, unix))]
#[path = "herdr_status_tests.rs"]
mod tests;
