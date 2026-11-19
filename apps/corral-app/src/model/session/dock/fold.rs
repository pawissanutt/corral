//! Which waiting cards the dock shows open. A docked choice (an exit choice, a child's question,
//! a proposed board plan) is one line until the owner opens it: by pressing its line, Alt+A,
//! Alt+↓, the needs-you chip, or from elsewhere (Bring); Not now and Esc fold it back. The exit
//! choice Finish… asked for opens by itself once the host docks it. Alt+↓ also unfolds the queue
//! when it gives it the keys.

use corral_protocol::detail::CardKind;
use corral_protocol::session_state::{CardBrief, SessionState};

use crate::model::Id;
use crate::model::daemon::Feed;
use crate::model::effect::Effect;
use crate::model::hue::Hue;
use crate::model::nav::Form;
use crate::model::session::choice::{self, ChoiceCard};
use crate::model::session::keys::{KeyAction, KeyBinding, bind};
use crate::model::session::{Action, Cx, Focus, State, Ui};

use super::{AsideLine, DockAction, briefs, card, decision, docked, kind, leave, send, sending};

/// At most this many folded lines show; the rest are counted.
const LINES: usize = 3;

/// Brings a waiting card forward: a question's answer card opens, a docked choice opens, a
/// decision card takes the keys.
pub(super) fn bring(state: &mut State, session: &Id, card: String, feed: &Feed) {
    let kind = kind(feed, session.as_str(), &card);
    let ui = state.ui_mut(session);
    match kind {
        Some(CardKind::Question) => {
            ui.dock.open = Some(card);
            ui.focus = Focus::Questions;
        }
        Some(kind) if docked(kind) => {
            ui.dock.opened.insert(card);
            ui.focus = Focus::Dock;
        }
        Some(kind) if decision(kind) => ui.focus = Focus::Dock,
        _ => {}
    }
}

/// Answers nothing: a decision card resolves as no; a docked choice folds back to its line and
/// stays pending; a question's card closes.
pub(super) fn dismiss(state: &mut State, session: &Id, card: String, feed: &Feed) -> Vec<Effect> {
    let now = feed.now;
    match kind(feed, session.as_str(), &card) {
        Some(kind) if decision(kind) => {
            if sending(state.ui(session.as_str()), feed, &card) {
                return Vec::new();
            }
            send(state, session, &card, Vec::new(), true)
        }
        Some(kind) if docked(kind) => {
            let ui = state.ui_mut(session);
            if ui
                .dock
                .compose
                .as_ref()
                .is_some_and(|mode| mode.card() == card)
            {
                leave(ui, now);
            }
            ui.dock.opened.remove(&card);
            if kind == CardKind::Exit {
                ui.dock.exit_wanted = false;
            }
            ui.focus = Focus::Composer;
            let words = if kind == CardKind::Exit {
                "The exit choice is waiting · Finish to reopen"
            } else {
                "The choice is still waiting · press its line to reopen"
            };
            ui.notices.push(Hue::Neutral, words, now);
            Vec::new()
        }
        _ => {
            let ui = state.ui_mut(session);
            ui.dock.open = None;
            ui.focus = Focus::Composer;
            Vec::new()
        }
    }
}

/// The focus order Alt+↓ walks, keeping only the items the dock shows. The item that takes the
/// keys opens: the queue unfolds, the first docked choice opens.
pub(super) fn next(state: &mut State, session: &Id, feed: &Feed) {
    let briefs = briefs(feed, session.as_str());
    let ui = state.ui(session.as_str());
    let queued = feed
        .live
        .get(session.as_str())
        .and_then(|live| live.state.as_ref())
        .is_some_and(|state| state.queue_total > 0);
    let shown = |focus: Focus| match focus {
        Focus::Queue => queued,
        Focus::Questions => briefs.iter().any(|brief| brief.kind == CardKind::Question),
        Focus::Dock => briefs
            .iter()
            .any(|brief| decision(brief.kind) || docked(brief.kind)),
        _ => true,
    };
    let order = [Focus::Composer, Focus::Queue, Focus::Questions, Focus::Dock];
    let at = order
        .iter()
        .position(|focus| *focus == ui.focus)
        .unwrap_or(0);
    let focus = (1..=order.len())
        .map(|step| order[(at + step) % order.len()])
        .find(|focus| shown(*focus))
        .unwrap_or(Focus::Composer);
    let first = briefs
        .iter()
        .find(|brief| docked(brief.kind))
        .map(|brief| brief.id.clone());
    let ui = state.ui_mut(session);
    ui.focus = focus;
    match focus {
        Focus::Queue => ui.dock.queue_open = Some(true),
        Focus::Dock => ui.dock.opened.extend(first),
        _ => {}
    }
}

/// Follows the host's cards: forgets opened cards that no longer wait, opens the exit choice
/// Finish… asked for once it docks, and lets an emptied queue fold by the rule again.
pub(super) fn settle(ui: &mut Ui, state: &SessionState) {
    ui.dock
        .opened
        .retain(|card| state.cards.iter().any(|brief| brief.id == *card));
    if ui.dock.exit_wanted
        && let Some(exit) = state
            .cards
            .iter()
            .find(|brief| brief.kind == CardKind::Exit)
    {
        ui.dock.opened.insert(exit.id.clone());
        ui.dock.exit_wanted = false;
        ui.focus = Focus::Dock;
    }
    if state.queue_total == 0 {
        ui.dock.queue_open = None;
    }
}

/// The docked choices of `cards`: the first the owner opened, and every other one as a line (up
/// to three, then `+n more`).
pub(super) fn docked_cards(
    cx: &Cx<'_>,
    cards: &[CardBrief],
    inert: Option<String>,
) -> (Option<ChoiceCard>, Vec<AsideLine>, Option<String>) {
    let mut opened = None;
    let mut lines = Vec::new();
    for brief in cards.iter().filter(|brief| docked(brief.kind)) {
        if opened.is_none() && cx.ui.dock.opened.contains(&brief.id) {
            let body = card(cx.feed, cx.id.as_str(), &brief.id);
            opened = Some(choice::card(cx, brief, body, inert.clone()));
        } else {
            let oldest = cards.first().is_some_and(|first| first.id == brief.id);
            lines.push(choice::line(cx, brief, oldest));
        }
    }
    let more = (lines.len() > LINES).then(|| format!("+{} more", lines.len() - LINES));
    lines.truncate(LINES);
    (opened, lines, more)
}

/// Esc on the page while the dock holds something it closes: a confirmation, reason or answer
/// mode, the open answer card, a decision card (it answers no), an opened docked choice (it
/// folds back to its line).
pub(in crate::model::session) fn escape(cx: &Cx<'_>) -> Option<Action> {
    let session = cx.id.clone();
    if cx.ui.dock.confirming.is_some() {
        return Some(Action::Dock(DockAction::Confirm {
            session,
            yes: false,
        }));
    }
    if cx.ui.dock.compose.is_some() {
        return Some(Action::Dock(DockAction::Leave(session)));
    }
    if cx.ui.dock.open.is_some() {
        return Some(Action::Dock(DockAction::Close(session)));
    }
    let briefs = cx
        .state
        .map(|state| state.cards.as_slice())
        .unwrap_or_default();
    let card = briefs
        .iter()
        .find(|brief| decision(brief.kind))
        .or_else(|| {
            briefs
                .iter()
                .find(|brief| docked(brief.kind) && cx.ui.dock.opened.contains(&brief.id))
        })?;
    Some(Action::Dock(DockAction::Dismiss {
        session,
        card: card.id.clone(),
    }))
}

/// The dock's own keys.
pub fn keys() -> Vec<KeyBinding> {
    vec![bind("alt+down", "Alt+↓", "next dock item", KeyAction::Part)]
}

pub fn press(key: &str, cx: &Cx<'_>) -> Option<Action> {
    (key == "alt+down" && cx.env.form != Form::Phone)
        .then(|| Action::Dock(DockAction::Next(cx.id.clone())))
}
