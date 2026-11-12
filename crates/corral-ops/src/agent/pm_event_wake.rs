//! Board event wakes are durable even while the native PM is lost. A frozen batch survives
//! publication/checkpoint crashes; host health controls transport and flood control only.

use std::collections::BTreeMap;

use anyhow::Result;
use corral_core::agent::AgentPm;
use corral_core::inbox::{Inbox, MessageBinding, Post, Recipient, Urgency};
use corral_core::records::RecordArea;
use corral_core::{Event, EventCursor, actionable_kind, format_event_line};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::Ctx;

const STATE: RecordArea<CursorState> = RecordArea::new("pm-event-wakes");
const POINTER: &str = "A wake is a pointer, not authority. Read fresh brief/next before acting.\n";
// Leave room for inbox framing (including worst-case sequence numbers) and the omission count.
const BODY_BUDGET: usize = super::delivery::INPUT_MAX_BYTES - 512;

#[derive(Serialize, Deserialize)]
struct CursorState {
    // Preserve the original cursor document's shape for records written before frozen batches.
    #[serde(flatten)]
    cursor: EventCursor,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pending: Option<FrozenBatch>,
}

#[derive(Serialize, Deserialize)]
struct FrozenBatch {
    from: EventCursor,
    to: EventCursor,
    key: String,
    // Snapshot selection and content too: config changes and log retention cannot change a replay.
    events: Vec<Event>,
    urgent_separate: bool,
}

/// Only the non-native route skips history. The caller holds the shared `pm-events` lock.
pub(crate) fn checkpoint(ctx: &Ctx) -> Result<()> {
    let tail = ctx.store.events_since(None)?.cursor().clone();
    if STATE
        .load(&ctx.store, "state")?
        .is_some_and(|state| state.cursor == tail && state.pending.is_none())
    {
        return Ok(());
    }
    STATE.save(
        &ctx.store,
        "state",
        &CursorState {
            cursor: tail,
            pending: None,
        },
    )
}

/// Publication never depends on health; only a healthy bound PM may receive native input.
pub(crate) fn push(ctx: &Ctx, pm: AgentPm) -> Result<()> {
    let Some(mut state) = STATE.load(&ctx.store, "state")? else {
        // First binding starts at EOF, not at sequence zero.
        return checkpoint(ctx);
    };
    let healthy = super::pm_wake::healthy(&pm);
    let inbox = Inbox::new(&ctx.store, Recipient::Pm)?;
    if state.pending.is_none() {
        if !healthy
            && inbox
                .pending_messages()?
                .iter()
                .any(|message| message.kind == "pm_event_wake")
        {
            // One unobserved wake already requests recovery. Hold the range for catch-up.
            return Ok(());
        }
        freeze(ctx, &mut state, healthy)?;
    }
    let Some(batch) = state.pending.as_ref() else {
        return Ok(());
    };
    let posted = publish_pass(ctx, &inbox, batch)?;
    state.cursor = batch.to.clone();
    state.pending = None;
    ctx.store.records_transaction(|| {
        STATE.save(&ctx.store, "state", &state)?;
        // After the cursor commits no batch at or below it can replay; only live keys remain.
        super::pm_wake_keys::retain(ctx, "event", &|_| false)
    })?;
    if posted > 0 && healthy {
        deliver(ctx, pm)?;
    }
    Ok(())
}

fn freeze(ctx: &Ctx, state: &mut CursorState, healthy: bool) -> Result<()> {
    let mut reader = ctx.store.events_since(Some(state.cursor.clone()))?;
    let mut events = Vec::new();
    for _ in 0..256 {
        let before = reader.read_position();
        let Some((event, _)) = reader.next_event()? else {
            if reader.read_position() == before {
                break;
            }
            continue;
        };
        if selected(ctx, &event) {
            events.push(event);
        }
    }
    if events.is_empty() {
        // A genuinely quiet pass performs no write. Deliberately filtered events can advance.
        if &state.cursor != reader.cursor() {
            state.cursor = reader.cursor().clone();
            STATE.save(&ctx.store, "state", state)?;
        }
        return Ok(());
    }
    let digest_events: Vec<_> = events
        .iter()
        .filter(|event| !healthy || !urgent(event))
        .collect();
    let first = digest_events
        .first()
        .map_or(events[0].seq, |event| event.seq);
    let last = digest_events
        .last()
        .map_or(events[events.len() - 1].seq, |event| event.seq);
    state.pending = Some(FrozenBatch {
        from: state.cursor.clone(),
        to: reader.cursor().clone(),
        key: format!("pmev:{first}-{last}"),
        events,
        urgent_separate: healthy,
    });
    // This commit precedes every inbox post. Replays cannot grow the batch boundary.
    STATE.save(&ctx.store, "state", state)
}

fn deliver(ctx: &Ctx, pm: AgentPm) -> Result<()> {
    if let Err(error) =
        crate::delivery::deliver(ctx, &Recipient::Pm, &super::pm_wake::ScopedPm(pm), None)
    {
        crate::messages::report(
            None,
            "PM event wake",
            format!("delivery deferred: {error:#}"),
        );
    }
    Ok(())
}

fn selected(ctx: &Ctx, event: &Event) -> bool {
    if event.kind == "attention_due" {
        return true;
    }
    let kinds = &ctx.store.config.pm.push_kinds;
    let defaults = kinds
        .iter()
        .map(String::as_str)
        .eq(corral_core::PM_DEFAULT_KINDS.iter().copied());
    (!defaults || corral_core::default_pm_event(event))
        && actionable_kind(event).is_some_and(|kind| kinds.iter().any(|k| k == kind))
}

fn urgent(event: &Event) -> bool {
    matches!(
        actionable_kind(event),
        Some("escalated" | "question_escalated" | "needs_me" | "blocked")
    )
}

fn causes(event: &Event) -> Vec<String> {
    let mut keys: Vec<String> = event.detail["question_keys"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|key| key.as_str().map(|key| format!("q:{key}")))
        .collect();
    if let Some(key) = event.detail["question_key"].as_str().or_else(|| {
        event
            .kind
            .starts_with("question_")
            .then(|| event.detail["key"].as_str())
            .flatten()
    }) {
        keys.push(format!("q:{key}"));
    }
    keys.sort();
    keys.dedup();
    if keys.is_empty() {
        keys.push(format!(
            "cond:{}:{}",
            event.task.as_deref().unwrap_or("workspace"),
            actionable_kind(event).unwrap_or(&event.kind)
        ));
    }
    keys
}

fn publish_pass(ctx: &Ctx, inbox: &Inbox<'_>, batch: &FrozenBatch) -> Result<usize> {
    let mut posted = 0;
    for event in batch
        .events
        .iter()
        .filter(|event| batch.urgent_separate && urgent(event))
    {
        let causes = causes(event);
        let key = format!("{}#{}", causes[0], event.seq);
        let mut line = format_event_line(event);
        let limit = BODY_BUDGET - POINTER.len() - 32;
        if line.len() > limit {
            let mut end = limit;
            while !line.is_char_boundary(end) {
                end -= 1;
            }
            line.truncate(end);
            line.push_str(" [truncated]");
        }
        posted += usize::from(publish(
            ctx,
            inbox,
            &key,
            &causes,
            event.seq,
            event.seq,
            &format!("{POINTER}{line}"),
        )?);
    }
    let routine: Vec<_> = batch
        .events
        .iter()
        .filter(|event| !batch.urgent_separate || !urgent(event))
        .collect();
    let (Some(first), Some(last)) = (routine.first(), routine.last()) else {
        return Ok(posted);
    };
    let mut grouped: BTreeMap<String, (&Event, usize)> = BTreeMap::new();
    for event in &routine {
        for cause in causes(event) {
            let entry = grouped.entry(cause).or_insert((event, 0));
            entry.0 = event;
            entry.1 += 1;
        }
    }
    let mut body = POINTER.to_owned();
    let mut omitted = 0;
    for (event, count) in grouped.values() {
        let line = if *count > 1 {
            format!("{} (count: {count})\n", format_event_line(event))
        } else {
            format!("{}\n", format_event_line(event))
        };
        if body.len() + line.len() <= BODY_BUDGET {
            body.push_str(&line);
        } else {
            omitted += 1;
        }
    }
    if omitted > 0 {
        body.push_str(&format!(
            "{omitted} causes omitted; read fresh brief/next.\n"
        ));
    }
    posted += usize::from(publish(
        ctx,
        inbox,
        &batch.key,
        &grouped.into_keys().collect::<Vec<_>>(),
        first.seq,
        last.seq,
        &body,
    )?);
    Ok(posted)
}

fn publish(
    ctx: &Ctx,
    inbox: &Inbox<'_>,
    key: &str,
    causes: &[String],
    first_seq: u64,
    last_seq: u64,
    body: &str,
) -> Result<bool> {
    ctx.store.records_transaction(|| {
        if !super::pm_wake_keys::claim(ctx, "event", key)? {
            return Ok(false);
        }
        inbox.post(Post {
            kind: "pm_event_wake",
            urgency: Urgency::Wake,
            author: "corral",
            body,
            binding: MessageBinding::default(),
            subject: Some(
                json!({"key": key, "causes": causes, "first_seq": first_seq, "last_seq": last_seq}),
            ),
            max_payload: super::delivery::INPUT_MAX_BYTES,
        })?;
        Ok(true)
    })
}

#[cfg(all(test, unix))]
#[path = "pm_event_wake_tests.rs"]
mod tests;
