//! Docked choices: the build's exit choice, a child agent's question and a proposed board
//! plan, with the host's options in its order. Each is one line in the dock until the owner
//! opens it. An option the host marks unavailable keeps its row and number, muted with the
//! reason, and cannot be chosen. A destructive option asks once on this device; one that needs
//! the owner's reason puts the composer in reason mode, whose Enter chooses it with the text. A
//! child's question that takes words offers Write my own answer, which puts the composer in
//! answer mode.

use corral_protocol::detail::{CardKind, CardQuestion, SessionCard};
use corral_protocol::session_host::CardAnswer;
use corral_protocol::session_state::CardBrief;

use crate::model::Id;
use crate::model::daemon::Feed;
use crate::model::effect::Effect;
use crate::model::hue::Hue;
use crate::model::nav::Form;

use super::dock::{self, AsideLine, CardsRead, Compose, Confirming, DockAction};
use super::exit::{self, ExitSummary};
use super::{Action, Cx, Focus, State};

/// One option of a card, as the host offers it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChoiceOption {
    /// `opt-<card>-<n>`.
    pub id: String,
    /// The host's option id.
    pub option: String,
    /// 1-based; the keys 1–9 pick the first nine.
    pub number: usize,
    pub label: String,
    pub detail: Option<String>,
    /// Muted with `unavailable: <reason>` (Held); cannot be chosen.
    pub unavailable: Option<String>,
    /// `needs your reason: <why>`: choosing it asks for the owner's reason first.
    pub needs_text: Option<String>,
    /// Asks once on this device before it goes out.
    pub destructive: bool,
    /// Chosen in this device's answer so far.
    pub chosen: bool,
    /// None while it cannot be chosen.
    pub action: Option<Action>,
}

/// The options of `question` of card `card`; `action` chooses one by its id.
pub(super) fn options(
    card: &str,
    question: &CardQuestion,
    picks: &[String],
    enabled: bool,
    action: impl Fn(&str) -> Action,
) -> Vec<ChoiceOption> {
    question
        .options
        .iter()
        .enumerate()
        .map(|(n, option)| ChoiceOption {
            id: format!("opt-{card}-{n}"),
            option: option.id.clone(),
            number: n + 1,
            label: option.label.clone(),
            detail: option.detail.clone(),
            unavailable: option.unavailable.clone(),
            // Only a choice that can be made asks for a reason; an unavailable one says why not.
            needs_text: option
                .needs_text
                .clone()
                .filter(|_| option.unavailable.is_none()),
            destructive: option.destructive,
            chosen: picks.contains(&option.id),
            action: (enabled && option.unavailable.is_none()).then(|| action(&option.id)),
        })
        .collect()
}

/// A docked choice.
#[derive(Debug, Clone, PartialEq)]
pub struct ChoiceCard {
    /// `dock-<card>`.
    pub id: String,
    pub card: String,
    pub kind: CardKind,
    /// `How should auth-refactor leave this session?`, `a2 asks: which API?`.
    pub title: String,
    /// The asking child.
    pub child: Option<String>,
    /// On the right: the exit's landing policy, as the host words it.
    pub policy: Option<String>,
    /// The exit's summary.
    pub summary: Option<ExitSummary>,
    pub body: Option<String>,
    pub more: bool,
    pub options: Vec<ChoiceOption>,
    /// `Write my own answer`: a child's question that takes words.
    pub own: Option<(&'static str, Action)>,
    /// `Not now`: put aside; the host keeps it pending.
    pub not_now: Action,
    /// `↑↓ select  1-3 pick  ⏎ choose  esc not now`; none on the phone.
    pub hint: Vec<(&'static str, &'static str)>,
    pub sending: bool,
    pub inert: Option<String>,
    /// The card's body is not read yet: only its title shows, with where the read stands.
    pub read: Option<CardsRead>,
}

/// The docked choice of `brief`, from its card when read.
pub(super) fn card(
    cx: &Cx<'_>,
    brief: &CardBrief,
    card: Option<&SessionCard>,
    inert: Option<String>,
) -> ChoiceCard {
    let sending = dock::sending(cx.ui, cx.feed, &brief.id);
    let enabled = !sending && inert.is_none();
    let question = card.and_then(|card| card.questions.first());
    let options = question.map_or_else(Vec::new, |question| {
        options(&brief.id, question, &[], enabled, |option| {
            Action::Dock(DockAction::Choose {
                session: cx.id.clone(),
                card: brief.id.clone(),
                option: option.to_owned(),
            })
        })
    });
    let title = title(brief);
    let own = (brief.kind == CardKind::ChildQuestion
        && question.is_some_and(|question| question.free_text)
        && enabled)
        .then(|| {
            (
                "Write my own answer",
                Action::Dock(DockAction::Own {
                    session: cx.id.clone(),
                    card: brief.id.clone(),
                }),
            )
        });
    let hint = if cx.env.form == Form::Phone {
        Vec::new()
    } else {
        let mut hint = vec![("↑↓", "select")];
        if options.len() > 1 {
            hint.push(("1-9", "pick"));
        }
        hint.extend([("⏎", "choose"), ("esc", "not now")]);
        hint
    };
    ChoiceCard {
        id: format!("dock-{}", brief.id),
        card: brief.id.clone(),
        kind: brief.kind,
        title,
        child: brief.child.clone(),
        policy: card
            .filter(|_| brief.kind == CardKind::Exit)
            .and_then(|card| card.meta.clone()),
        summary: (brief.kind == CardKind::Exit)
            .then(|| exit::summary(cx, card))
            .flatten(),
        body: card
            .filter(|_| brief.kind != CardKind::Exit)
            .and_then(|card| card.body.clone()),
        more: card.is_some_and(|card| card.body_more),
        options,
        own,
        not_now: Action::Dock(DockAction::Dismiss {
            session: cx.id.clone(),
            card: brief.id.clone(),
        }),
        hint,
        sending,
        inert,
        read: card.is_none().then(|| dock::card_read(cx)),
    }
}

/// `docs asks: Keep the old guide as a redirect?`, or the card's own title.
fn title(brief: &CardBrief) -> String {
    match (brief.kind, &brief.child) {
        (CardKind::ChildQuestion, Some(child)) => format!("{child} asks: {}", brief.title),
        (CardKind::PlanReview, _) => format!("Plan to review: {}", brief.title),
        _ => brief.title.clone(),
    }
}

/// The one line a docked choice is until the owner opens it:
/// `▲ docs asks: Keep the old guide as a redirect? · Alt+A to answer`. The key is named on the
/// `oldest` card, the one Alt+A opens, where keys show; pressing the line opens it.
pub(super) fn line(cx: &Cx<'_>, brief: &CardBrief, oldest: bool) -> AsideLine {
    let mut text = format!("▲ {}", title(brief));
    if oldest && cx.env.form != Form::Phone {
        text.push_str(match brief.kind {
            CardKind::ChildQuestion => " · Alt+A to answer",
            CardKind::PlanReview => " · Alt+A to review",
            _ => " · Alt+A to choose",
        });
    }
    AsideLine {
        id: format!("dock-{}", brief.id),
        text,
        action: Action::Dock(DockAction::Bring {
            session: cx.id.clone(),
            card: brief.id.clone(),
        }),
    }
}

/// Chooses option `option` of `card`'s first question: a destructive one waits for its
/// confirmation; then one that needs a reason waits for it in the composer; else it goes out.
pub(super) fn choose(
    state: &mut State,
    session: &Id,
    card_id: &str,
    option: &str,
    feed: &Feed,
) -> Vec<Effect> {
    let Some(card) = dock::card(feed, session.as_str(), card_id) else {
        return Vec::new();
    };
    let Some(found) = card
        .questions
        .first()
        .and_then(|question| question.options.iter().find(|found| found.id == option))
    else {
        return Vec::new();
    };
    let now = feed.now;
    if let Some(why) = &found.unavailable {
        state.ui_mut(session).notices.push(
            Hue::Held,
            format!("{}: unavailable: {why}", found.label),
            now,
        );
        return Vec::new();
    }
    if dock::sending(state.ui(session.as_str()), feed, card_id) {
        return Vec::new();
    }
    let chosen = Confirming {
        card: card_id.to_owned(),
        option: found.id.clone(),
        label: found.label.clone(),
        detail: found.detail.clone(),
        needs_text: found.needs_text.clone(),
    };
    if found.destructive {
        let ui = state.ui_mut(session);
        ui.dock.confirming = Some(chosen);
        ui.focus = Focus::Dock;
        return Vec::new();
    }
    proceed(state, session, chosen, now)
}

/// The chosen option, past its confirmation: reason mode when it needs a reason, else sent.
fn proceed(state: &mut State, session: &Id, chosen: Confirming, now: u64) -> Vec<Effect> {
    let Some(why) = chosen.needs_text else {
        return dock::send(
            state,
            session,
            &chosen.card,
            vec![CardAnswer {
                options: vec![chosen.option],
                text: None,
            }],
            false,
        );
    };
    let ui = state.ui_mut(session);
    ui.notices.push(
        Hue::Held,
        format!("{} needs your reason: {why}", chosen.label),
        now,
    );
    ui.dock.compose = Some(Compose::Reason {
        card: chosen.card,
        option: chosen.option,
        label: chosen.label,
        why,
    });
    ui.focus = Focus::Composer;
    Vec::new()
}

/// Confirms (or cancels) the destructive option waiting for its confirmation.
pub(super) fn confirm(state: &mut State, session: &Id, yes: bool, feed: &Feed) -> Vec<Effect> {
    let Some(chosen) = state.ui_mut(session).dock.confirming.take() else {
        return Vec::new();
    };
    if !yes || dock::sending(state.ui(session.as_str()), feed, &chosen.card) {
        return Vec::new();
    }
    proceed(state, session, chosen, feed.now)
}

/// Write my own answer: the composer's Enter answers the child's question.
pub(super) fn own(state: &mut State, session: &Id, card: String, feed: &Feed) {
    let child = dock::brief(feed, session.as_str(), &card).and_then(|brief| brief.child.clone());
    let ui = state.ui_mut(session);
    ui.dock.compose = Some(Compose::Answer { card, child });
    ui.focus = Focus::Composer;
}
