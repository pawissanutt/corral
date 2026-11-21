//! The dock above the composer: what waits on the owner and the owner's queued input, each
//! present only when it has content, in the terminal UI's order: a docked choice (an exit
//! choice, a child's question, a proposed board plan), the owner questions, the queue. A select,
//! confirm or input card and an opened question card take the composer's place.
//!
//! The host's state briefs the waiting cards; their bodies are read from `SessionCards`, and a
//! card shown open before its body is read says where that read stands, with Retry after a
//! failure. Every answer goes out as `session.card.answer` under a key whose reply this page
//! reads; a destructive option is confirmed once on this device first, and an option that
//! needs the owner's reason puts the composer in reason mode until the reason is given or the
//! owner leaves it. Which cards show open is `fold`'s; what is read and what comes back is
//! `reads`'. An Answer pressed outside the page (Home, a session's row) names the card it opens
//! ([`Asked`]); the page shows that card open, reading until the host's state briefs it, and
//! never another card in its place.

use std::collections::{BTreeMap, BTreeSet};

use corral_protocol::detail::{CardKind, DetailBody, DetailRef, SessionCard};
use corral_protocol::session_host::{CardAnswer, SessionCardAnswer};
use corral_protocol::session_state::CardBrief;

use crate::model::daemon::{Feed, Operation};
use crate::model::effect::Effect;
use crate::model::hue::Hue;
use crate::model::{Id, Live};

use super::choice::{self, ChoiceCard};
use super::dialog::{self, DecisionCard};
use super::questions::{self, AnswerCard, QuestionDock};
use super::queue::{self, QueueDock};
use super::{Action, Cx, Focus, State, Ui, exit};

mod fold;
#[cfg(test)]
mod fold_tests;
mod reads;

pub(super) use fold::escape;
pub use fold::{keys, press};
pub(super) use reads::{card_read, fetch, settle};

/// This device's answer to one card while it is being given.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CardDraft {
    /// The option ids chosen, per question.
    pub picks: Vec<Vec<String>>,
    /// The owner's own words, per question; none for a question not typed in yet.
    pub texts: Vec<Option<String>>,
    /// The answer card's tab; the questions' count is Review.
    pub tab: usize,
}

impl CardDraft {
    pub fn picks(&self, question: usize) -> &[String] {
        self.picks.get(question).map_or(&[][..], Vec::as_slice)
    }

    /// The words typed for `question`, when any were.
    pub fn typed(&self, question: usize) -> Option<&str> {
        self.texts.get(question).and_then(Option::as_deref)
    }

    pub fn text(&self, question: usize) -> &str {
        self.typed(question).unwrap_or_default()
    }

    pub(super) fn picks_mut(&mut self, question: usize) -> &mut Vec<String> {
        if self.picks.len() <= question {
            self.picks.resize(question + 1, Vec::new());
        }
        &mut self.picks[question]
    }

    pub(super) fn text_mut(&mut self, question: usize) -> &mut Option<String> {
        if self.texts.len() <= question {
            self.texts.resize(question + 1, None);
        }
        &mut self.texts[question]
    }
}

/// What the composer's Enter gives instead of a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Compose {
    /// The chosen option needs the owner's reason (Land over open findings): Enter chooses it
    /// with the text.
    Reason {
        card: String,
        option: String,
        label: String,
        why: String,
    },
    /// The owner's own answer to a child's question.
    Answer { card: String, child: Option<String> },
}

impl Compose {
    pub fn card(&self) -> &str {
        match self {
            Self::Reason { card, .. } | Self::Answer { card, .. } => card,
        }
    }
}

/// The card an Answer pressed outside the page opens: the one the session's row names (a chat's
/// or a build's `question_id`, which is the card's id), or, where the row names none, the
/// oldest owner question the host's state briefs. Never a card matched by its words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Asked {
    Card(String),
    Oldest,
}

/// An option chosen on this device, before it goes out: a destructive one waits here for its
/// one confirmation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Confirming {
    pub card: String,
    pub option: String,
    pub label: String,
    pub detail: Option<String>,
    /// The reason the host asks for before it takes this option.
    pub needs_text: Option<String>,
}

/// A request this page sent, read back under its key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pending {
    Card {
        card: String,
    },
    Withdraw {
        key: String,
        /// Edit: the text goes back into the draft.
        edit: bool,
        /// The text as the host listed it.
        listed: String,
        /// Whether the listed text is the whole input.
        whole: bool,
    },
    Move {
        key: String,
    },
    /// A `session.action`, named as a notice names it.
    Action {
        what: &'static str,
    },
}

/// The dock's state on this device, per session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockUi {
    pub drafts: BTreeMap<String, CardDraft>,
    /// The owner question whose answer card is open.
    pub open: Option<String>,
    /// Docked choices the owner opened (by pressing their line, Alt+A, Alt+↓ or from
    /// elsewhere); every other waiting card is one line in the dock.
    pub opened: BTreeSet<String>,
    /// Finish… asked the host for the exit choice: it opens when it docks.
    pub exit_wanted: bool,
    /// The owner folded or unfolded the queue; none follows the rule in `queue::open`.
    pub queue_open: Option<bool>,
    pub confirming: Option<Confirming>,
    pub compose: Option<Compose>,
    pub pending: BTreeMap<Id, Pending>,
    /// The host's status of this device's answer to a card that still waits (`answered`: the
    /// answer is recorded and its delivery pending).
    pub answered: BTreeMap<String, String>,
    /// The briefed card ids when the cards were last asked for.
    pub asked: Option<Vec<String>>,
    /// An Answer from outside the page whose card the host's state does not brief yet: it
    /// opens once briefed.
    pub wanted: Option<Asked>,
}

impl DockUi {
    pub const EMPTY: Self = Self {
        drafts: BTreeMap::new(),
        open: None,
        opened: BTreeSet::new(),
        exit_wanted: false,
        queue_open: None,
        confirming: None,
        compose: None,
        pending: BTreeMap::new(),
        answered: BTreeMap::new(),
        asked: None,
        wanted: None,
    };
}

/// What the dock and its cards do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DockAction {
    /// Bring a waiting card forward: open a question's answer card, open a docked choice, or
    /// give a decision card the keys.
    Bring { session: Id, card: String },
    /// Answer from outside the page: open the session's page with the card `asked` names open,
    /// or reading until the host's state briefs it.
    Answer { session: Id, asked: Asked },
    /// Close the open answer card, or the one an Answer still waits for; its answers so far
    /// stay.
    Close(Id),
    /// Show a tab of the answer card; the questions' count is Review.
    Show {
        session: Id,
        card: String,
        tab: usize,
    },
    /// Choose option `option` of question `question` (a multi-choice question toggles it).
    Pick {
        session: Id,
        card: String,
        question: usize,
        option: String,
    },
    /// The owner's own words for question `question`.
    Text {
        session: Id,
        card: String,
        question: usize,
        text: String,
    },
    /// Send a question card's answers once every question has one, else go to the first
    /// unanswered; send an input card's text.
    Submit { session: Id, card: String },
    /// Choose an option of a docked choice or a select or confirm card: a destructive one asks
    /// first, one that needs a reason puts the composer in reason mode.
    Choose {
        session: Id,
        card: String,
        option: String,
    },
    /// Answer nothing: a decision card resolves as no; a docked choice folds back to its line
    /// and stays pending; a question's card closes.
    Dismiss { session: Id, card: String },
    /// Confirm, or cancel, the destructive option waiting for its confirmation.
    Confirm { session: Id, yes: bool },
    /// Write my own answer to a child's question: the composer's Enter answers it.
    Own { session: Id, card: String },
    /// Leave reason or answer mode; nothing is sent.
    Leave(Id),
    /// Finish…: dock the pending exit choice, or ask the host to open it.
    Finish(Id),
    /// Take a queued input back: into the draft (`edit`), or away unsent.
    Withdraw {
        session: Id,
        key: String,
        edit: bool,
    },
    /// Move a queued input one place up (-1) or down (1).
    Move { session: Id, key: String, delta: i8 },
    /// Fold or unfold the queue.
    ToggleQueue(Id),
    /// Read the cards behind the briefs `ids`.
    Read { session: Id, ids: Vec<String> },
    /// Give the keys to the next dock item (queue, questions, docked choice, then the composer).
    Next(Id),
}

/// The dock of a session page.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Dock {
    /// The first docked choice the owner opened: the exit choice, a child's question, a
    /// proposed board plan.
    pub choice: Option<ChoiceCard>,
    pub questions: Option<QuestionDock>,
    pub queue: Option<QueueDock>,
    /// A select, confirm or input card, drawn in the composer's place.
    pub decision: Option<DecisionCard>,
    /// The open question's answer card, drawn in the composer's place.
    pub answer: Option<AnswerCard>,
    /// The destructive option waiting for this device's one confirmation.
    pub confirm: Option<CardConfirm>,
    /// Docked choices not opened, one line each (up to three): `▲ docs asks: … · Alt+A to answer`.
    pub waiting: Vec<AsideLine>,
    /// `+2 more` beyond the lines shown.
    pub more: Option<String>,
    /// Docks the oldest waiting card (the header's needs-you chip).
    pub oldest: Option<Action>,
    /// Briefed cards whose bodies are not read: where their read stands.
    pub read: Option<CardsRead>,
    /// Why the cards cannot be answered from here now, when they cannot.
    pub inert: Option<String>,
}

impl Dock {
    pub fn is_empty(&self) -> bool {
        self.choice.is_none()
            && self.questions.is_none()
            && self.queue.is_none()
            && self.decision.is_none()
            && self.waiting.is_empty()
    }
}

/// `Land on main?` with what it does, before it goes out.
#[derive(Debug, Clone, PartialEq)]
pub struct CardConfirm {
    pub title: String,
    pub detail: Option<String>,
    /// The option's own label, which confirms.
    pub confirm: (String, Action),
    pub cancel: Action,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AsideLine {
    /// `dock-<card>`.
    pub id: String,
    pub text: String,
    pub action: Action,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CardsRead {
    /// `reading…`, `! read failed: … · nothing is inferred`.
    pub words: String,
    pub hue: Hue,
    pub retry: Option<Action>,
}

fn target(session: &str) -> DetailRef {
    DetailRef::SessionCards {
        id: session.to_owned(),
    }
}

/// Card `card` of session `session`, as last read.
pub(super) fn card<'f>(feed: &'f Feed, session: &str, card: &str) -> Option<&'f SessionCard> {
    match feed.detail(&target(session))?.body()? {
        DetailBody::SessionCards(cards) => cards.cards.iter().find(|found| found.id == card),
        _ => None,
    }
}

/// The host's brief of card `card` of session `session`.
pub(super) fn brief<'f>(feed: &'f Feed, session: &str, card: &str) -> Option<&'f CardBrief> {
    briefs(feed, session).iter().find(|brief| brief.id == card)
}

pub(super) fn briefs<'f>(feed: &'f Feed, session: &str) -> &'f [CardBrief] {
    feed.live
        .get(session)
        .and_then(|live| live.state.as_ref())
        .map_or(&[][..], |state| state.cards.as_slice())
}

fn kind(feed: &Feed, session: &str, id: &str) -> Option<CardKind> {
    brief(feed, session, id)
        .map(|brief| brief.kind)
        .or_else(|| card(feed, session, id).map(|card| card.kind))
}

pub(super) const fn docked(kind: CardKind) -> bool {
    matches!(
        kind,
        CardKind::Exit | CardKind::ChildQuestion | CardKind::BoardPlan
    )
}

pub(super) const fn decision(kind: CardKind) -> bool {
    matches!(kind, CardKind::Select | CardKind::Confirm | CardKind::Input)
}

/// Whether this device's answer to `card` is on its way.
pub(super) fn sending(ui: &Ui, feed: &Feed, card: &str) -> bool {
    ui.dock.pending.iter().any(|(key, pending)| {
        matches!(pending, Pending::Card { card: sent } if sent == card)
            && feed
                .operations
                .get(key)
                .is_none_or(|operation| operation.loading)
    })
}

/// What a request's reply refused, as corrald or the host said it.
pub(super) fn refusal(operation: &Operation) -> Option<String> {
    operation
        .refusal
        .as_ref()
        .map(|refusal| refusal.reason.clone())
        .or_else(|| operation.error.as_ref().map(|error| error.message.clone()))
}

/// Sends an answer to `card` under a new key.
pub(super) fn send(
    state: &mut State,
    session: &Id,
    card: &str,
    answers: Vec<CardAnswer>,
    dismiss: bool,
) -> Vec<Effect> {
    let key = state.ids.next_id();
    state.ui_mut(session).dock.pending.insert(
        key.clone(),
        Pending::Card {
            card: card.to_owned(),
        },
    );
    vec![Effect::SessionCardAnswer {
        key,
        answer: SessionCardAnswer {
            id: session.to_string(),
            card: card.to_owned(),
            answers,
            dismiss,
        },
    }]
}

/// Leaves reason or answer mode, saying so.
pub(super) fn leave(ui: &mut Ui, now: u64) {
    let words = match ui.dock.compose.take() {
        Some(Compose::Reason { label, .. }) => format!("{label} was not chosen; nothing was sent"),
        Some(Compose::Answer { .. }) => "Left answer mode; nothing was sent".into(),
        None => return,
    };
    ui.notices.push(Hue::Neutral, words, now);
}

/// The composer's Enter in reason or answer mode: the text answers the card. An empty text
/// sends nothing and says what is missing. None outside those modes.
pub(super) fn compose_send(
    state: &mut State,
    session: &Id,
    text: &str,
    feed: &Feed,
) -> Option<Vec<Effect>> {
    let compose = state.ui(session.as_str()).dock.compose.clone()?;
    let text = text.trim();
    let now = feed.now;
    if text.is_empty() {
        let words = match &compose {
            Compose::Reason { label, why, .. } => format!("Type your reason to {label}: {why}"),
            Compose::Answer { .. } => "Type your answer, or press Esc to leave answer mode".into(),
        };
        state.ui_mut(session).notices.push(Hue::Held, words, now);
        return Some(Vec::new());
    }
    if sending(state.ui(session.as_str()), feed, compose.card()) {
        return Some(Vec::new());
    }
    let options = match &compose {
        Compose::Reason { option, .. } => vec![option.clone()],
        Compose::Answer { .. } => Vec::new(),
    };
    let ui = state.ui_mut(session);
    ui.dock.compose = None;
    ui.draft.clear();
    Some(send(
        state,
        session,
        compose.card(),
        vec![CardAnswer {
            options,
            text: Some(text.to_owned()),
        }],
        false,
    ))
}

/// Handles a dock action. Every answer goes out as an effect; nothing is simulated here.
pub(super) fn act(state: &mut State, action: DockAction, feed: &Feed) -> Vec<Effect> {
    match action {
        DockAction::Bring { session, card } => {
            fold::bring(state, &session, card, feed);
            Vec::new()
        }
        DockAction::Answer { session, asked } => {
            fold::answer(state, &session, asked, feed);
            vec![Effect::OpenLive(Live::Session(session))]
        }
        DockAction::Close(session) => {
            let ui = state.ui_mut(&session);
            ui.dock.open = None;
            ui.dock.wanted = None;
            ui.focus = Focus::Composer;
            Vec::new()
        }
        DockAction::Show { session, card, tab } => {
            state
                .ui_mut(&session)
                .dock
                .drafts
                .entry(card)
                .or_default()
                .tab = tab;
            Vec::new()
        }
        DockAction::Pick {
            session,
            card,
            question,
            option,
        } => {
            questions::pick(state, &session, &card, question, option, feed);
            Vec::new()
        }
        DockAction::Text {
            session,
            card,
            question,
            text,
        } => {
            questions::text(state, &session, &card, question, text, feed);
            Vec::new()
        }
        DockAction::Submit { session, card } => match kind(feed, session.as_str(), &card) {
            Some(CardKind::Input) => dialog::submit(state, &session, &card, feed),
            Some(CardKind::Question) => questions::submit(state, &session, &card, feed),
            _ => Vec::new(),
        },
        DockAction::Choose {
            session,
            card,
            option,
        } => choice::choose(state, &session, &card, &option, feed),
        DockAction::Dismiss { session, card } => fold::dismiss(state, &session, card, feed),
        DockAction::Confirm { session, yes } => choice::confirm(state, &session, yes, feed),
        DockAction::Own { session, card } => {
            choice::own(state, &session, card, feed);
            Vec::new()
        }
        DockAction::Leave(session) => {
            leave(state.ui_mut(&session), feed.now);
            Vec::new()
        }
        DockAction::Finish(session) => exit::finish(state, &session, feed),
        DockAction::Withdraw { session, key, edit } => {
            queue::withdraw(state, &session, key, edit, feed)
        }
        DockAction::Move {
            session,
            key,
            delta,
        } => queue::move_item(state, &session, key, delta, feed),
        DockAction::ToggleQueue(session) => {
            let open = queue::open(state.ui(session.as_str()), feed, session.as_str());
            state.ui_mut(&session).dock.queue_open = Some(!open);
            Vec::new()
        }
        DockAction::Read { session, ids } => reads::read(state, session, ids, feed),
        DockAction::Next(session) => {
            fold::next(state, &session, feed);
            Vec::new()
        }
    }
}

fn confirm(cx: &Cx<'_>) -> Option<CardConfirm> {
    let chosen = cx.ui.dock.confirming.as_ref()?;
    Some(CardConfirm {
        title: format!("{}?", chosen.label),
        detail: chosen.detail.clone(),
        confirm: (
            chosen.label.clone(),
            Action::Dock(DockAction::Confirm {
                session: cx.id.clone(),
                yes: true,
            }),
        ),
        cancel: Action::Dock(DockAction::Confirm {
            session: cx.id.clone(),
            yes: false,
        }),
    })
}

/// The dock of the session `cx` reads.
pub fn dock(cx: &Cx<'_>) -> Dock {
    let queue = queue::queue_dock(cx);
    let wanted = fold::wanted_card(cx);
    let Some(state) = cx.state else {
        return Dock {
            queue,
            answer: wanted,
            ..Dock::default()
        };
    };
    let inert = reads::inert(cx).filter(|_| !state.cards.is_empty());
    let (choice, waiting, more) = fold::docked_cards(cx, &state.cards, inert.clone());
    let dialog_card = state
        .cards
        .iter()
        .find(|brief| decision(brief.kind))
        .map(|brief| {
            let body = card(cx.feed, cx.id.as_str(), &brief.id);
            dialog::card(cx, brief, body, inert.clone())
        });
    let answer = cx
        .ui
        .dock
        .open
        .as_deref()
        .and_then(|open| state.cards.iter().find(|brief| brief.id == open))
        .map(|brief| {
            let body = card(cx.feed, cx.id.as_str(), &brief.id);
            questions::answer_card(cx, brief, body, inert.clone())
        })
        .or(wanted);
    // A card shown open says where the read stands itself; the dock's line would repeat it.
    let shown_read = choice.as_ref().is_some_and(|c| c.read.is_some())
        || dialog_card.as_ref().is_some_and(|c| c.read.is_some())
        || answer.as_ref().is_some_and(|c| c.read.is_some());
    Dock {
        choice,
        questions: questions::dock(cx, &state.cards),
        queue,
        decision: dialog_card,
        answer,
        confirm: confirm(cx),
        waiting,
        more,
        oldest: state.cards.first().map(|brief| {
            Action::Dock(DockAction::Bring {
                session: cx.id.clone(),
                card: brief.id.clone(),
            })
        }),
        read: reads::cards_read(cx, &state.cards).filter(|_| !shown_read),
        inert,
    }
}
