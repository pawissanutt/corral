//! Closure owns both ends of a pending transfer. Task callers need the task document
//! when an already-created sibling must be fenced in the same financial commit.
use super::*;
use corral_core::{Task, TaskRef};

/// An incoming pending intent or this generation's canonical outgoing intent. The episode
/// is the outgoing index for both retained and replacement containers; Active successors
/// are deliberately excluded, regardless of whether their predecessor is later closed.
fn intent_to_revoke(
    store: &Store,
    container: &ContainerRef,
    state: &AgentRunState,
) -> Result<Option<ContinuationIntent>> {
    if state.paid_episode.is_none()
        && !state
            .current_generation()
            .and_then(|g| g.connection.as_ref())
            .is_some_and(|binding| binding.connection.api_key())
    {
        return Ok(None);
    }
    let pending = |intent: &ContinuationIntent| {
        matches!(intent.phase, IntentPhase::Prepared | IntentPhase::Paused)
    };
    if let Some(intent) = intent_for(store, container, state.binding.generation)?.filter(pending) {
        return Ok(Some(intent));
    }
    let Some(link) = &state.paid_episode else {
        return Ok(None);
    };
    let episode = store.paid_episode(&link.episode)?;
    let PaidPhase::Prepared { successor, .. } = &episode.phase else {
        return Ok(None);
    };
    Ok(
        intent_for(store, &successor.container, successor.generation)?.filter(|intent| {
            pending(intent)
                && intent.source.container == *container
                && intent.source.generation == state.binding.generation
        }),
    )
}

fn fence(state: &mut AgentRunState, generation: u32) -> Result<()> {
    state
        .generation_mut(generation)
        .context("continuation generation missing")?
        .fenced = Some(GenerationFence {
        reason: "continuation revoked".into(),
        at: corral_core::clock::now(),
    });
    if state.binding.generation == generation {
        state.binding.health = HostHealth::Fenced;
    }
    Ok(())
}

fn revoke_pending(
    store: &Store,
    state: &mut AgentRunState,
    intent: &mut ContinuationIntent,
) -> Result<bool> {
    let episode = store.paid_episode(&intent.link.episode)?;
    // An Active owner's pause already closed its financial episode.
    if intent.phase == IntentPhase::Paused && matches!(episode.phase, PaidPhase::Closed) {
        context_cache::remove(store, intent)?;
        return Ok(false);
    }
    ensure!(
        episode.epoch == intent.link.epoch
            && matches!(&episode.phase,
        PaidPhase::Prepared { successor, .. } if successor == &intent.successor),
        "SuccessorAmbiguous"
    );
    // Core cannot close Prepared episodes. This exact Closed intent is the durable fence
    // for a successor number that has not yet been opened; no generation row is fabricated.
    let generation = state.binding.generation;
    fence(state, generation)?;
    intent.phase = IntentPhase::Closed;
    intent.updated = corral_core::clock::now();
    store.save_paid_episode(&episode)?;
    save(store, intent)?;
    context_cache::remove(store, intent)?;
    event(store, "continuation_closed", intent, &episode)?;
    Ok(true)
}

/// Native-state closure. A task transaction must use close_task_in when a pending
/// transfer has an existing sibling: this state alone cannot atomically fence that sibling.
pub(crate) fn close_in(
    store: &Store,
    container: &ContainerRef,
    state: &mut AgentRunState,
    _why: &str,
) -> Result<bool> {
    if let Some(mut intent) = intent_to_revoke(store, container, state)? {
        if intent.successor.container != *container {
            let task = intent
                .successor
                .container
                .task()
                .context("continuation successor task missing")?;
            ensure!(
                store
                    .head(task)?
                    .agent_head(&intent.successor.container)?
                    .is_none(),
                "close_task_in is required to atomically fence an existing continuation sibling"
            );
        }
        return revoke_pending(store, state, &mut intent);
    }
    close_active(store, container, state)
}

/// Task transaction boundary: source, any existing pending sibling, intent, financial
/// CAS and event publish in one commit. Closing an old source never closes an Active sibling.
pub(crate) fn close_task_in(
    store: &Store,
    container: &ContainerRef,
    task: &mut Task,
    _why: &str,
) -> Result<bool> {
    ensure!(
        container.task() == Some(task.id.as_str()),
        "foreign continuation task closure"
    );
    let state = agent::container_state(task, container)
        .context("continuation container has no native state")?;
    let Some(mut intent) = intent_to_revoke(store, container, state)? else {
        return close_active(
            store,
            container,
            agent::container_state_mut(task, container)
                .context("continuation container has no native state")?,
        );
    };
    ensure!(
        intent.successor.container.task() == Some(task.id.as_str()),
        "foreign continuation successor"
    );
    let changed = revoke_pending(
        store,
        agent::container_state_mut(task, container)
            .context("continuation container has no native state")?,
        &mut intent,
    )?;
    if changed
        && intent.successor.container != *container
        && let Some(successor) = agent::container_state_mut(task, &intent.successor.container)
        && successor.generation(intent.successor.generation).is_some()
    {
        fence(successor, intent.successor.generation)?;
    }
    Ok(changed)
}

fn close_active(
    store: &Store,
    container: &ContainerRef,
    state: &mut AgentRunState,
) -> Result<bool> {
    let Some(link) = state.paid_episode.clone() else {
        return Ok(false);
    };
    let mut episode = store.paid_episode(&link.episode)?;
    let PaidPhase::Active { owner } = &episode.phase else {
        return Ok(false);
    };
    if &owner.container != container
        || owner.generation != state.binding.generation
        || link.epoch != episode.epoch
    {
        return Ok(false);
    }
    let owner = owner.clone();
    episode.close(&link, &owner, state)?;
    store.save_paid_episode(&episode)?;
    if let Some(mut intent) = intent_for(store, container, owner.generation)? {
        intent.phase = IntentPhase::Closed;
        intent.updated = corral_core::clock::now();
        save(store, &intent)?;
        context_cache::remove(store, &intent)?;
        event(store, "continuation_closed", &intent, &episode)?;
    } else {
        store.log_event(
            container.task(),
            "continuation_closed",
            json!({"episode":episode.id,
            "role":episode.root.role, "reason":"role lineage closed"}),
        )?;
    }
    Ok(true)
}

pub(crate) fn close(ctx: &Ctx, container: &ContainerRef, why: &str) -> Result<bool> {
    if let Some(task) = container.task() {
        let mut closed = false;
        ctx.store.update(task, |task| {
            closed = close_task_in(&ctx.store, container, task, why)?;
            Ok(())
        })?;
        return Ok(closed);
    }
    with_state(ctx, container, |state| {
        close_in(&ctx.store, container, state, why)
    })
}
