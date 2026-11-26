//! Ephemeral turn notices that are not transcript messages.
use corral_protocol::detail::{SessionItem, SessionMessage};

use crate::model::hue::Hue;

use super::{LiveSession, Marker, MessageItem};

pub(super) fn notices(turn: &LiveSession, recorded: &[SessionMessage]) -> Vec<MessageItem> {
    let mut after = Vec::new();
    if let Some(held) = queued(turn.steer, turn.follow_up) {
        after.push(MessageItem::Marker(Marker::quiet("→", Hue::Neutral, held)));
    }
    if let Some(end) = turn.ended.as_ref().filter(|_| !turn.open) {
        let last = recorded
            .iter()
            .rev()
            .find(|message| message.author == "assistant")
            .map_or(&[][..], |message| message.items.as_slice());
        if let Some(error) = &end.error {
            if !last
                .iter()
                .any(|item| matches!(item, SessionItem::ProviderError { .. }))
            {
                after.push(MessageItem::Marker(Marker::quiet(
                    "✗",
                    Hue::Fail,
                    error.clone(),
                )));
            }
        } else if end.aborted
            && !last
                .iter()
                .any(|item| matches!(item, SessionItem::Aborted { .. }))
        {
            after.push(MessageItem::Marker(Marker::quiet(
                "○",
                Hue::Neutral,
                "stopped".into(),
            )));
        }
    }
    after
}

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
