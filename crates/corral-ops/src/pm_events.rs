//! Opt-in event delivery to the workspace's remembered PM, shared by every sync caller.

use anyhow::{Context, Result, bail};
use corral_core::records::RecordArea;
use corral_core::{EventCursor, actionable_kind, format_event_line};
use serde_json::json;

use crate::Ctx;

const STATE: RecordArea<EventCursor> = RecordArea::new("pm-events");

/// Serialize delivery across board workers, hooks, and `sync --watch`.
///
/// Disabled/unavailable delivery checkpoints EOF so those events are skipped, not queued.
/// A failed prompt leaves its cursor uncommitted for the next sync.
pub(crate) fn push_events(ctx: &Ctx, herdr_available: bool) -> Result<()> {
    let _attribution = ctx.store.attribute(corral_core::Actor::Pm);
    let Some(_lock) = ctx.store.try_lock("pm-events")? else {
        return Ok(());
    };
    if let Some(pm) = corral_core::agent::pm_state::load_current(&ctx.store)? {
        return crate::agent::pm_event_wake::push(ctx, pm);
    }
    crate::agent::pm_event_wake::checkpoint(ctx)?;
    let pane = if ctx.store.config.pm.push_events && herdr_available {
        crate::pm::live_pm_pane(ctx)?
    } else {
        None
    };
    let Some(pane) = pane else {
        return save_cursor(ctx, ctx.store.events_since(None)?.cursor());
    };
    let cursor = STATE.load(&ctx.store, "state")?.unwrap_or_default();
    let mut reader = ctx.store.events_since(Some(cursor))?;
    // Bound one sync's work even when producers are continuously appending.
    for _ in 0..256 {
        let before = reader.read_position();
        let Some((event, cursor)) = reader.next_event()? else {
            save_cursor(ctx, reader.cursor())?;
            if reader.read_position() == before {
                break;
            }
            continue;
        };
        let defaults = ctx
            .store
            .config
            .pm
            .push_kinds
            .iter()
            .map(String::as_str)
            .eq(corral_core::PM_DEFAULT_KINDS.iter().copied());
        if (!defaults || corral_core::default_pm_event(&event))
            && actionable_kind(&event)
                .is_some_and(|kind| ctx.store.config.pm.push_kinds.iter().any(|k| k == kind))
        {
            let text = format!("[corral] {}", format_event_line(&event));
            let delivery = ctx
                .herdr
                .agent_prompt(&pane, &text)
                .with_context(|| format!("pushing corral event to PM pane {pane}"))?;
            // This kind is deliberately non-actionable: delivery accounting must not push itself.
            ctx.store.log_event(
                event.task.as_deref(),
                "pm_event_delivery",
                json!({
                    "pane": pane, "event_kind": event.kind, "cursor": cursor,
                    "via": delivery.via, "submitted": delivery.submitted,
                    "attempts": delivery.attempts, "error": delivery.error,
                }),
            )?;
            if !delivery.submitted {
                bail!(
                    "PM event was not submitted to pane {pane}; retained for next sync: {}",
                    delivery
                        .error
                        .as_deref()
                        .unwrap_or("submission was not confirmed")
                );
            }
        }
        save_cursor(ctx, &cursor)?;
    }
    Ok(())
}

fn save_cursor(ctx: &Ctx, cursor: &EventCursor) -> Result<()> {
    STATE.save(&ctx.store, "state", cursor)
}
