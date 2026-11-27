//! Prompt history: ↑ in an empty composer recalls this session's earlier owner messages, newest
//! first, and ↓ walks back toward the empty draft, as Pi's editor does. The entries are what the
//! session already holds: the owner's messages in the served transcript (their `!` commands
//! too) and this device's sends it does not serve yet; nothing is stored apart. ↑ ↓ are the
//! history's only while the draft is empty or still the entry recalled, and while no picker, `/`
//! menu, `@` popup or dock mode takes them.

use corral_protocol::detail::{DetailRef, SessionItem, SessionMessage, SessionPage as Served};
use corral_protocol::session_state::InputOrigin;

use crate::model::Id;
use crate::model::daemon::Feed;
use crate::model::effect::Effect;

use super::{Action, Cx, Focus, Sent, State, transcript};

/// What ↑ and ↓ do in the composer now; none where they move the caret as usual.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HistoryKeys {
    pub older: Option<Action>,
    pub newer: Option<Action>,
}

fn owner_author(author: &str) -> bool {
    let who = author.to_ascii_lowercase();
    who == "you" || who == "user" || who.starts_with("owner")
}

/// What the owner typed in `message`: their words, or the `!` command they ran; none for what
/// the host, an agent or an automation said.
fn typed(message: &SessionMessage) -> Option<String> {
    let typed = message
        .input
        .as_ref()
        .is_some_and(|input| matches!(input.origin, InputOrigin::Owner | InputOrigin::Pane));
    let mut words: Vec<String> = Vec::new();
    for item in &message.items {
        match item {
            SessionItem::Text { text } if typed => words.push(text.clone()),
            SessionItem::BashExecution {
                command,
                exclude_from_context,
                ..
            } if typed || owner_author(&message.author) => words.push(format!(
                "{}{command}",
                if *exclude_from_context { "!!" } else { "!" }
            )),
            _ => {}
        }
    }
    let text = words.join("\n").trim().to_owned();
    (!text.is_empty()).then_some(text)
}

/// The session's owner entries, newest first, without a repeat of the one before.
pub fn entries(page: Option<&Served>, echoes: &[Sent]) -> Vec<String> {
    let served = page.map_or(&[][..], |page| page.messages.as_slice());
    let mut all: Vec<String> = served.iter().filter_map(typed).collect();
    all.extend(echoes.iter().map(|sent| {
        sent.restore
            .clone()
            .unwrap_or_else(|| sent.text.clone())
            .trim()
            .to_owned()
    }));
    all.retain(|text| !text.is_empty());
    all.dedup();
    all.reverse();
    all
}

fn of(feed: &Feed, session: &Id, echoes: &[Sent]) -> Vec<String> {
    let page = transcript::page(feed.detail(&DetailRef::Session {
        id: session.to_string(),
    }));
    entries(page, echoes)
}

/// ↑ and ↓ for the composer of `cx`; `menus` says a picker, the `/` menu or the `@` popup has
/// them.
pub(super) fn keys(cx: &Cx<'_>, menus: bool) -> HistoryKeys {
    let ui = cx.ui;
    if menus || ui.focus != Focus::Composer || ui.dock.compose.is_some() {
        return HistoryKeys::default();
    }
    let entries = of(cx.feed, cx.id, &ui.echoes);
    let walking = match ui.recall {
        None => ui.draft.is_empty(),
        Some(at) => entries.get(at).is_some_and(|entry| *entry == ui.draft),
    };
    if !walking {
        return HistoryKeys::default();
    }
    let next = ui.recall.map_or(0, |at| at + 1);
    HistoryKeys {
        older: (next < entries.len()).then(|| Action::Recall {
            session: cx.id.clone(),
            older: true,
        }),
        newer: ui.recall.map(|_| Action::Recall {
            session: cx.id.clone(),
            older: false,
        }),
    }
}

/// Puts the entry before (`older`) or after the recalled one in the draft; past the newest, the
/// empty draft.
pub(super) fn recall(state: &mut State, session: &Id, older: bool, feed: &Feed) -> Vec<Effect> {
    let ui = state.ui_mut(session);
    let entries = of(feed, session, &ui.echoes);
    let next = match (ui.recall, older) {
        (None, true) => Some(0),
        (Some(at), true) => Some(at + 1),
        (None, false) | (Some(0), false) => None,
        (Some(at), false) => Some(at - 1),
    };
    match next {
        Some(at) if at < entries.len() => {
            ui.draft = entries[at].clone();
            ui.recall = Some(at);
        }
        Some(_) => {}
        None => {
            ui.draft.clear();
            ui.recall = None;
        }
    }
    ui.picker.slash_at = 0;
    ui.mention.at = 0;
    Vec::new()
}

#[cfg(test)]
mod tests {
    use corral_protocol::detail::{DetailBody, HostPresence, InputFacts};

    use super::*;
    use crate::model::daemon::{DetailCache, detail_key};

    fn said(author: &str, origin: Option<InputOrigin>, text: &str) -> SessionMessage {
        SessionMessage {
            id: String::new(),
            author: author.into(),
            at: None,
            items: vec![SessionItem::Text { text: text.into() }],
            model: None,
            stop_reason: None,
            usage: None,
            input: origin.map(|origin| InputFacts {
                id: None,
                origin,
                typed_at: None,
                device: None,
                to: None,
            }),
            attachments: Vec::new(),
        }
    }

    /// ↑ in an empty composer did nothing, so the owner retyped what they had just sent.
    #[test]
    fn up_on_an_empty_draft_recalls_the_last_owner_message() {
        let id = Id::from("A-1");
        let target = DetailRef::Session { id: id.to_string() };
        let mut feed = Feed::default();
        feed.details.insert(
            detail_key(&target),
            DetailCache {
                transcript_seams: Default::default(),
                target: target.clone(),
                pages: Vec::new(),
                loading: false,
                stale: false,
                error: None,
                merged: Some(DetailBody::Session(Served {
                    id: id.to_string(),
                    role: "assistant".into(),
                    task: None,
                    run: None,
                    machine: None,
                    messages: vec![
                        said("you", Some(InputOrigin::Owner), "first"),
                        said("assistant", None, "a reply"),
                        said("you", Some(InputOrigin::Owner), "second"),
                        said("assistant", None, "another reply"),
                    ],
                    older: None,
                    replyable: true,
                    reply_refusal: None,
                    host: HostPresence::Running,
                })),
            },
        );
        let mut state = State::default();
        state.act(
            Action::Recall {
                session: id.clone(),
                older: true,
            },
            &feed,
        );
        assert_eq!(state.ui(id.as_str()).draft, "second");
    }
}
