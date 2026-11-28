//! Owner questions: the dock line that says what waits (up to three, each its title, and
//! `answer recorded; delivery pending` once the host recorded this device's answer), and the
//! answer card that opens in the composer's place: one tab per question and Review, numbered
//! options (several for a multi-choice question), the owner's own words where the question takes
//! them. A card goes out once, with one answer per question, and only when every question has
//! one; until then Send goes to the first unanswered question.

use corral_protocol::detail::{CardKind, CardQuestion, SessionCard};
use corral_protocol::session_host::CardAnswer;
use corral_protocol::session_state::CardBrief;

use crate::model::Id;
use crate::model::daemon::Feed;
use crate::model::effect::Effect;
use crate::model::hue::Hue;
use crate::model::nav::Form;

use super::actions::{ActionGroup, row};
use super::choice::{ChoiceOption, options};
use super::dock::{self, CardDraft, CardsRead, DockAction};
use super::keys::{KeyAction, KeyBinding, bind};
use super::{Action, Cx, State};

/// The questions dock: `2 owner questions are waiting · Alt+A to answer`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestionDock {
    pub header: String,
    /// `Alt+A to answer` where keys show.
    pub hint: Option<&'static str>,
    pub items: Vec<QuestionLine>,
    /// `+2 more` beyond the three shown.
    pub more: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestionLine {
    /// `dock-<card>`.
    pub id: String,
    pub card: String,
    /// The host's title of the card.
    pub label: String,
    /// `answer recorded; delivery pending`, `this app cannot answer this kind of card`; none
    /// otherwise. The card's id is not prose and is not shown.
    pub note: Option<String>,
    pub hue: Hue,
    /// Opens the answer card; none for a card this app cannot answer.
    pub action: Option<Action>,
}

/// An owner question's answer card, in the composer's place. Before its body is read it is the
/// question's title and where the read stands; its questions and Send follow once read.
#[derive(Debug, Clone, PartialEq)]
pub struct AnswerCard {
    /// `dock-<card>`.
    pub id: String,
    pub card: String,
    pub title: String,
    pub meta: Option<String>,
    pub body: Option<String>,
    /// The body was clipped: `… more in the transcript`.
    pub more: bool,
    /// One per question, then Review; none for a card of one question.
    pub tabs: Vec<AnswerTab>,
    /// The question the shown tab asks; none on Review.
    pub question: Option<QuestionView>,
    pub review: Option<Review>,
    /// None until the card's body is read.
    pub send: Option<SendAnswers>,
    pub close: Action,
    /// This device's answer is on its way: the card takes no other.
    pub sending: bool,
    /// Why the card cannot be answered from here now.
    pub inert: Option<String>,
    /// The card's body is not here: where its read stands, with Retry after a failure.
    pub read: Option<CardsRead>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnswerTab {
    /// `seg-answer-<n>`, `seg-answer-review`.
    pub id: String,
    pub label: String,
    /// `✓` answered, `○` open.
    pub answered: bool,
    pub selected: bool,
    pub action: Action,
}

#[derive(Debug, Clone, PartialEq)]
pub struct QuestionView {
    pub index: usize,
    pub header: Option<String>,
    pub text: String,
    /// `· choose any`.
    pub multi: bool,
    pub options: Vec<ChoiceOption>,
    /// `Other` or `Answer`: the owner's own words in place of picks.
    pub words: Option<TextRow>,
    /// An optional note on the current picks, only if the host accepts it.
    pub note: Option<TextRow>,
}

/// A text field whose every change is one action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextRow {
    /// `field-answer-<card>-<n>`.
    pub id: String,
    pub label: &'static str,
    pub text: String,
    pub placeholder: &'static str,
    session: Id,
    card: String,
    question: usize,
    note_on_picks: bool,
}

impl TextRow {
    pub(super) fn new(
        session: &Id,
        card: &str,
        question: usize,
        label: &'static str,
        text: String,
        placeholder: &'static str,
    ) -> Self {
        Self {
            id: format!("field-answer-{card}-{question}"),
            label,
            text,
            placeholder,
            session: session.clone(),
            card: card.to_owned(),
            question,
            note_on_picks: false,
        }
    }

    fn note(mut self) -> Self {
        self.id.push_str("-note");
        self.note_on_picks = true;
        self
    }

    /// The action the field's new `text` dispatches.
    pub fn with(&self, text: String) -> Action {
        Action::Dock(if self.note_on_picks {
            DockAction::QuestionNote {
                session: self.session.clone(),
                card: self.card.clone(),
                question: self.question,
                text,
            }
        } else {
            DockAction::Text {
                session: self.session.clone(),
                card: self.card.clone(),
                question: self.question,
                text,
            }
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Review {
    pub rows: Vec<ReviewRow>,
    pub unanswered: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewRow {
    pub question: String,
    /// The answer as it will go out; none: `○ unanswered`.
    pub answer: Option<String>,
    /// Shows the question's tab.
    pub action: Action,
}

/// `Send answers`, or `Go to the first unanswered · 1 left` with `1 unanswered, not sent`.
#[derive(Debug, Clone, PartialEq)]
pub struct SendAnswers {
    pub id: &'static str,
    pub label: String,
    pub note: Option<String>,
    pub ready: bool,
    pub action: Action,
}

fn available(question: &CardQuestion, id: &str) -> bool {
    question
        .options
        .iter()
        .any(|option| option.id == id && option.unavailable.is_none())
}

/// A refreshed card may no longer take a draft as written. Require a new answer rather than
/// silently dropping a choice or sending a pick without the note its owner wrote.
fn valid_draft(question: &CardQuestion, draft: &CardDraft, index: usize) -> bool {
    let picks = draft.picks(index);
    let text_allowed = if picks.is_empty() {
        question.free_text
    } else {
        question.note_on_picks
    };
    (question.multi || picks.len() <= 1)
        && picks.iter().all(|id| available(question, id))
        && (text_allowed || draft.text(index).trim().is_empty())
}

fn answered(question: &CardQuestion, draft: &CardDraft, index: usize) -> bool {
    valid_draft(question, draft, index)
        && (!draft.picks(index).is_empty()
            || !draft.text(index).trim().is_empty()
            || (question.options.is_empty() && !question.free_text))
}

fn unanswered(card: &SessionCard, draft: &CardDraft) -> Vec<usize> {
    card.questions
        .iter()
        .enumerate()
        .filter(|(index, question)| !answered(question, draft, *index))
        .map(|(index, _)| index)
        .collect()
}

/// The answer as the Review tab says it.
fn said(question: &CardQuestion, draft: &CardDraft, index: usize) -> Option<String> {
    if !valid_draft(question, draft, index) {
        return None;
    }
    let mut parts: Vec<String> = draft
        .picks(index)
        .iter()
        .filter_map(|id| question.options.iter().find(|option| option.id == *id))
        .map(|option| option.label.clone())
        .collect();
    let text = draft.text(index).trim();
    if !text.is_empty() {
        parts.push(format!("“{text}”"));
    }
    (!parts.is_empty()).then(|| parts.join(" · "))
}

fn view(
    cx: &Cx<'_>,
    card: &SessionCard,
    index: usize,
    draft: &CardDraft,
    enabled: bool,
) -> Option<QuestionView> {
    let question = card.questions.get(index)?;
    let (picks, text) = if valid_draft(question, draft, index) {
        (draft.picks(index), draft.text(index))
    } else {
        (&[][..], "")
    };
    let pick = |option: &str| {
        Action::Dock(DockAction::Pick {
            session: cx.id.clone(),
            card: card.id.clone(),
            question: index,
            option: option.to_owned(),
        })
    };
    Some(QuestionView {
        index,
        header: question.header.clone(),
        text: question.text.clone(),
        multi: question.multi,
        options: options(&card.id, question, picks, enabled, pick),
        words: question.free_text.then(|| {
            let words_only = question.options.is_empty();
            TextRow::new(
                cx.id,
                &card.id,
                index,
                if words_only { "Answer" } else { "Other" },
                if picks.is_empty() {
                    text.to_owned()
                } else {
                    String::new()
                },
                if words_only {
                    "Your answer…"
                } else {
                    "Answer in your own words instead…"
                },
            )
        }),
        note: (question.note_on_picks && !picks.is_empty()).then(|| {
            TextRow::new(
                cx.id,
                &card.id,
                index,
                "Note",
                text.to_owned(),
                "Optional note on your choice…",
            )
            .note()
        }),
    })
}

/// The answer card of `brief`, from its card when read.
pub(super) fn answer_card(
    cx: &Cx<'_>,
    brief: &CardBrief,
    card: Option<&SessionCard>,
    inert: Option<String>,
) -> AnswerCard {
    match card {
        Some(card) => read_card(cx, brief, card, inert),
        None => waiting_card(
            cx,
            brief.id.clone(),
            brief.title.clone(),
            dock::card_read(cx),
            inert,
        ),
    }
}

/// The answer card of a question whose body is not here: its title, and where its read stands.
pub(super) fn waiting_card(
    cx: &Cx<'_>,
    card: String,
    title: String,
    read: CardsRead,
    inert: Option<String>,
) -> AnswerCard {
    AnswerCard {
        id: format!("dock-{card}"),
        sending: dock::sending(cx.ui, cx.feed, &card),
        card,
        title,
        meta: None,
        body: None,
        more: false,
        tabs: Vec::new(),
        question: None,
        review: None,
        send: None,
        close: Action::Dock(DockAction::Close(cx.id.clone())),
        inert,
        read: Some(read),
    }
}

fn read_card(
    cx: &Cx<'_>,
    brief: &CardBrief,
    card: &SessionCard,
    inert: Option<String>,
) -> AnswerCard {
    let empty = CardDraft::default();
    let draft = cx.ui.dock.drafts.get(&card.id).unwrap_or(&empty);
    let count = card.questions.len();
    let current = if count > 1 { draft.tab.min(count) } else { 0 };
    let sending = dock::sending(cx.ui, cx.feed, &card.id);
    let enabled = !sending && inert.is_none();
    let show = |tab: usize| {
        Action::Dock(DockAction::Show {
            session: cx.id.clone(),
            card: card.id.clone(),
            tab,
        })
    };
    let open = unanswered(card, draft);
    let mut tabs = Vec::new();
    if count > 1 {
        tabs.extend(card.questions.iter().enumerate().map(|(index, question)| {
            AnswerTab {
                id: format!("seg-answer-{index}"),
                label: question
                    .header
                    .clone()
                    .unwrap_or_else(|| format!("Q{}", index + 1)),
                answered: answered(question, draft, index),
                selected: current == index,
                action: show(index),
            }
        }));
        tabs.push(AnswerTab {
            id: "seg-answer-review".into(),
            label: "Review".into(),
            answered: open.is_empty(),
            selected: current == count,
            action: show(count),
        });
    }
    let review = (count > 1 && current == count).then(|| Review {
        rows: card
            .questions
            .iter()
            .enumerate()
            .map(|(index, question)| ReviewRow {
                question: question.text.clone(),
                answer: said(question, draft, index),
                action: show(index),
            })
            .collect(),
        unanswered: open.len(),
    });
    let left = open.len();
    AnswerCard {
        id: format!("dock-{}", card.id),
        card: card.id.clone(),
        title: brief.title.clone(),
        meta: card.meta.clone(),
        body: card.body.clone(),
        more: card.body_more,
        tabs,
        question: view(cx, card, current, draft, enabled),
        review,
        send: Some(SendAnswers {
            id: "btn-send-answers",
            label: if left == 0 {
                "Send answers".into()
            } else {
                format!("Go to the first unanswered · {left} left")
            },
            note: (left > 0).then(|| format!("{left} unanswered, not sent")),
            ready: left == 0 && enabled,
            action: Action::Dock(DockAction::Submit {
                session: cx.id.clone(),
                card: card.id.clone(),
            }),
        }),
        close: Action::Dock(DockAction::Close(cx.id.clone())),
        sending,
        inert,
        read: None,
    }
}

/// The questions dock of the briefed `cards`; none while no question waits.
pub(super) fn dock(cx: &Cx<'_>, cards: &[CardBrief]) -> Option<QuestionDock> {
    let asked: Vec<&CardBrief> = cards
        .iter()
        .filter(|brief| matches!(brief.kind, CardKind::Question | CardKind::Other))
        .collect();
    if asked.is_empty() {
        return None;
    }
    let header = match asked.len() {
        1 => "An owner question is waiting".to_owned(),
        n => format!("{n} owner questions are waiting"),
    };
    let items = asked
        .iter()
        .take(3)
        .map(|brief| {
            let note = match (brief.kind, cx.ui.dock.answered.get(&brief.id)) {
                (CardKind::Other, _) => Some("this app cannot answer this kind of card".to_owned()),
                (_, Some(_)) => Some("answer recorded; delivery pending".to_owned()),
                (_, None) => None,
            };
            QuestionLine {
                id: format!("dock-{}", brief.id),
                card: brief.id.clone(),
                label: brief.title.clone(),
                note,
                hue: Hue::You,
                action: (brief.kind == CardKind::Question).then(|| {
                    Action::Dock(DockAction::Bring {
                        session: cx.id.clone(),
                        card: brief.id.clone(),
                    })
                }),
            }
        })
        .collect();
    Some(QuestionDock {
        header,
        hint: (cx.env.form != Form::Phone).then_some("Alt+A to answer"),
        items,
        more: (asked.len() > 3).then(|| format!("+{} more", asked.len() - 3)),
    })
}

/// Chooses an option: a multi-choice question toggles it; a single choice replaces the pick.
/// Stay on a question that accepts a note, so the note field is available before moving on.
pub(super) fn pick(
    state: &mut State,
    session: &Id,
    card_id: &str,
    question: usize,
    option: String,
    feed: &Feed,
) {
    let Some(card) = dock::card(feed, session.as_str(), card_id) else {
        return;
    };
    let Some(asked) = card.questions.get(question) else {
        return;
    };
    if !available(asked, &option) || dock::sending(state.ui(session.as_str()), feed, card_id) {
        return;
    }
    let count = card.questions.len();
    let draft = state
        .ui_mut(session)
        .dock
        .drafts
        .entry(card_id.to_owned())
        .or_default();
    if !valid_draft(asked, draft, question) {
        draft.picks_mut(question).clear();
        *draft.text_mut(question) = None;
    }
    let had_picks = !draft.picks(question).is_empty();
    if asked.multi {
        let picks = draft.picks_mut(question);
        match picks.iter().position(|picked| *picked == option) {
            Some(at) => {
                picks.remove(at);
            }
            None => picks.push(option),
        }
    } else {
        *draft.picks_mut(question) = vec![option];
        if count > 1 && !asked.note_on_picks {
            draft.tab = question + 1;
        }
    }
    // Do not change a typed Other into a note or send a former note as Other.
    if !asked.note_on_picks || !had_picks || draft.picks(question).is_empty() {
        *draft.text_mut(question) = None;
    }
}

/// Text in Other replaces picks; text in Note can only annotate picks.
pub(super) fn text(
    state: &mut State,
    session: &Id,
    card_id: &str,
    question: usize,
    text: String,
    note: bool,
    feed: &Feed,
) {
    let Some(card) = dock::card(feed, session.as_str(), card_id) else {
        return;
    };
    if dock::sending(state.ui(session.as_str()), feed, card_id) {
        return;
    }
    let asked = card.questions.get(question);
    if card.kind != CardKind::Input
        && !asked.is_some_and(|q| if note { q.note_on_picks } else { q.free_text })
    {
        return;
    }
    let draft = state
        .ui_mut(session)
        .dock
        .drafts
        .entry(card_id.to_owned())
        .or_default();
    let has_picks = !draft.picks(question).is_empty();
    if note && (!has_picks || !asked.is_some_and(|q| valid_draft(q, draft, question))) {
        return;
    }
    if !note && has_picks {
        if text.trim().is_empty() {
            return;
        }
        draft.picks_mut(question).clear();
    }
    *draft.text_mut(question) = Some(text);
}

/// Sends the card's answers, one per question, once every question has one; until then it
/// shows the first unanswered question and sends nothing.
pub(super) fn submit(state: &mut State, session: &Id, card_id: &str, feed: &Feed) -> Vec<Effect> {
    let Some(card) = dock::card(feed, session.as_str(), card_id) else {
        return Vec::new();
    };
    if dock::sending(state.ui(session.as_str()), feed, card_id) {
        return Vec::new();
    }
    let draft = state
        .ui(session.as_str())
        .dock
        .drafts
        .get(card_id)
        .cloned()
        .unwrap_or_default();
    if let Some(first) = unanswered(card, &draft).first() {
        state
            .ui_mut(session)
            .dock
            .drafts
            .entry(card_id.to_owned())
            .or_default()
            .tab = *first;
        return Vec::new();
    }
    let answers = (0..card.questions.len())
        .map(|index| CardAnswer {
            options: draft.picks(index).to_vec(),
            text: Some(draft.text(index).trim().to_owned()).filter(|text| !text.is_empty()),
        })
        .collect();
    dock::send(state, session, card_id, answers, false)
}

/// The oldest card Alt+A opens: an owner question or a docked choice.
fn first_question<'a>(cx: &Cx<'a>) -> Option<&'a CardBrief> {
    cx.state?
        .cards
        .iter()
        .find(|brief| brief.kind == CardKind::Question || dock::docked(brief.kind))
}

/// The Message group's row: answer the waiting questions.
pub fn actions(cx: &Cx<'_>) -> Option<ActionGroup> {
    let brief = first_question(cx)?;
    Some(ActionGroup {
        title: "Message",
        rows: vec![row(
            "answer",
            "Answer the waiting questions",
            Some("Alt+A"),
            Action::Dock(DockAction::Bring {
                session: cx.id.clone(),
                card: brief.id.clone(),
            }),
        )],
    })
}

pub fn keys() -> Vec<KeyBinding> {
    vec![bind("alt+a", "Alt+A", "answer questions", KeyAction::Part)]
}

pub fn press(key: &str, cx: &Cx<'_>) -> Option<Action> {
    if key != "alt+a" {
        return None;
    }
    let brief = first_question(cx)?;
    Some(Action::Dock(DockAction::Bring {
        session: cx.id.clone(),
        card: brief.id.clone(),
    }))
}
