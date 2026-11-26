//! The open turn laid over the recorded transcript: its parts in the order they happened, until
//! the recorded page holds them, then the owner input the host holds and how the turn ended.

use std::collections::BTreeSet;

use corral_protocol::detail::{SessionItem, SessionMessage};

use crate::model::hue::Hue;
use crate::model::text::span;

use super::live::{LiveMessage, LiveSession};
use super::replay::{Cx, avatar, head, millis, thinking};
use super::{Marker, Message, MessageItem, MessageKind, More, Prose, group, tool};

/// Where a part of the turn stands: after which message, words before calls, then call order.
type Order = (u64, u8, usize);
/// A part of the turn under its key.
type Keyed = (String, MessageItem);

/// How many assistant messages of the live turn the recorded page already holds: those
/// written at or after the turn's start, by the host's own clock. Each live message is the
/// turn's n-th assistant message (its ordinal), so a recorded n-th retires the live one.
fn recorded_of_turn(turn: &LiveSession, recorded: &[SessionMessage]) -> u64 {
    let Some(started) = millis(turn.started_at.as_deref()) else {
        return 0;
    };
    recorded
        .iter()
        .filter(|message| message.author == "assistant")
        .filter(|message| millis(message.at.as_deref()).is_some_and(|at| at >= started))
        .count() as u64
}

/// The open turn laid over the recorded messages, its parts in the order they happened: each
/// message's thinking and words, then the calls it made, then the next message (the one still
/// being written streams); then the owner input the host holds and how the turn ended. A
/// message or tool call the recorded page already holds is drawn there only, and an ending the
/// recorded transcript states is not said twice.
pub(super) fn overlay(
    turn: &LiveSession,
    recorded: &[SessionMessage],
    cx: Cx<'_>,
) -> (Vec<Message>, Vec<MessageItem>) {
    let calls: BTreeSet<&str> = recorded
        .iter()
        .flat_map(|message| &message.items)
        .filter_map(|item| match item {
            SessionItem::ToolCall { call_id, .. } => Some(call_id.as_str()),
            _ => None,
        })
        .collect();
    let held = recorded_of_turn(turn, recorded);
    let base = format!(
        "live|{}|{}",
        turn.generation.unwrap_or(0),
        turn.turn.unwrap_or(0)
    );
    // A message stands before the calls it made; a call before the next message. Each part
    // is keyed by what it is, not where it stands, so the words being written keep their
    // view when the parts before them are recorded or fold into one row.
    let mut parts: Vec<(Order, Keyed)> = turn
        .messages
        .iter()
        .filter(|message| message.ordinal > held)
        .flat_map(|message| {
            let order = (message.ordinal, 0, 0);
            words(message, &base, cx)
                .into_iter()
                .map(move |part| (order, part))
        })
        .collect();
    parts.extend(
        turn.tools
            .iter()
            .enumerate()
            .filter(|(_, call)| !calls.contains(call.call.as_str()))
            .map(|(at, call)| {
                let key = format!("{base}|call:{}", call.call);
                ((call.after, 1, at), (key, tool::live(call, cx)))
            }),
    );
    // Stable: a message's thinking stays before its words.
    parts.sort_by_key(|(order, _)| *order);
    let (keys, items): (Vec<String>, Vec<MessageItem>) =
        parts.into_iter().map(|(_, part)| part).unzip();
    let items = group::fold(items, cx);
    // A folded run keeps the key of its first call.
    let mut each = keys.into_iter();
    let mut keys = Vec::with_capacity(items.len());
    for item in &items {
        keys.extend(each.next());
        if let MessageItem::Group(group) = item {
            each.by_ref().take(group.count - 1).for_each(drop);
        }
    }
    let live = if items.is_empty() {
        Vec::new()
    } else {
        let (avatar, hue) = avatar("assistant");
        let mut head = head("assistant", None);
        head.push(
            span(if turn.open {
                " · working now"
            } else {
                " · live"
            })
            .dim(),
        );
        vec![Message {
            id: String::new(),
            anchor: base.clone(),
            kind: MessageKind::Agent,
            avatar,
            hue,
            head,
            author: "assistant".into(),
            detail: String::new(),
            tag: None,
            tag_facts: None,
            items,
            keys,
            footer: None,
            live: true,
        }]
    };
    let mut after = Vec::new();
    if let Some(held) = queued(turn.steer, turn.follow_up) {
        after.push(MessageItem::Marker(Marker::quiet("→", Hue::Neutral, held)));
    }
    if let Some(end) = turn.ended.as_ref().filter(|_| !turn.open) {
        // The latest recorded assistant message says how its turn ended once it is recorded.
        let last = recorded
            .iter()
            .rev()
            .find(|message| message.author == "assistant")
            .map_or(&[][..], |message| message.items.as_slice());
        let recorded_as = |aborted: bool| {
            last.iter().any(|item| match item {
                SessionItem::Aborted { .. } => aborted,
                SessionItem::ProviderError { .. } => !aborted,
                _ => false,
            })
        };
        if let Some(error) = &end.error {
            if !recorded_as(false) {
                after.push(MessageItem::Marker(Marker::quiet(
                    "✗",
                    Hue::Fail,
                    error.clone(),
                )));
            }
        } else if end.aborted && !recorded_as(true) {
            after.push(MessageItem::Marker(Marker::quiet(
                "○",
                Hue::Neutral,
                "stopped".into(),
            )));
        }
    }
    (live, after)
}

/// One live message's thinking, then its words; streaming until the message is complete.
fn words(message: &LiveMessage, base: &str, cx: Cx<'_>) -> Vec<(String, MessageItem)> {
    let streaming = message.settled_at.is_none();
    let key = format!("{base}|{}", message.ordinal);
    let mut items = Vec::new();
    if !message.thinking.is_empty() {
        items.push((
            format!("{key}|thinking"),
            MessageItem::Thinking(thinking(
                &message.thinking,
                key.clone(),
                More::Whole,
                streaming,
                cx,
            )),
        ));
    }
    if !message.text.is_empty() {
        items.push((
            format!("{key}|text"),
            MessageItem::Prose(Prose {
                markdown: message.text.clone(),
                more: More::Whole,
                streaming,
            }),
        ));
    }
    items
}

/// Owner input the host holds: `1 message steers this turn · 2 wait for the next`.
fn queued(steer: u64, follow_up: u64) -> Option<String> {
    let steer = match steer {
        0 => None,
        1 => Some("1 message steers this turn".to_owned()),
        n => Some(format!("{n} messages steer this turn")),
    };
    let follow = match follow_up {
        0 => None,
        1 => Some("1 message waits for the next turn".to_owned()),
        n => Some(format!("{n} messages wait for the next turn")),
    };
    let words: Vec<String> = steer.into_iter().chain(follow).collect();
    (!words.is_empty()).then(|| format!("queued · {}", words.join(" · ")))
}
