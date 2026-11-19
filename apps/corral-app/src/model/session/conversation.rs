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

use super::super::composer::ModelPill;
use super::super::status::cost_text;
use super::super::transcript::{self, Transcript};
use super::super::{Action, Cx, DockAction, SubView, Tab, Ui};
use super::{
    CHILD_CONTROL, FleetAction, FleetButton, asks, card_of, child, child_detail, control, ctx,
    fleet, helper, live, ops, profile_model, scope, shown_id, shown_state,
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
    /// `slice · gpt-6` and `$0.21 · ctx 18%`: the facts before and after its running time,
    /// which is the clock's (`AgentsClock::time`); with no second part there is no time either.
    pub facts: (String, String),
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

/// What the bottom of the page reports while an agent's conversation is open, in place of the
/// session's own agent: the agent's model and effort in the composer, its id, cost and context
/// share in the status bar. Each is what the fleet (or the agent's served detail) reports; a
/// fact neither reports stays absent, never the parent's.
#[derive(Debug, Clone, PartialEq)]
pub struct OpenedAgent {
    /// The agent's id in the fleet.
    pub child: String,
    /// `a2`, a helper call's id shortened.
    pub id: String,
    pub model: ModelPill,
    pub effort: Option<String>,
    pub cost_usd: Option<f64>,
    pub context_percent: Option<f32>,
}

/// The agent whose conversation the Agents tab shows, while the fleet still lists it.
pub(in crate::model::session) fn opened_agent(cx: &Cx<'_>) -> Option<OpenedAgent> {
    if cx.ui.tab != Tab::Agents {
        return None;
    }
    let Some(SubView::Agent(id)) = cx.ui.sub.as_ref() else {
        return None;
    };
    let row = fleet(cx)?.children.iter().find(|child| child.id == *id)?;
    let served = child_detail(cx.feed, cx.id, id);
    let unavailable = if helper(row) {
        Some("a helper call keeps the model it was started with".to_owned())
    } else if !live(row) {
        Some("it has finished".to_owned())
    } else {
        control(cx, CHILD_CONTROL)
            .or_else(|| ops::sending(cx, id).then(|| "your last request is on its way".into()))
    };
    let picking = cx.ui.fleet.picking.as_deref() == Some(id.as_str());
    Some(OpenedAgent {
        child: id.clone(),
        id: shown_id(row),
        model: ModelPill {
            label: row.model.clone(),
            pending: row
                .pending_model
                .clone()
                .or_else(|| served.and_then(|detail| detail.pending_model.clone())),
            unavailable,
            opens: Some(Action::Fleet(FleetAction::Models {
                session: cx.id.clone(),
                child: (!picking).then(|| id.clone()),
            })),
        },
        effort: row
            .effort
            .clone()
            .or_else(|| served.and_then(|detail| detail.effort.clone())),
        cost_usd: row
            .cost_usd
            .or_else(|| served.and_then(|detail| detail.usage.cost_usd)),
        context_percent: row
            .context_percent
            .or_else(|| served.and_then(|detail| detail.usage.context_percent)),
    })
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
            facts: (
                "The host's fleet no longer lists this agent; its transcript is what was recorded."
                    .into(),
                String::new(),
            ),
            transcript,
            end: None,
            receiver: None,
            buttons: Vec::new(),
        };
    };
    let (mark, hue, state) = shown_state(row);
    let facts = (
        profile_model(row),
        [cost_text(row.cost_usd, false), format!("ctx {}", ctx(row))].join(" · "),
    );
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
