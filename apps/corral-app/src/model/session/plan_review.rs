//! Plan review: the plan a session in plan mode submitted, waiting for the owner's choice. Its
//! card (`plan_review`) is one line in the dock until the owner opens it; open, the whole plan
//! document takes the conversation's place and the dock holds the host's options (Execute, with
//! a clean or a compacted context, Keep planning), the session's model with the way to change
//! it, and one text field: the owner's feedback with Keep planning, a note to the executor with
//! an execute option. Keep planning asks for that feedback first; choosing it again, or Enter in
//! the field, sends it, words or none.
//!
//! The document is read whole from the host (`SessionPlanModeDoc` of the review's card, paged,
//! joined in offset order) and read again when the host's cards change. Only the read made for
//! the card under review is drawn: a newer submission has a new card, so its view never shows
//! an earlier one's text. A host that does not serve it, or a read that fails, leaves the card's
//! own body, which holds the plan's first 8 KiB, with why. An execute option runs the whole
//! plan, so it cannot be chosen until the whole plan is shown: read whole for this card, or
//! carried whole by the card (a body the host did not clip), and only while the session's page
//! has that document in the conversation's place. Keep planning and Not now always can. Ask, a
//! Home card and the Sessions inspector draw no document: there a review stays its line, which
//! opens the session's page with the review open.

use corral_protocol::detail::{CardKind, DetailBody, DetailRef, PlanModeDocPage, SessionCard};
use corral_protocol::session_host::CardAnswer;
use corral_protocol::session_state::CardBrief;

use crate::model::Id;
use crate::model::daemon::Feed;
use crate::model::effect::{Effect, Env};
use crate::model::hue::Hue;
use crate::model::nav::Form;

use super::choice::{self, ChoiceOption};
use super::dock::{self, Asked, CardsRead, DockAction};
use super::questions::TextRow;
use super::tabs::load;
use super::{Action, Can, Cx, Focus, Host, PickerAction, PickerKind, SessionPage, State, Tab, Ui};

/// The host part the whole document is served by.
const CAPABILITY: &str = "plan_mode_doc";
/// The option that returns to planning: its text is the owner's feedback.
const REVISE: &str = "revise";
const UNKNOWN_MODEL: &str = "the execution model has not been reported";

/// The document of one submission: the host serves, under the review card's id, exactly the
/// text an execute answer to that card runs, so another submission's read never answers it.
pub fn target(session: &str, card: &str) -> DetailRef {
    DetailRef::SessionPlanModeDoc {
        id: session.to_owned(),
        card: card.to_owned(),
    }
}

/// The plan document submitted with `card`, once every byte of it has been read.
fn whole<'f>(feed: &'f Feed, session: &str, card: &str) -> Option<&'f PlanModeDocPage> {
    match feed.detail(&target(session, card))?.body()? {
        DetailBody::SessionPlanModeDoc(page)
            if page.offset == 0 && page.body.len() as u64 == page.bytes =>
        {
            Some(page)
        }
        _ => None,
    }
}

/// Why an execute option of `card` cannot be chosen: an execute answer runs the whole plan, so
/// it waits until the whole plan is shown, either read whole for this card or carried whole by
/// the card itself. None once it is.
fn unread(feed: &Feed, session: &str, card: &SessionCard) -> Option<&'static str> {
    if whole(feed, session, &card.id).is_some() || (card.body.is_some() && !card.body_more) {
        return None;
    }
    let failed = feed
        .detail(&target(session, &card.id))
        .is_some_and(|cache| cache.error.is_some());
    Some(if failed {
        "the whole plan could not be read; retry, or answer in the terminal"
    } else {
        "the whole plan is not read yet"
    })
}

/// Why an execute option of `card` cannot be chosen where its plan is not drawn: the owner
/// approves what is shown, and only the session's page shows the opened review's document, in
/// the conversation's place. None while it does.
fn hidden(ui: &Ui, card: &str) -> Option<&'static str> {
    (ui.tab != Tab::Chat || ui.sub.is_some() || !ui.dock.opened.contains(card))
        .then_some("the plan is not shown; go back to the conversation to read it")
}

/// Session `id`'s page as its compact forms draw it (Ask, a Home card, the Sessions inspector):
/// on its chat, its own UI state read as if Chat were shown, the stored tab and view untouched.
/// None of them draws a plan document, so a plan review opened on the session's page is its
/// line here, and that line opens the page.
pub fn compact(feed: &Feed, state: &State, id: &Id, env: Env) -> SessionPage {
    let ui = state.ui(id.as_str());
    let reviews = || {
        dock::briefs(feed, id.as_str())
            .iter()
            .filter(|brief| brief.kind == CardKind::PlanReview)
    };
    let reviewing = reviews().any(|brief| ui.dock.opened.contains(&brief.id));
    let mut page = if ui.tab == Tab::Chat && ui.sub.is_none() && !reviewing {
        super::page(feed, state, id, env)
    } else {
        let mut chat = ui.clone();
        chat.tab = Tab::Chat;
        chat.sub = None;
        chat.focus = Focus::Composer;
        for brief in reviews() {
            chat.dock.opened.remove(&brief.id);
        }
        let mut shown = State::default();
        shown.sessions.insert(id.clone(), chat);
        super::page(feed, &shown, id, env)
    };
    for brief in reviews() {
        let open = Action::Dock(DockAction::Answer {
            session: page.id.clone(),
            asked: Asked::Card(brief.id.clone()),
        });
        let bring = Action::Dock(DockAction::Bring {
            session: page.id.clone(),
            card: brief.id.clone(),
        });
        if page.dock.oldest.as_ref() == Some(&bring) {
            page.dock.oldest = Some(open.clone());
        }
        let id = format!("dock-{}", brief.id);
        if let Some(line) = page.dock.waiting.iter_mut().find(|line| line.id == id) {
            line.text = format!(
                "▲ Plan to review: {} · open the session to review",
                brief.title
            );
            line.action = open;
        }
    }
    page
}

/// The plan review the owner opened, as the host's state briefs it.
fn opened<'a>(cx: &Cx<'a>) -> Option<&'a CardBrief> {
    cx.state?
        .cards
        .iter()
        .find(|brief| brief.kind == CardKind::PlanReview && cx.ui.dock.opened.contains(&brief.id))
}

/// The plan document in the conversation's place.
#[derive(Debug, Clone, PartialEq)]
pub struct PlanDoc {
    pub title: String,
    /// `revision 2 · anthropic/claude-opus-5-5`: only the facts reported.
    pub facts: String,
    /// The plan's Markdown: the whole document once read, else what the card carries.
    pub body: Option<String>,
    /// Where the read of the whole document stands, or why the card's part is all there is.
    pub note: Option<(Hue, String)>,
    pub retry: Option<Action>,
    /// Back to the conversation; the card stays pending.
    pub back: Action,
}

fn not_now(cx: &Cx<'_>, brief: &CardBrief) -> Action {
    Action::Dock(DockAction::Dismiss {
        session: cx.id.clone(),
        card: brief.id.clone(),
    })
}

/// Why the whole document is not shown, with Retry after a failed read.
fn standing(cx: &Cx<'_>, card: &str, clipped: bool) -> (Option<(Hue, String)>, Option<Action>) {
    let target = target(cx.id.as_str(), card);
    let rest = if clipped {
        "this is its start, and the rest is in the session's terminal"
    } else {
        "this is the part its card carries"
    };
    if let Some(error) = cx
        .feed
        .detail(&target)
        .and_then(|cache| cache.error.as_ref())
    {
        let why = if error.message.is_empty() {
            error.code.as_str()
        } else {
            error.message.as_str()
        };
        return (
            Some((
                Hue::Fail,
                format!("The whole plan could not be read ({why}); {rest}."),
            )),
            Some(Action::Refresh(target)),
        );
    }
    let unserved = match cx.can(CAPABILITY) {
        Can::Absent => Some("This host does not serve the whole plan".to_owned()),
        Can::Unavailable(why) => Some(format!("The whole plan cannot be read now ({why})")),
        Can::Yes | Can::Unknown if cx.host != Host::Running => {
            Some("The whole plan can be read once the host runs".to_owned())
        }
        Can::Yes | Can::Unknown => None,
    };
    match unserved {
        Some(why) => (Some((Hue::Held, format!("{why}; {rest}."))), None),
        None => (Some((Hue::Neutral, "Reading the whole plan…".into())), None),
    }
}

/// The document of the plan review the owner opened; none while none is open.
pub(super) fn doc(cx: &Cx<'_>) -> Option<PlanDoc> {
    let brief = opened(cx)?;
    let card = dock::card(cx.feed, cx.id.as_str(), &brief.id);
    let page = whole(cx.feed, cx.id.as_str(), &brief.id);
    let revision = cx
        .state
        .and_then(|state| state.plan_mode.as_ref())
        .and_then(|plan| plan.revision)
        .or_else(|| page.map(|page| page.revision));
    let facts: Vec<String> = [revision.map(|n| format!("revision {n}")), cx.model()]
        .into_iter()
        .flatten()
        .collect();
    let (body, (note, retry)) = match page {
        Some(page) => (Some(page.body.clone()), (None, None)),
        None => (
            card.and_then(|card| card.body.clone()),
            standing(cx, &brief.id, card.is_some_and(|card| card.body_more)),
        ),
    };
    Some(PlanDoc {
        title: brief.title.clone(),
        facts: facts.join(" · "),
        body: body.filter(|body| !body.trim().is_empty()),
        note,
        retry,
        back: not_now(cx, brief),
    })
}

/// The read of the whole document while its review is open: once, while the host serves it.
pub(super) fn fetches(cx: &Cx<'_>) -> Option<Action> {
    let brief = opened(cx)?;
    load(cx, Ok(target(cx.id.as_str(), &brief.id)), Some(CAPABILITY))
}

/// The opened review's choices in the dock.
#[derive(Debug, Clone, PartialEq)]
pub struct PlanReview {
    /// `dock-<card>`.
    pub id: String,
    pub card: String,
    /// The card's question: `How should this plan proceed?`.
    pub title: String,
    /// `Model: anthropic/claude-opus-5-5`; an execute option runs on it.
    pub model: String,
    /// Opens the session's model list.
    pub change: Action,
    pub options: Vec<ChoiceOption>,
    /// The owner's feedback or note.
    pub words: TextRow,
    /// Keep planning waits for the owner's feedback: the field takes the keys.
    pub asking: bool,
    /// Enter in the field: sends Keep planning with its text.
    pub submit: Option<Action>,
    pub not_now: Action,
    pub hint: Vec<(&'static str, &'static str)>,
    pub sending: bool,
    pub inert: Option<String>,
    pub read: Option<CardsRead>,
}

/// The choices of the plan review the owner opened, from its card when read.
pub(super) fn review(cx: &Cx<'_>, inert: Option<String>) -> Option<PlanReview> {
    let brief = opened(cx)?;
    let card = dock::card(cx.feed, cx.id.as_str(), &brief.id);
    let sending = dock::sending(cx.ui, cx.feed, &brief.id);
    let enabled = !sending && inert.is_none();
    let draft = cx.ui.dock.drafts.get(&brief.id);
    let picks = draft.map(|draft| draft.picks(0)).unwrap_or_default();
    let question = card.and_then(|card| card.questions.first());
    let model = cx.model().filter(|model| !model.trim().is_empty());
    let mut options = question.map_or_else(Vec::new, |question| {
        choice::options(&brief.id, question, picks, enabled, |option| {
            Action::Dock(DockAction::ChoosePlan {
                session: cx.id.clone(),
                card: brief.id.clone(),
                option: option.to_owned(),
                expected_model: model.as_ref().filter(|_| option != REVISE).cloned(),
            })
        })
    });
    let unread = card
        .and_then(|card| unread(cx.feed, cx.id.as_str(), card))
        .map(|why| match standing(cx, &brief.id, true).0 {
            // A read that will not come: the note over the plan says why.
            Some((Hue::Held, _)) => "the whole plan cannot be read here; answer in the terminal",
            _ => why,
        });
    let unread = hidden(cx.ui, &brief.id)
        .or(unread)
        .or_else(|| model.is_none().then_some(UNKNOWN_MODEL));
    if let Some(why) = unread {
        for option in options
            .iter_mut()
            .filter(|option| option.option != REVISE && option.unavailable.is_none())
        {
            option.unavailable = Some(why.to_owned());
            option.needs_text = None;
            option.action = None;
        }
    }
    let asking = picks.iter().any(|pick| pick == REVISE);
    let (label, placeholder) = if asking {
        ("Feedback", "What should change? Enter sends it, empty too")
    } else {
        ("Note", "Optional: for the executor, or what should change")
    };
    let hint = if cx.env.form == Form::Phone {
        Vec::new()
    } else {
        vec![
            ("↑↓", "select"),
            ("1-9", "pick"),
            ("⏎", "choose"),
            ("esc", "not now"),
        ]
    };
    Some(PlanReview {
        id: format!("dock-{}", brief.id),
        card: brief.id.clone(),
        title: question.map_or_else(|| brief.title.clone(), |question| question.text.clone()),
        model: format!("Model: {}", model.as_deref().unwrap_or("not reported")),
        change: Action::Picker(PickerAction::Open {
            session: cx.id.clone(),
            kind: PickerKind::Model,
        }),
        options,
        words: TextRow::new(
            cx.id,
            &brief.id,
            0,
            label,
            draft
                .map(|draft| draft.text(0).to_owned())
                .unwrap_or_default(),
            placeholder,
        ),
        asking,
        submit: (enabled && card.is_some()).then(|| {
            Action::Dock(DockAction::Submit {
                session: cx.id.clone(),
                card: brief.id.clone(),
            })
        }),
        not_now: not_now(cx, brief),
        hint,
        sending,
        inert,
        read: card.is_none().then(|| dock::card_read(cx)),
    })
}

/// The answer's option and the owner's text, when any was typed.
fn answer(option: &str, text: &str) -> Vec<CardAnswer> {
    let text = text.trim();
    vec![CardAnswer {
        options: vec![option.to_owned()],
        text: (!text.is_empty()).then(|| text.to_owned()),
    }]
}

fn typed(state: &State, session: &Id, card: &str) -> (bool, String) {
    let draft = state.ui(session.as_str()).dock.drafts.get(card);
    (
        draft.is_some_and(|draft| draft.picks(0).iter().any(|pick| pick == REVISE)),
        draft
            .map(|draft| draft.text(0).to_owned())
            .unwrap_or_default(),
    )
}

/// Chooses an option of the review: an execute option goes out with the note typed so far;
/// Keep planning with no feedback yet asks for it first, and goes out when chosen again.
pub(super) fn choose(
    state: &mut State,
    session: &Id,
    card_id: &str,
    option: &str,
    expected_model: Option<String>,
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
    // Only Keep planning may answer a plan that is not shown whole, whatever chose the option.
    let unread = (option != REVISE)
        .then(|| {
            hidden(state.ui(session.as_str()), card_id)
                .or_else(|| unread(feed, session.as_str(), card))
                .or_else(|| expected_model.is_none().then_some(UNKNOWN_MODEL))
        })
        .flatten();
    if let Some(why) = found.unavailable.as_deref().or(unread) {
        state.ui_mut(session).notices.push(
            Hue::Held,
            format!("{}: unavailable: {why}", found.label),
            feed.now,
        );
        return Vec::new();
    }
    if dock::sending(state.ui(session.as_str()), feed, card_id) {
        return Vec::new();
    }
    let (asking, text) = typed(state, session, card_id);
    if option == REVISE && !asking && text.trim().is_empty() {
        let ui = state.ui_mut(session);
        *ui.dock
            .drafts
            .entry(card_id.to_owned())
            .or_default()
            .picks_mut(0) = vec![REVISE.to_owned()];
        ui.focus = Focus::Dock;
        return Vec::new();
    }
    dock::send_with_model(
        state,
        session,
        card_id,
        answer(option, &text),
        false,
        expected_model.filter(|_| option != REVISE),
    )
}

/// The owner's feedback or note; an option waiting for it stays chosen.
pub(super) fn text(state: &mut State, session: &Id, card: &str, text: String) {
    *state
        .ui_mut(session)
        .dock
        .drafts
        .entry(card.to_owned())
        .or_default()
        .text_mut(0) = Some(text);
}

/// Enter in the field: Keep planning with the text, which the host also reads from an answer
/// that names no option. Nothing typed and nothing asked for sends nothing.
pub(super) fn submit(state: &mut State, session: &Id, card: &str, feed: &Feed) -> Vec<Effect> {
    let (asking, text) = typed(state, session, card);
    if (!asking && text.trim().is_empty()) || dock::sending(state.ui(session.as_str()), feed, card)
    {
        return Vec::new();
    }
    dock::send(state, session, card, answer(REVISE, &text), false)
}
