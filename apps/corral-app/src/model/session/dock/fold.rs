//! Which waiting cards the dock shows open. A docked choice (an exit choice, a child's question,
//! a proposed board plan, a plan review) is one line until the owner opens it: by pressing its line, Alt+A,
//! Alt+↓, the needs-you chip, or from elsewhere (Bring, or Answer from outside the page); Not now
//! and Esc fold it back. The exit choice Finish… asked for opens by itself once the host docks
//! it, and so does the card an Answer from outside the page named, once briefed. Alt+↓ also
//! unfolds the queue when it gives it the keys.

use corral_protocol::app::SESSION_STATE_V1;
use corral_protocol::detail::CardKind;
use corral_protocol::session_state::{CardBrief, SessionState};

use crate::model::Id;
use crate::model::daemon::Feed;
use crate::model::effect::Effect;
use crate::model::hue::Hue;
use crate::model::nav::Form;
use crate::model::session::choice::{self, ChoiceCard};
use crate::model::session::keys::{KeyAction, KeyBinding, bind};
use crate::model::session::questions::{self, AnswerCard};
use crate::model::session::{Action, Cx, Focus, State, Tab, Ui};

use super::{
    AsideLine, Asked, CardsRead, DockAction, briefs, card, decision, docked, kind, leave, send,
    sending,
};

/// At most this many folded lines show; the rest are counted.
const LINES: usize = 3;

/// Brings a waiting card forward: a question's answer card opens, a docked choice opens, a
/// decision card takes the keys. The owner's own choice replaces an Answer still waiting.
pub(super) fn bring(state: &mut State, session: &Id, card: String, feed: &Feed) {
    let kind = kind(feed, session.as_str(), &card);
    let ui = state.ui_mut(session);
    ui.dock.wanted = None;
    if let Some(kind) = kind {
        open(ui, card, kind);
    }
}

fn open(ui: &mut Ui, card: String, kind: CardKind) {
    match kind {
        CardKind::Question => {
            ui.dock.open = Some(card);
            ui.focus = Focus::Questions;
        }
        // The plan's document takes the conversation's place.
        CardKind::PlanReview => {
            ui.dock.opened.insert(card);
            ui.tab = Tab::Chat;
            ui.sub = None;
            ui.focus = Focus::Dock;
        }
        kind if docked(kind) => {
            ui.dock.opened.insert(card);
            ui.focus = Focus::Dock;
        }
        kind if decision(kind) => ui.focus = Focus::Dock,
        _ => {}
    }
}

/// The card `asked` names among the host's briefs: that card by its id, or the oldest owner
/// question.
fn named<'c>(cards: &'c [CardBrief], asked: &Asked) -> Option<&'c CardBrief> {
    match asked {
        Asked::Card(id) => cards.iter().find(|brief| brief.id == *id),
        Asked::Oldest => cards.iter().find(|brief| brief.kind == CardKind::Question),
    }
}

/// Answer from outside the page: opens the card `asked` names when the host's state briefs it,
/// else waits for a state that does. A connection that never carries the host's state has
/// nothing to wait for: the page opens as it is.
pub(super) fn answer(state: &mut State, session: &Id, asked: Asked, feed: &Feed) {
    let briefed = feed
        .live
        .get(session.as_str())
        .and_then(|live| live.state.as_ref())
        .map(|host| named(&host.cards, &asked).map(|brief| (brief.id.clone(), brief.kind)));
    let ui = state.ui_mut(session);
    ui.dock.wanted = None;
    match briefed {
        Some(Some((card, kind))) => open(ui, card, kind),
        // A state that briefs no owner question has none to open. A card the row names may be
        // newer than the state held here, so that one is waited for.
        Some(None) if asked == Asked::Oldest => {}
        _ if feed.capable(SESSION_STATE_V1) => ui.dock.wanted = Some(asked),
        _ => {}
    }
}

/// The card an Answer waits for, open in the composer's place until the host's state briefs
/// it: the question as the session's row says it, and why it is not here yet.
pub(super) fn wanted_card(cx: &Cx<'_>) -> Option<AnswerCard> {
    let asked = cx.ui.dock.wanted.as_ref()?;
    let said = crate::model::agents::session_line(cx.feed, cx.id.as_str())
        .and_then(|line| line.doing)
        .unwrap_or_else(|| "Owner question".to_owned());
    let card = match asked {
        Asked::Card(id) => id.clone(),
        Asked::Oldest => String::new(),
    };
    let read = match super::reads::inert(cx) {
        Some(why) => CardsRead {
            words: why,
            hue: Hue::Held,
            retry: None,
        },
        None => CardsRead {
            words: "reading the question…".into(),
            hue: Hue::Neutral,
            retry: None,
        },
    };
    Some(questions::waiting_card(cx, card, said, read, None))
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
            let words = match kind {
                CardKind::Exit => "The exit choice is waiting · Finish to reopen",
                CardKind::PlanReview => "The plan is still waiting · press its line to reopen",
                _ => "The choice is still waiting · press its line to reopen",
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
        .map(|brief| (brief.id.clone(), brief.kind));
    let ui = state.ui_mut(session);
    ui.focus = focus;
    match (focus, first) {
        (Focus::Queue, _) => ui.dock.queue_open = Some(true),
        // Opened as its line opens it: a plan review's document takes the main view.
        (Focus::Dock, Some((card, kind))) => open(ui, card, kind),
        _ => {}
    }
}

/// Follows the host's cards: forgets opened cards that no longer wait, opens the exit choice
/// Finish… asked for once it docks and the card an Answer waits for once briefed, and lets an
/// emptied queue fold by the rule again. A named card the state does not brief is waited for
/// while the session's row still names it (`advertised`); once it does not, the Answer ends
/// without opening another card.
pub(super) fn settle(ui: &mut Ui, state: &SessionState, advertised: Option<&str>, now: u64) {
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
    if let Some(asked) = ui.dock.wanted.take() {
        if let Some(brief) = named(&state.cards, &asked) {
            open(ui, brief.id.clone(), brief.kind);
        } else if matches!(&asked, Asked::Card(id) if advertised == Some(id.as_str())) {
            ui.dock.wanted = Some(asked);
        } else {
            ui.notices.push(
                Hue::Neutral,
                "That question no longer waits; nothing else was opened",
                now,
            );
        }
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
        let open = cx.ui.dock.opened.contains(&brief.id);
        // An opened plan review is the plan view's: neither a choice card nor a line.
        if open && brief.kind == CardKind::PlanReview {
            continue;
        }
        if opened.is_none() && open {
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
    if cx.ui.dock.open.is_some() || cx.ui.dock.wanted.is_some() {
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
