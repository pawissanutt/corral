//! Owner-relevant event projection; authenticated seen state is supplied by the gateway.
use anyhow::{Result, bail};
use corral_core::backend::{EventQuery, EventSeq, PageReq, StoredEvent};
use corral_protocol::app::{
    CHANGES_LIMIT_MAX, ChangeKind, ChangeLine, ChangesPage, ChangesQuery, SeenCursor,
};

use super::{change_kinds, paging::json_len, short};
use crate::Ctx;

/// Page and totals select the same vocabulary on one pinned view. Raw event sequences
/// remain cursors, never counts: filtering system events must not reset durable seen state.
pub fn changes(ctx: &Ctx, query: &ChangesQuery, seen: &SeenCursor) -> Result<ChangesPage> {
    if query.limit == 0 || query.limit > CHANGES_LIMIT_MAX {
        bail!("changes limit must be between 1 and {CHANGES_LIMIT_MAX}");
    }
    if matches!((query.after, query.before), (Some(after), Some(before)) if after >= before) {
        bail!("changes after must precede before");
    }
    ctx.read_step(|ctx| {
        let view = ctx.store.view()?;
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
        let owner_query = change_kinds::query();
        let total = view.count_events(&owner_query)?;
        let unseen = view.count_events(&EventQuery {
            after: Some(EventSeq(seen.through)),
            ..owner_query.clone()
        })?;
        let found = view.events(
            &EventQuery {
                after: query.after.map(EventSeq),
                before: query.before.map(EventSeq),
                newest_first: query.after.is_none(),
                ..owner_query.clone()
            },
            PageReq::first(u32::from(query.limit)),
        )?;
        let mut lines = Vec::with_capacity(found.items.len());
        let mut bytes = 1024usize;
        for event in found.items {
            let line = owner_line(&event)?;
            let size = json_len(&line)? + 1;
            if bytes + size > corral_protocol::app::APP_CONTROL_MAX - 2048 {
                break;
            }
            bytes += size;
            lines.push(line);
        }
        let older = if let Some(oldest) = lines.iter().map(|line| line.seq).min() {
            let preceding = view.events(
                &EventQuery {
                    before: Some(EventSeq(oldest)),
                    ..owner_query
                },
                PageReq::first(1),
            )?;
            (!preceding.items.is_empty()).then_some(oldest)
        } else {
            None
        };
        view.ensure_fresh()?;
        Ok(ChangesPage {
            lines,
            head,
            older,
            seen: seen.clone(),
            total: Some(total),
            unseen: Some(unseen),
        })
    })
}

fn owner_line(event: &StoredEvent) -> Result<ChangeLine> {
    let kind = change_kinds::KINDS
        .iter()
        .find(|(name, _)| *name == event.event.kind)
        .map_or(ChangeKind::Other, |(_, kind)| *kind);
    let mapped = crate::boardview::changes::change_line(event);
    let from = event
        .event
        .detail
        .get("from")
        .and_then(serde_json::Value::as_str)
        .map(|text| short(text, 128));
    let to = event
        .event
        .detail
        .get("to")
        .and_then(serde_json::Value::as_str)
        .map(|text| short(text, 128));
    // Task identity is a separate field; consumers must not prepend it twice.
    let text = if kind == ChangeKind::Moved {
        format!(
            "moved {} → {}",
            from.as_deref().unwrap_or("unknown"),
            to.as_deref().unwrap_or("unknown")
        )
    } else if mapped.class == crate::boardview::changes::ChangeClass::Other
        || matches!(kind, ChangeKind::Question | ChangeKind::Annotation)
    {
        event.event.kind.replace('_', " ")
    } else {
        let prefix = format!("{} · ", event.event.task.as_deref().unwrap_or("workspace"));
        mapped
            .text
            .strip_prefix(&prefix)
            .unwrap_or(&mapped.text)
            .to_owned()
    };
    let class = if kind == ChangeKind::Moved || kind == ChangeKind::Created {
        "work".into()
    } else if kind == ChangeKind::Plan {
        "plan".into()
    } else {
        serde_json::to_value(kind)?
            .as_str()
            .unwrap_or("other")
            .to_owned()
    };
    Ok(ChangeLine {
        seq: event.seq.0,
        at: event.event.ts.to_string(),
        class,
        kind,
        from,
        to,
        failed: event.event.kind.ends_with("_failed")
            || event.event.kind.ends_with("_error")
            || event
                .event
                .detail
                .get("result")
                .and_then(serde_json::Value::as_str)
                == Some("fail"),
        task: event.event.task.as_deref().map(|task| short(task, 512)),
        text: short(&text, 2048),
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
    })
}

#[cfg(test)]
#[path = "changes_tests.rs"]
mod tests;
