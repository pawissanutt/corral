//! Decision cards: what the session's extensions ask through Pi's select, confirm and input
//! dialogs, served as cards since a headless session has no terminal to draw them. One card at a
//! time takes the composer's place, with the caller's title, its body (an edit approval shows
//! its exact text), the caller's options numbered, or one text row, and the time left before the
//! caller gives up. The card adds no option or default of its own: Esc, the close control and a
//! timeout answer nothing, which the caller reads as no.

use corral_protocol::detail::{CardKind, SessionCard};
use corral_protocol::session_host::CardAnswer;
use corral_protocol::session_state::CardBrief;

use crate::model::Id;
use crate::model::daemon::Feed;
use crate::model::effect::Effect;
use crate::model::hue::Hue;
use crate::model::nav::Form;

use super::choice::{ChoiceOption, options};
use super::dock::{self, DockAction};
use super::questions::TextRow;
use super::{Action, Cx, State};

/// For this long after a decision card appears, Enter, digits and presses on its options are
/// ignored, so a key meant for what was there before does not answer it. The screen that draws
/// the card keeps the time it appeared.
pub const OPEN_GUARD_MS: u64 = 400;

/// From this many seconds left the countdown is drawn Held.
const HURRY_SECONDS: u64 = 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecisionKind {
    Select,
    Confirm,
    Input,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DecisionCard {
    /// `dock-<card>`.
    pub id: String,
    pub card: String,
    pub kind: DecisionKind,
    /// Bold in the head: the caller's question.
    pub title: String,
    /// The caller's message; scrolls.
    pub body: Option<String>,
    /// The body was clipped by the host.
    pub more: bool,
    /// Numbered options (select, confirm).
    pub options: Vec<ChoiceOption>,
    /// The option the caller marks as its initial one: where the selection starts.
    pub selected: Option<usize>,
    /// One text row (input).
    pub input: Option<TextRow>,
    /// `Use` (input): sends the row's text.
    pub submit: Option<Action>,
    /// `answer in 4:58`, Held from 30 s; `time is up` once the caller gave up.
    pub countdown: Option<(Hue, String)>,
    /// Pressable key caps: `⏎ choose  esc no`; none on the phone.
    pub hint: Vec<(&'static str, &'static str)>,
    /// Esc and the close control: answers nothing.
    pub dismiss: Action,
    pub sending: bool,
    pub inert: Option<String>,
    /// The card's body is not read yet: only its title shows.
    pub reading: bool,
}

const fn kind(kind: CardKind) -> DecisionKind {
    match kind {
        CardKind::Confirm => DecisionKind::Confirm,
        CardKind::Input => DecisionKind::Input,
        _ => DecisionKind::Select,
    }
}

fn countdown(expires: Option<&str>, now: u64) -> Option<(Hue, String)> {
    let at = crate::model::agents::unix(expires?)?;
    if now == 0 {
        return None;
    }
    let left = at.saturating_sub(now);
    if left == 0 {
        return Some((Hue::Held, "time is up".into()));
    }
    let hue = if left <= HURRY_SECONDS {
        Hue::Held
    } else {
        Hue::Neutral
    };
    Some((hue, format!("answer in {}:{:02}", left / 60, left % 60)))
}

/// The decision card of `brief`, from its card when read.
pub(super) fn card(
    cx: &Cx<'_>,
    brief: &CardBrief,
    card: Option<&SessionCard>,
    inert: Option<String>,
) -> DecisionCard {
    let kind = kind(brief.kind);
    let sending = dock::sending(cx.ui, cx.feed, &brief.id);
    let enabled = !sending && inert.is_none();
    let question = card.and_then(|card| card.questions.first());
    let options = match (kind, question) {
        (DecisionKind::Input, _) | (_, None) => Vec::new(),
        (_, Some(question)) => options(&brief.id, question, &[], enabled, |option| {
            Action::Dock(DockAction::Choose {
                session: cx.id.clone(),
                card: brief.id.clone(),
                option: option.to_owned(),
            })
        }),
    };
    let selected = question.and_then(|question| {
        let initial = question.initial.as_deref()?;
        question
            .options
            .iter()
            .position(|option| option.id == initial)
    });
    let input = (kind == DecisionKind::Input && card.is_some()).then(|| {
        let typed = cx
            .ui
            .dock
            .drafts
            .get(&brief.id)
            .and_then(|draft| draft.typed(0).map(str::to_owned));
        let initial = question.and_then(|question| question.initial.clone());
        TextRow::new(
            cx.id,
            &brief.id,
            0,
            "Answer",
            typed.or(initial).unwrap_or_default(),
            "",
        )
    });
    let submit = input.as_ref().filter(|_| enabled).map(|_| {
        Action::Dock(DockAction::Submit {
            session: cx.id.clone(),
            card: brief.id.clone(),
        })
    });
    let hint = if cx.env.form == Form::Phone {
        Vec::new()
    } else {
        match kind {
            DecisionKind::Select => vec![
                ("↑↓", "select"),
                ("1-9", "pick"),
                ("⏎", "choose"),
                ("esc", "cancel"),
            ],
            DecisionKind::Confirm => vec![("⏎", "choose"), ("esc", "no")],
            DecisionKind::Input => vec![("⏎", "use"), ("esc", "cancel")],
        }
    };
    DecisionCard {
        id: format!("dock-{}", brief.id),
        card: brief.id.clone(),
        kind,
        title: brief.title.clone(),
        body: card.and_then(|card| card.body.clone()),
        more: card.is_some_and(|card| card.body_more),
        options,
        selected,
        input,
        submit,
        countdown: countdown(
            card.and_then(|card| card.expires_at.as_deref())
                .or(brief.expires_at.as_deref()),
            cx.feed.now,
        ),
        hint,
        dismiss: Action::Dock(DockAction::Dismiss {
            session: cx.id.clone(),
            card: brief.id.clone(),
        }),
        sending,
        inert,
        reading: card.is_none(),
    }
}

/// Use: sends the input card's text as typed, or the caller's initial text when none was.
pub(super) fn submit(state: &mut State, session: &Id, card_id: &str, feed: &Feed) -> Vec<Effect> {
    let Some(card) = dock::card(feed, session.as_str(), card_id) else {
        return Vec::new();
    };
    if dock::sending(state.ui(session.as_str()), feed, card_id) {
        return Vec::new();
    }
    let text = state
        .ui(session.as_str())
        .dock
        .drafts
        .get(card_id)
        .and_then(|draft| draft.typed(0).map(str::to_owned))
        .or_else(|| {
            card.questions
                .first()
                .and_then(|question| question.initial.clone())
        })
        .unwrap_or_default();
    dock::send(
        state,
        session,
        card_id,
        vec![CardAnswer {
            options: Vec::new(),
            text: Some(text),
        }],
        false,
    )
}
