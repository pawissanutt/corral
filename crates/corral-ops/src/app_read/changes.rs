//! Durable owner-change history; per-device seen state belongs to the gateway.
use anyhow::{Result, bail};
use corral_core::backend::{EventQuery, EventSeq, PageReq};
use corral_protocol::app::{CHANGES_LIMIT_MAX, ChangeLine, ChangesPage, ChangesQuery, SeenCursor};

use super::{paging::json_len, short};
use crate::Ctx;

/// Read at most 200 durable events using indexed exclusive sequence bounds.
/// `after` pages ascend so reconnect catch-up cannot skip intermediate events;
/// ordinary history and `before` pages descend. `head` is an event sequence, not
/// the change-feed checkpoint. The authenticated gateway replaces `seen` with
/// the enrolled device's durable seen cursor before returning the response.
pub fn changes(ctx: &Ctx, query: &ChangesQuery) -> Result<ChangesPage> {
    if query.limit == 0 || query.limit > CHANGES_LIMIT_MAX {
        bail!("changes limit must be between 1 and {CHANGES_LIMIT_MAX}");
    }
    if matches!((query.after, query.before), (Some(after), Some(before)) if after >= before) {
        bail!("changes after must precede before");
    }
    ctx.read_step(|ctx| {
        let view = ctx.store.view()?;
        let limit = u32::from(query.limit);
        let head = view
            .events(
                &EventQuery {
                    newest_first: true,
                    ..Default::default()
                },
                PageReq::first(1),
            )?
            .items
            .first()
            .map_or(0, |event| event.seq.0);
        let query = EventQuery {
            after: query.after.map(EventSeq),
            before: query.before.map(EventSeq),
            newest_first: query.after.is_none(),
            ..Default::default()
        };
        let found = view.events(&query, PageReq::first(limit))?;
        let mut lines = Vec::new();
        let mut bytes = 1024usize;
        // A page-sized source query is always bounded independently of store size.
        for event in found.items {
            let mapped = crate::boardview::changes::change_line(&event);
            let class = serde_json::to_value(mapped.class)?
                .as_str()
                .unwrap_or("other")
                .to_owned();
            let line = ChangeLine {
                seq: mapped.seq,
                at: mapped.at,
                class,
                task: mapped.task.map(|task| short(&task, 512)),
                text: short(&mapped.text, 2048),
                actor: event
                    .event
                    .actor
                    .as_deref()
                    .map_or_else(String::new, |actor| short(actor, 256)),
                via: event
                    .event
                    .via
                    .as_deref()
                    .map_or_else(String::new, |via| short(via, 256)),
            };
            let size = json_len(&line)? + 1;
            if bytes + size > corral_protocol::app::APP_CONTROL_MAX - 2048 {
                break;
            }
            bytes += size;
            lines.push(line);
        }
        let oldest = lines.iter().map(|line| line.seq).min();
        let older = if let Some(oldest) = oldest {
            let preceding = view.events(
                &EventQuery {
                    before: Some(EventSeq(oldest)),
                    newest_first: true,
                    ..Default::default()
                },
                PageReq::first(1),
            )?;
            (!preceding.items.is_empty()).then_some(oldest)
        } else {
            None
        };
        Ok(ChangesPage {
            lines,
            head,
            older,
            seen: SeenCursor::default(),
        })
    })
}
