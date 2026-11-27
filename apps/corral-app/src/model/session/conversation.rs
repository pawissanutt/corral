//! An agent's conversation, opened from the Agents tab (`Agents › a2`): its head (the way back,
//! its state, profile and model, time, cost and context share), its own transcript read through
//! the one session transcript model under the scope `<session>/child/<child>` (its brief, replies
//! and tool rows, with the owner's steers and the messages between agents placed by time), how
//! it stands at the end (not started, asking, cancelled, failed), and its controls. What the
//! composer sends goes to the agent while it is the receiver. An agent the fleet does not list
//! (opened by its id) is read-only and says what corrald recorded for it, or that it recorded
//! nothing under that id.

use corral_protocol::detail::DetailRef;
use corral_protocol::subscription::SessionChild;

use crate::model::Id;
use crate::model::board::Mark;
use crate::model::daemon::{ClientError, DetailCache};
use crate::model::hue::Hue;

use super::super::activity::{self, ActivityLine};
use super::super::composer::ModelPill;
use super::super::status::cost_text;
use super::super::transcript::{self, Transcript};
use super::super::{Action, Cx, DockAction, SessionRole, SubView, Tab, Ui};
use super::{
    CHILD_CONTROL, FleetAction, FleetButton, asks, card_of, child, child_detail, child_state,
    child_target, control, ctx, fleet, helper, live, ops, profile_model, scope, shown_id,
    shown_state,
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
    match child.state.as_str() {
        _ if child.error.is_some() => ended(&child.state, child.error.as_deref()),
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
        state => ended(state, None),
    }
}

/// How an agent that finished stands: its error, or the end its state says.
fn ended(state: &str, error: Option<&str>) -> Option<EndLine> {
    let line = |hue, words: String| {
        Some(EndLine {
            hue,
            words,
            action: None,
        })
    };
    if let Some(error) = error {
        return line(Hue::Fail, format!("✗ {error}"));
    }
    match state {
        "cancelled" => line(
            Hue::Neutral,
            "✗ Cancelled. Its edits so far stay in the worktree.".into(),
        ),
        "failed" => line(Hue::Fail, "✗ It failed without a reason recorded.".into()),
        "done" => line(Hue::Ok, "✓ Done.".into()),
        _ => None,
    }
}

/// The head of an agent the live fleet does not list: what corrald recorded for it, or that it
/// holds nothing under this id. An id typed by the owner names no agent until corrald serves
/// one, and none of it is live: no controls, no receiver.
fn unlisted(cx: &Cx<'_>, id: &str, transcript: Option<&DetailCache>) -> Unlisted {
    let detail = cx.feed.detail(&child_target(cx.id, id));
    if let Some(served) = child_detail(cx.feed, cx.id, id) {
        let (mark, hue, state) = child_state(&served.state);
        let lead = [served.profile.as_str(), served.model.as_str()]
            .into_iter()
            .filter(|fact| !fact.is_empty())
            .collect::<Vec<_>>()
            .join(" · ");
        let said = "as recorded · not in the host's live list";
        return Unlisted {
            mark,
            hue,
            state,
            title: if served.title.is_empty() {
                id.to_owned()
            } else {
                served.title.clone()
            },
            facts: if lead.is_empty() {
                said.to_owned()
            } else {
                format!("{lead} · {said}")
            },
            end: ended(&served.state, served.error.as_deref()),
        };
    }
    let plain = |mark, hue, state, facts: String| Unlisted {
        mark,
        hue,
        state,
        title: id.to_owned(),
        facts,
        end: None,
    };
    if transcript.is_some_and(|cache| cache.body().is_some()) {
        return plain(
            Mark::Na,
            Hue::Neutral,
            "recorded",
            "Not in the host's live list · this is its recorded transcript".into(),
        );
    }
    let errors: Vec<&ClientError> = [detail, transcript]
        .into_iter()
        .flatten()
        .filter_map(|cache| cache.error.as_ref())
        .collect();
    if errors.iter().any(|error| error.code == "not_found") {
        return plain(
            Mark::Na,
            Hue::Neutral,
            "not recorded",
            format!(
                "corrald holds no agent {id} for this session. Check the id in the parent's transcript."
            ),
        );
    }
    if let Some(error) = errors.first() {
        return plain(
            Mark::Fail,
            Hue::Fail,
            "unreadable",
            format!("Its record could not be read: {}", error.message),
        );
    }
    plain(
        Mark::Next,
        Hue::Neutral,
        "reading",
        "Reading what corrald recorded under this id…".into(),
    )
}

/// What `unlisted` says of an agent the fleet does not list.
struct Unlisted {
    mark: Mark,
    hue: Hue,
    state: &'static str,
    title: String,
    facts: String,
    end: Option<EndLine>,
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
    pub activity: Option<ActivityLine>,
}

fn child_activity(state: &str, activity: Option<&str>) -> Option<ActivityLine> {
    let (phase, hue, glyph, spinner) = match state {
        "running" => (
            activity
                .filter(|text| !text.trim().is_empty())
                .unwrap_or("working"),
            Hue::Worker,
            None,
            true,
        ),
        "needs" => ("waiting for your answer", Hue::You, Some("▲"), false),
        "queued" => ("waiting to start", Hue::Held, None, false),
        "stopping" => ("waiting for cleanup", Hue::Held, None, false),
        _ => return None,
    };
    Some(ActivityLine {
        spinner,
        glyph,
        hue,
        phase: if spinner {
            format!("Working · {phase}")
        } else {
            phase.to_owned()
        },
        short: activity::short(phase),
        elapsed: None,
    })
}

/// The child conversation on screen, inline in Agents or opened under its own scope.
pub(in crate::model::session) fn opened_agent(cx: &Cx<'_>) -> Option<OpenedAgent> {
    let standalone = *cx.role == SessionRole::Child;
    let (parent, id, fleet) = if standalone {
        let (parent, id) = cx.id.as_str().split_once("/child/")?;
        if parent.is_empty() || id.is_empty() {
            return None;
        }
        let parent = Id::from(parent);
        let fleet = cx
            .feed
            .live
            .get(&parent)
            .and_then(|live| live.fleet.as_ref());
        (parent, id, fleet)
    } else {
        let Some(SubView::Agent(id)) = cx.ui.sub.as_ref().filter(|_| cx.ui.tab == Tab::Agents)
        else {
            return None;
        };
        (cx.id.clone(), id.as_str(), fleet(cx))
    };
    let row = fleet.and_then(|fleet| fleet.children.iter().find(|child| child.id == id));
    let served = child_detail(cx.feed, &parent, id);
    let (unavailable, opens) = match row.filter(|_| !standalone) {
        Some(row) => {
            let unavailable = if helper(row) {
                Some("a helper call keeps the model it was started with".to_owned())
            } else if !live(row) {
                Some("it has finished".to_owned())
            } else {
                control(cx, CHILD_CONTROL).or_else(|| {
                    ops::sending(cx, id).then(|| "your last request is on its way".into())
                })
            };
            let picking = cx.ui.fleet.picking.as_deref() == Some(id);
            (
                unavailable,
                Some(Action::Fleet(FleetAction::Models {
                    session: parent,
                    child: (!picking).then(|| id.to_owned()),
                })),
            )
        }
        None => (Some("this conversation is read-only".into()), None),
    };
    let state = row
        .map(|row| row.state.as_str())
        .or_else(|| served.map(|detail| detail.state.as_str()));
    let activity = row
        .and_then(|row| {
            row.activity
                .as_deref()
                .filter(|text| !text.trim().is_empty())
        })
        .or_else(|| served.and_then(|detail| detail.activity.as_deref()));
    Some(OpenedAgent {
        child: id.to_owned(),
        id: row.map_or_else(|| id.to_owned(), shown_id),
        model: ModelPill {
            label: row
                .map(|row| row.model.as_str())
                .filter(|model| !model.trim().is_empty())
                .or_else(|| {
                    served
                        .map(|detail| detail.model.as_str())
                        .filter(|model| !model.trim().is_empty())
                })
                .unwrap_or("Model unavailable")
                .to_owned(),
            pending: row
                .and_then(|row| row.pending_model.clone())
                .or_else(|| served.and_then(|detail| detail.pending_model.clone())),
            unavailable,
            opens,
        },
        effort: row
            .and_then(|row| row.effort.clone())
            .or_else(|| served.and_then(|detail| detail.effort.clone())),
        cost_usd: row
            .and_then(|row| row.cost_usd)
            .or_else(|| served.and_then(|detail| detail.usage.cost_usd)),
        context_percent: row
            .and_then(|row| row.context_percent)
            .or_else(|| served.and_then(|detail| detail.usage.context_percent)),
        activity: state.and_then(|state| child_activity(state, activity)),
    })
}

/// The conversation of agent `id`; `row` is its fleet row (none when the fleet does not list
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
        let said = unlisted(cx, id, cache);
        return Conversation {
            child: id.to_owned(),
            scope,
            back,
            mark: said.mark,
            hue: said.hue,
            state: said.state,
            title: said.title,
            facts: (said.facts, String::new()),
            transcript,
            end: said.end,
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
