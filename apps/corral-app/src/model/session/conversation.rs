//! An agent's conversation, opened from the Agents tab (`Agents › a2`): its head (the way back,
//! its state, profile and model, time, cost and context share), its own transcript read through
//! the one session transcript model under the scope `<session>/child/<child>` (its brief, replies
//! and tool rows, with the owner's steers and the messages between agents placed by time), how
//! it stands at the end (not started, asking, cancelled, failed), and its controls. What the
//! composer sends goes to the agent while it is the receiver.

use corral_protocol::detail::DetailRef;
use corral_protocol::subscription::SessionChild;

use crate::model::Id;
use crate::model::board::Mark;
use crate::model::hue::Hue;

use super::super::status::cost_text;
use super::super::transcript::{self, Transcript};
use super::super::{Action, Cx, DockAction, Ui};
use super::{
    FleetAction, FleetButton, asks, card_of, child, child_detail, ctx, profile_model, scope,
    shown_state, time,
};

#[derive(Debug, Clone, PartialEq)]
pub struct Conversation {
    pub child: String,
    /// The transcript's scope, `<session>/child/<child>`.
    pub scope: Id,
    /// `‹ Agents`.
    pub back: Action,
    pub mark: Mark,
    pub hue: Hue,
    pub state: &'static str,
    pub title: String,
    /// `slice · gpt-6 · 1m 02s · $0.21 · ctx 18%`.
    pub facts: String,
    pub transcript: Transcript,
    /// How it stands after its last message.
    pub end: Option<EndLine>,
    /// `What you send goes to a2`, while it is the composer's receiver.
    pub receiver: Option<String>,
    pub buttons: Vec<FleetButton>,
}

/// `▲ Which cache? · answer in the card`, `✗ Cancelled. …`.
#[derive(Debug, Clone, PartialEq)]
pub struct EndLine {
    pub hue: Hue,
    pub words: String,
    pub action: Option<Action>,
}

fn end(cx: &Cx<'_>, child: &SessionChild) -> Option<EndLine> {
    let line = |hue, words: String, action| Some(EndLine { hue, words, action });
    if asks(child) {
        let question = child_detail(cx.feed, cx.id, &child.id)
            .and_then(|detail| detail.question.as_ref())
            .map_or_else(
                || "It waits for your answer".to_owned(),
                |question| question.question.clone(),
            );
        let card = card_of(cx, &child.id);
        let words = if card.is_some() {
            format!("▲ {question} · answer in the card")
        } else {
            format!("▲ {question} · its card has not reached this device yet")
        };
        return line(
            Hue::You,
            words,
            card.map(|card| {
                Action::Dock(DockAction::Bring {
                    session: cx.id.clone(),
                    card,
                })
            }),
        );
    }
    if let Some(error) = &child.error {
        return line(Hue::Fail, format!("✗ {error}"), None);
    }
    match child.state.as_str() {
        "queued" if child.after.is_empty() => line(
            Hue::Neutral,
            "Not started: it waits for a free slot.".into(),
            None,
        ),
        "queued" => line(
            Hue::Neutral,
            format!("Not started: it waits for {}.", child.after.join(", ")),
            None,
        ),
        "cancelled" => line(
            Hue::Neutral,
            "✗ Cancelled. Its edits so far stay in the worktree.".into(),
            None,
        ),
        "failed" => line(
            Hue::Fail,
            "✗ It failed without a reason recorded.".into(),
            None,
        ),
        "done" => line(Hue::Ok, "✓ Done.".into(), None),
        _ => None,
    }
}

/// The conversation of agent `id`; `row` is its fleet row (none once the fleet no longer lists
/// it), `ui` the conversation scope's own UI state (what its rows have open).
pub(super) fn conversation(
    cx: &Cx<'_>,
    ui: &Ui,
    id: &str,
    row: Option<&SessionChild>,
) -> Conversation {
    let scope = scope(cx.id, id);
    let cache = cx.feed.detail(&DetailRef::Session {
        id: scope.to_string(),
    });
    let transcript = transcript::transcript(&scope, ui, cache, cx.feed);
    let back = Action::Fleet(FleetAction::Leave(cx.id.clone()));
    let Some(row) = row else {
        return Conversation {
            child: id.to_owned(),
            scope,
            back,
            mark: Mark::Na,
            hue: Hue::Neutral,
            state: "no longer listed",
            title: id.to_owned(),
            facts:
                "The host's fleet no longer lists this agent; its transcript is what was recorded."
                    .into(),
            transcript,
            end: None,
            receiver: None,
            buttons: Vec::new(),
        };
    };
    let (mark, hue, state) = shown_state(row);
    let facts = [
        profile_model(row),
        time(row, cx.feed.now),
        cost_text(row.cost_usd, false),
        format!("ctx {}", ctx(row)),
    ]
    .join(" · ");
    Conversation {
        child: id.to_owned(),
        scope,
        back,
        mark,
        hue,
        state,
        title: row.title.clone(),
        facts,
        transcript,
        end: end(cx, row),
        receiver: (cx.ui.to.as_deref() == Some(id)).then(|| format!("What you send goes to {id}")),
        buttons: child::buttons(cx, row, true),
    }
}

#[cfg(test)]
#[path = "conversation_tests.rs"]
mod tests;
