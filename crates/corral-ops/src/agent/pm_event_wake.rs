//! Board events and `attention_due` for a bound corral-pi PM, published as keyed wakes into
//! the PM inbox and delivered by the Pi adapter instead of pasted into the pane.

use std::collections::BTreeMap;

use anyhow::Result;
use corral_core::agent::AgentPm;
use corral_core::inbox::{Inbox, MessageBinding, Post, Recipient, Urgency};
use corral_core::records::RecordArea;
use corral_core::{Event, EventCursor, actionable_kind, format_event_line};
use serde_json::json;

use crate::Ctx;

const STATE: RecordArea<EventCursor> = RecordArea::new("pm-event-wakes");
const POINTER: &str = "A wake is a pointer, not authority. Read fresh brief/next before acting.\n";
// Leave room for inbox framing (including worst-case sequence numbers) and the omission count.
const BODY_BUDGET: usize = super::delivery::INPUT_MAX_BYTES - 512;

/// While the native route is unavailable, skip history rather than replay it on binding.
/// The caller holds the shared `pm-events` lock for both routes.
pub(crate) fn checkpoint(ctx: &Ctx) -> Result<()> {
    STATE.save(&ctx.store, "state", ctx.store.events_since(None)?.cursor())
}

/// Publish before advancing; transport failure never holds the durable event cursor.
pub(crate) fn push(ctx: &Ctx, pm: AgentPm) -> Result<()> {
    let cursor = match STATE.load(&ctx.store, "state")? {
        Some(cursor) => cursor,
        None => {
            checkpoint(ctx)?;
            return Ok(());
        }
    };
    let mut reader = ctx.store.events_since(Some(cursor))?;
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
        return STATE.save(&ctx.store, "state", reader.cursor());
    }
    let inbox = Inbox::new(&ctx.store, Recipient::Pm)?;
    let posted = publish_pass(&inbox, &events)?;
    STATE.save(&ctx.store, "state", reader.cursor())?;
    if posted > 0 {
        deliver(ctx, pm)?;
    }
    Ok(())
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
            actionable_kind(event).unwrap_or(&event.kind),
        ));
    }
    keys
}

fn publish_pass(inbox: &Inbox<'_>, events: &[Event]) -> Result<usize> {
    let mut posted = 0;
    // Urgent events precede the routine digest even when they occurred later in the pass.
    for event in events.iter().filter(|event| urgent(event)) {
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
            inbox,
            &key,
            &causes,
            event.seq,
            event.seq,
            &format!("{POINTER}{line}"),
        )?);
    }
    let routine: Vec<_> = events.iter().filter(|event| !urgent(event)).collect();
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
        inbox,
        &format!("pmev:{}-{}", first.seq, last.seq),
        &grouped.into_keys().collect::<Vec<_>>(),
        first.seq,
        last.seq,
        &body,
    )?);
    Ok(posted)
}

/// The subject is the durable dedupe record if a crash loses the cursor write.
fn publish(
    inbox: &Inbox<'_>,
    key: &str,
    causes: &[String],
    first_seq: u64,
    last_seq: u64,
    body: &str,
) -> Result<bool> {
    if inbox.messages()?.into_iter().any(|message| {
        message.kind == "pm_event_wake"
            && message
                .subject
                .as_ref()
                .is_some_and(|subject| subject["key"] == key)
    }) {
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
}

#[cfg(all(test, unix))]
#[path = "pm_event_wake_tests.rs"]
mod tests;
