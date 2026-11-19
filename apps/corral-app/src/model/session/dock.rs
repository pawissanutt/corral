//! The dock above the composer: what waits on the owner and the owner's queued input, each
//! present only when it has content, in the terminal UI's order: a docked choice (an exit
//! choice, a child's question, a proposed board plan), the owner questions, the queue. A select,
//! confirm or input card and an opened question card take the composer's place.
//!
//! The host's state briefs the waiting cards; their bodies are read from `SessionCards`, and a
//! brief whose card is not read yet shows its title alone. Every answer goes out as
//! `session.card.answer` under a key whose reply this page reads; a destructive option is
//! confirmed once on this device first, and an option that needs the owner's reason puts the
//! composer in reason mode until the reason is given or the owner leaves it.

use std::collections::{BTreeMap, BTreeSet};

use corral_protocol::detail::{CardKind, DetailBody, DetailGet, DetailRef, SessionCard};
use corral_protocol::session_host::{
    CardAnswer, SessionActionDone, SessionCardAnswer, SessionCardAnswered,
};
use corral_protocol::session_state::CardBrief;

use crate::model::Id;
use crate::model::daemon::{Feed, Link, Operation};
use crate::model::effect::Effect;
use crate::model::hue::Hue;
use crate::model::nav::Form;

use super::choice::{self, ChoiceCard};
use super::dialog::{self, DecisionCard};
use super::keys::{KeyAction, KeyBinding, bind};
use super::questions::{self, AnswerCard, QuestionDock};
use super::queue::{self, QueueDock};
use super::read_state::ReadState;
use super::{Action, Cx, Focus, Host, State, Ui, exit};

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
    /// Docked choices put aside with Not now; the host keeps them pending.
    pub aside: BTreeSet<String>,
    pub confirming: Option<Confirming>,
    pub compose: Option<Compose>,
    pub pending: BTreeMap<Id, Pending>,
    /// The host's status of this device's answer to a card that still waits (`answered`: the
    /// answer is recorded and its delivery pending).
    pub answered: BTreeMap<String, String>,
    /// The briefed card ids when the cards were last asked for.
    pub asked: Option<Vec<String>>,
}

impl DockUi {
    pub const EMPTY: Self = Self {
        drafts: BTreeMap::new(),
        open: None,
        aside: BTreeSet::new(),
        confirming: None,
        compose: None,
        pending: BTreeMap::new(),
        answered: BTreeMap::new(),
        asked: None,
    };
}

/// What the dock and its cards do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DockAction {
    /// Bring a waiting card forward: open a question's answer card, dock a choice put aside, or
    /// give a decision card the keys.
    Bring { session: Id, card: String },
    /// Close the open answer card; its answers so far stay.
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
    /// Answer nothing: a decision card resolves as no; a docked choice is put aside and stays
    /// pending; a question's card closes.
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
    /// Read the cards behind the briefs `ids`.
    Read { session: Id, ids: Vec<String> },
    /// Give the keys to the next dock item (queue, questions, docked choice, then the composer).
    Next(Id),
}

/// The dock of a session page.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Dock {
    /// The first docked choice not put aside: the exit choice, a child's question, a proposed
    /// board plan.
    pub choice: Option<ChoiceCard>,
    pub questions: Option<QuestionDock>,
    pub queue: Option<QueueDock>,
    /// A select, confirm or input card, drawn in the composer's place.
    pub decision: Option<DecisionCard>,
    /// The open question's answer card, drawn in the composer's place.
    pub answer: Option<AnswerCard>,
    /// The destructive option waiting for this device's one confirmation.
    pub confirm: Option<CardConfirm>,
    /// A docked choice put aside: `The exit choice is waiting · Finish to reopen`.
    pub aside: Option<AsideLine>,
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
            && self.aside.is_none()
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

fn bring(state: &mut State, session: &Id, card: String, feed: &Feed) {
    let kind = kind(feed, session.as_str(), &card);
    let ui = state.ui_mut(session);
    match kind {
        Some(CardKind::Question) => {
            ui.dock.open = Some(card);
            ui.focus = Focus::Questions;
        }
        Some(kind) if docked(kind) => {
            ui.dock.aside.remove(&card);
            ui.focus = Focus::Dock;
        }
        Some(kind) if decision(kind) => ui.focus = Focus::Dock,
        _ => {}
    }
}

fn dismiss(state: &mut State, session: &Id, card: String, feed: &Feed) -> Vec<Effect> {
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
            ui.dock.aside.insert(card);
            ui.focus = Focus::Composer;
            let words = if kind == CardKind::Exit {
                "The exit choice is waiting · Finish to reopen"
            } else {
                "The choice is still waiting · press it in the dock to reopen"
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

/// The focus order Alt+↓ walks, keeping only the items the dock shows.
fn next(state: &mut State, session: &Id, feed: &Feed) {
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
        Focus::Dock => briefs.iter().any(|brief| {
            decision(brief.kind) || (docked(brief.kind) && !ui.dock.aside.contains(&brief.id))
        }),
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
    state.ui_mut(session).focus = focus;
}

fn read(state: &mut State, session: Id, ids: Vec<String>, feed: &Feed) -> Vec<Effect> {
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

/// Handles a dock action. Every answer goes out as an effect; nothing is simulated here.
pub(super) fn act(state: &mut State, action: DockAction, feed: &Feed) -> Vec<Effect> {
    match action {
        DockAction::Bring { session, card } => {
            bring(state, &session, card, feed);
            Vec::new()
        }
        DockAction::Close(session) => {
            let ui = state.ui_mut(&session);
            ui.dock.open = None;
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
        DockAction::Dismiss { session, card } => dismiss(state, &session, card, feed),
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
        DockAction::Read { session, ids } => read(state, session, ids, feed),
        DockAction::Next(session) => {
            next(state, &session, feed);
            Vec::new()
        }
    }
}

/// Follows the feed for one page's dock: reads each settled reply once, and forgets drafts,
/// asides and modes of cards the host no longer waits on.
pub(super) fn settle(ui: &mut Ui, scope: &str, feed: &Feed) {
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
    ui.dock.aside.retain(|card| open.contains(card.as_str()));
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
fn inert(cx: &Cx<'_>) -> Option<String> {
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
pub(super) fn fetch(cx: &Cx<'_>) -> Option<Action> {
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

fn cards_read(cx: &Cx<'_>, briefs: &[CardBrief]) -> Option<CardsRead> {
    let missing = briefs
        .iter()
        .any(|brief| card(cx.feed, cx.id.as_str(), &brief.id).is_none());
    if !missing {
        return None;
    }
    let target = target(cx.id.as_str());
    let read = ReadState::of(cx.feed.detail(&target), cx.feed);
    Some(CardsRead {
        words: if read.has_rows() {
            "reading…".into()
        } else {
            read.words()
        },
        hue: read.hue(),
        retry: matches!(read, ReadState::Failed(_)).then_some(Action::Refresh(target)),
    })
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

fn aside_line(cx: &Cx<'_>, brief: &CardBrief) -> AsideLine {
    let text = match brief.kind {
        CardKind::Exit => "The exit choice is waiting · Finish to reopen".into(),
        _ => format!("Waiting: {}", brief.title),
    };
    AsideLine {
        text,
        action: Action::Dock(DockAction::Bring {
            session: cx.id.clone(),
            card: brief.id.clone(),
        }),
    }
}

/// The dock of the session `cx` reads.
pub fn dock(cx: &Cx<'_>) -> Dock {
    let queue = queue::queue_dock(cx);
    let Some(state) = cx.state else {
        return Dock {
            queue,
            ..Dock::default()
        };
    };
    let inert = inert(cx).filter(|_| !state.cards.is_empty());
    let mut choice = None;
    let mut dialog_card = None;
    let mut aside = None;
    for brief in &state.cards {
        let body = card(cx.feed, cx.id.as_str(), &brief.id);
        if docked(brief.kind) {
            if cx.ui.dock.aside.contains(&brief.id) {
                aside = aside.or_else(|| Some(aside_line(cx, brief)));
            } else if choice.is_none() {
                choice = Some(choice::card(cx, brief, body, inert.clone()));
            }
        } else if decision(brief.kind) && dialog_card.is_none() {
            dialog_card = Some(dialog::card(cx, brief, body, inert.clone()));
        }
    }
    let answer = cx
        .ui
        .dock
        .open
        .as_deref()
        .and_then(|open| state.cards.iter().find(|brief| brief.id == open))
        .and_then(|brief| {
            let body = card(cx.feed, cx.id.as_str(), &brief.id)?;
            Some(questions::answer_card(cx, brief, body, inert.clone()))
        });
    Dock {
        choice,
        questions: questions::dock(cx, &state.cards),
        queue,
        decision: dialog_card,
        answer,
        confirm: confirm(cx),
        aside,
        oldest: state.cards.first().map(|brief| {
            Action::Dock(DockAction::Bring {
                session: cx.id.clone(),
                card: brief.id.clone(),
            })
        }),
        read: cards_read(cx, &state.cards),
        inert,
    }
}

/// Esc on the page while the dock holds something it closes: a confirmation, reason or answer
/// mode, the open answer card, a decision card (it answers no), a focused docked choice.
pub(super) fn escape(cx: &Cx<'_>) -> Option<Action> {
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
            (cx.ui.focus == Focus::Dock)
                .then(|| {
                    briefs
                        .iter()
                        .find(|brief| docked(brief.kind) && !cx.ui.dock.aside.contains(&brief.id))
                })
                .flatten()
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
