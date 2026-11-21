//! What the dock reads and what comes back: reading the cards behind the host's briefs (once,
//! and again when the briefs name cards the last read did not hold), where that read stands, why
//! the cards cannot be answered from here now, and each settled reply to a request this page
//! sent, read once, after which drafts, opened cards and modes of cards the host no longer waits
//! on are forgotten.

use std::collections::BTreeSet;

use corral_protocol::detail::DetailGet;
use corral_protocol::session_host::{SessionActionDone, SessionCardAnswered};
use corral_protocol::session_state::CardBrief;

use crate::model::Id;
use crate::model::daemon::{Feed, Link, Operation};
use crate::model::effect::Effect;
use crate::model::hue::Hue;
use crate::model::session::queue;
use crate::model::session::read_state::ReadState;
use crate::model::session::{Action, Cx, Focus, Host, State, Ui};

use super::{CardsRead, DockAction, Pending, card, fold, refusal, target};

/// Reads the cards behind the briefs `ids`.
pub(super) fn read(state: &mut State, session: Id, ids: Vec<String>, feed: &Feed) -> Vec<Effect> {
    let target = target(session.as_str());
    let etag = feed
        .detail(&target)
        .and_then(|cache| cache.pages.first())
        .map(|page| page.etag.clone());
    state.ui_mut(&session).dock.asked = Some(ids);
    vec![Effect::Detail(DetailGet {
        target,
        etag,
        page: None,
    })]
}

/// Follows the feed for one page's dock: reads each settled reply once, opens the exit choice
/// Finish… asked for once it docks, and forgets drafts, opened cards and modes of cards the host
/// no longer waits on.
pub(in crate::model::session) fn settle(ui: &mut Ui, scope: &str, feed: &Feed) {
    let settled: Vec<(Id, Pending)> = ui
        .dock
        .pending
        .iter()
        .filter(|(key, _)| {
            feed.operations
                .get(*key)
                .is_some_and(|operation| !operation.loading)
        })
        .map(|(key, pending)| (key.clone(), pending.clone()))
        .collect();
    for (key, pending) in settled {
        ui.dock.pending.remove(&key);
        let Some(operation) = feed.operations.get(&key) else {
            continue;
        };
        match pending {
            Pending::Card { card } => answered(ui, card, operation, feed.now),
            Pending::Action { what } => match refusal(operation) {
                Some(why) => ui
                    .notices
                    .push(Hue::Fail, format!("{what} refused: {why}"), feed.now),
                None => {
                    if let Some(text) = operation
                        .decode::<SessionActionDone>()
                        .and_then(|done| done.text)
                    {
                        ui.notices.push(Hue::Neutral, text, feed.now);
                    }
                }
            },
            pending => queue::settled(ui, pending, operation, feed.now),
        }
    }
    let Some(state) = feed.live.get(scope).and_then(|live| live.state.as_ref()) else {
        return;
    };
    let open: BTreeSet<&str> = state.cards.iter().map(|brief| brief.id.as_str()).collect();
    ui.dock
        .drafts
        .retain(|card, _| open.contains(card.as_str()));
    let advertised = feed
        .assistants
        .get(scope)
        .and_then(|row| row.question_id.as_deref());
    fold::settle(ui, state, advertised, feed.now);
    ui.dock
        .answered
        .retain(|card, _| open.contains(card.as_str()));
    if ui
        .dock
        .open
        .as_deref()
        .is_some_and(|card| !open.contains(card))
    {
        ui.dock.open = None;
    }
    if ui
        .dock
        .confirming
        .as_ref()
        .is_some_and(|chosen| !open.contains(chosen.card.as_str()))
    {
        ui.dock.confirming = None;
    }
    if ui
        .dock
        .compose
        .as_ref()
        .is_some_and(|mode| !open.contains(mode.card()))
    {
        ui.dock.compose = None;
        ui.notices.push(
            Hue::Held,
            "That choice no longer waits; your text stays in the draft",
            feed.now,
        );
    }
}

fn answered(ui: &mut Ui, card: String, operation: &Operation, now: u64) {
    if let Some(why) = refusal(operation) {
        ui.notices
            .push(Hue::Fail, format!("Answer not taken: {why}"), now);
        return;
    }
    let status = operation
        .decode::<SessionCardAnswered>()
        .map(|reply| reply.status)
        .unwrap_or_default();
    match status.as_str() {
        "expired" => ui.notices.push(
            Hue::Held,
            "That card expired before the answer reached the host",
            now,
        ),
        "missing" => ui.notices.push(
            Hue::Held,
            "That card no longer waits; nothing was answered",
            now,
        ),
        "answered" => {
            ui.dock.answered.insert(card.clone(), status);
        }
        _ => {}
    }
    ui.dock.drafts.remove(&card);
    if ui.dock.open.as_deref() == Some(card.as_str()) {
        ui.dock.open = None;
        ui.focus = Focus::Composer;
    }
}

/// Why the cards cannot be answered from here now.
pub(super) fn inert(cx: &Cx<'_>) -> Option<String> {
    if cx.feed.link != Link::Up {
        return Some("answer when corrald is back".into());
    }
    match cx.host {
        Host::NotRunning | Host::Unreachable | Host::Ended => {
            Some("the host must be running to take an answer".into())
        }
        Host::Running | Host::Unknown => None,
    }
}

/// The cards to read now: once while none was read, and again when the briefs name cards the
/// last read did not hold. A failed read is retried from the dock, not here.
pub(in crate::model::session) fn fetch(cx: &Cx<'_>) -> Option<Action> {
    let briefs = cx
        .state
        .map(|state| state.cards.as_slice())
        .unwrap_or_default();
    if briefs.is_empty() || cx.host != Host::Running {
        return None;
    }
    let cache = cx.feed.detail(&target(cx.id.as_str()));
    if cache.is_some_and(|cache| cache.loading) {
        return None;
    }
    let ids: Vec<String> = briefs.iter().map(|brief| brief.id.clone()).collect();
    let missing = briefs
        .iter()
        .any(|brief| card(cx.feed, cx.id.as_str(), &brief.id).is_none());
    let wanted = cache.is_none() || (missing && cx.ui.dock.asked.as_ref() != Some(&ids));
    wanted.then(|| {
        Action::Dock(DockAction::Read {
            session: cx.id.clone(),
            ids,
        })
    })
}

/// Where the read of briefed cards whose bodies are not here stands.
pub(super) fn cards_read(cx: &Cx<'_>, briefs: &[CardBrief]) -> Option<CardsRead> {
    briefs
        .iter()
        .any(|brief| card(cx.feed, cx.id.as_str(), &brief.id).is_none())
        .then(|| card_read(cx))
}

/// Where the read stands of a briefed card whose body is not here: reading while a read is under
/// way or about to be asked for, the failure with Retry, and, once a read came back without the
/// card, that it was not there, with Retry. Nothing is inferred meanwhile.
pub(in crate::model::session) fn card_read(cx: &Cx<'_>) -> CardsRead {
    let target = target(cx.id.as_str());
    let read = ReadState::of(cx.feed.detail(&target), cx.feed);
    let retry = Some(Action::Refresh(target));
    match read {
        ReadState::Failed(_) => CardsRead {
            words: read.words(),
            hue: read.hue(),
            retry,
        },
        ReadState::Read { .. } | ReadState::Stale { .. } if fetch(cx).is_none() => CardsRead {
            words: "! not in the last read of the cards · nothing is inferred".into(),
            hue: Hue::Held,
            retry,
        },
        _ => CardsRead {
            words: "reading…".into(),
            hue: Hue::Neutral,
            retry: None,
        },
    }
}
