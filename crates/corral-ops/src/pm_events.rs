//! Opt-in event delivery to the workspace's remembered PM, shared by every sync caller.

use std::io::ErrorKind;
use std::path::Path;

use anyhow::{Context, Result, bail};
use corral_core::{EventCursor, EventReader, actionable_kind, format_event_line};
use serde_json::json;

use crate::Ctx;

/// Serialize delivery across board workers, hooks, and `sync --watch`.
///
/// Disabled/unavailable delivery checkpoints EOF so those events are skipped, not queued.
/// A failed prompt leaves its cursor uncommitted for the next sync.
pub(crate) fn push_events(ctx: &Ctx, herdr_available: bool) -> Result<()> {
    let _attribution = ctx.store.attribute(corral_core::Actor::Pm);
    let Some(_lock) = ctx.store.try_lock("pm-events")? else {
        return Ok(());
    };
    let path = ctx.store.dir().join("events.jsonl");
    let state = ctx.store.state_dir().join("pm-events.json");
    let pane = if ctx.store.config.pm.push_events && herdr_available {
        crate::pm::live_pm_pane(ctx)?
    } else {
        None
    };
    let Some(pane) = pane else {
        return save_cursor(&state, EventReader::open(&path, None)?.cursor());
    };
    let cursor = match std::fs::read(&state) {
        Ok(bytes) => serde_json::from_slice::<EventCursor>(&bytes)
            .with_context(|| format!("parsing {}", state.display()))?,
        Err(err) if err.kind() == ErrorKind::NotFound => EventCursor::default(),
        Err(err) => return Err(err).with_context(|| format!("reading {}", state.display())),
    };
    let mut reader = EventReader::open(&path, Some(cursor))?;
    // Bound one sync's work even when producers are continuously appending.
    for _ in 0..256 {
        let before = reader.read_position();
        let Some((event, cursor)) = reader.next_event()? else {
            save_cursor(&state, reader.cursor())?;
            if reader.read_position() == before {
                break;
            }
            continue;
        };
        if actionable_kind(&event)
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
        save_cursor(&state, &cursor)?;
    }
    Ok(())
}

fn save_cursor(path: &Path, cursor: &EventCursor) -> Result<()> {
    let tmp = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec(cursor).context("serializing PM event cursor")?;
    std::fs::write(&tmp, bytes).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("saving {}", path.display()))
}
