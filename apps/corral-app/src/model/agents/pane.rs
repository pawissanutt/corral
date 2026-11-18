//! A run's herdr pane from the app: named keys and literal text typed into it (`pane.keys`), and
//! Focus pane (`pane.focus`), which brings it forward on corrald's machine. Each request leaves
//! under its own request id, and the session shows how its latest one ended, so a failure is
//! never lost. A headless (process) session has a log, not a pane: it takes no keys.

use corral_protocol::pane::{PaneFocus, PaneKeys};
use corral_protocol::rows::AgentRow;
use corral_types::SessionRef;

use crate::model::Id;
use crate::model::daemon;
use crate::model::effect::{Effect, PaneRequest};
use crate::model::hue::Hue;
use crate::model::id::IdSupply;

use super::live::{placement, session_name};
use super::{Action, State};

/// The keys bar: herdr's names for the keys an agent's screen asks for, and their caps.
pub const KEYS: [(&str, &str); 6] = [
    ("enter", "Enter"),
    ("esc", "Esc"),
    ("tab", "Tab"),
    ("up", "↑"),
    ("down", "↓"),
    ("ctrl+c", "Ctrl+C"),
];

/// Typed text is shown this many characters long where its outcome is named.
const SHOWN: usize = 32;

/// What a pane request asked for, as its outcome names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaneAsk {
    Focus,
    /// The text and keys sent, as read (`“y”, then Enter`); `text` when words were typed, so
    /// the field clears once they are delivered.
    Keys {
        shown: String,
        text: bool,
    },
}

/// The latest pane request of one run session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneSent {
    pub key: Id,
    pub ask: PaneAsk,
    /// Why it never left this device, when it did not.
    pub unsent: Option<String>,
}

/// One key of the keys bar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneKey {
    pub cap: &'static str,
    pub test_id: String,
    pub action: Action,
}

/// The keys bar and the type field under a pane session's terminal; a headless session's says
/// why it takes no keys instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneInput {
    pub session: Id,
    pub keys: Vec<PaneKey>,
    pub placeholder: String,
    pub accessibility_label: &'static str,
    pub type_label: &'static str,
    pub enter_label: &'static str,
    pub hint: String,
    /// Why this session takes no keys; then neither keys nor field are drawn.
    pub closed: Option<String>,
    /// How the latest pane request ended, in its tone.
    pub status: Option<(Hue, String)>,
    /// The key of the latest delivered typed text: the field clears once when it changes.
    pub delivered: Option<String>,
}

impl State {
    /// Remembers the pane request `key` of `session` as the one whose outcome the session
    /// shows; `unsent` when it could not leave this device.
    pub fn pane_sent(&mut self, session: Id, key: Id, ask: PaneAsk, unsent: Option<String>) {
        self.pane.insert(session, PaneSent { key, ask, unsent });
    }

    /// Types `text` literally into the run's pane, then presses `keys`; nothing to send sends
    /// nothing.
    pub(super) fn type_into(
        &mut self,
        session: Id,
        text: String,
        keys: Vec<String>,
        feed: &daemon::Feed,
    ) -> Vec<Effect> {
        let text = Some(text).filter(|text| !text.trim().is_empty());
        if text.is_none() && keys.is_empty() {
            return Vec::new();
        }
        let Some(row) = feed.agents.get(session.as_str()) else {
            return vec![gone(&session)];
        };
        let ask = PaneAsk::Keys {
            shown: shown(text.as_deref(), &keys),
            text: text.is_some(),
        };
        vec![Effect::Pane {
            key: self.ids.next_id(),
            what: ask,
            request: PaneRequest::Keys(PaneKeys {
                target: target(row),
                keys,
                text,
            }),
            session,
        }]
    }
}

/// Focus pane for `session`, under a key of `ids`.
pub fn focus(ids: &mut IdSupply, feed: &daemon::Feed, session: Id) -> Effect {
    match feed.agents.get(session.as_str()) {
        Some(row) => Effect::Pane {
            key: ids.next_id(),
            what: PaneAsk::Focus,
            request: PaneRequest::Focus(target(row)),
            session,
        },
        None => gone(&session),
    }
}

fn gone(session: &Id) -> Effect {
    Effect::Toast(format!(
        "corrald lists no live run {session} now; nothing was sent."
    ))
}

fn target(row: &AgentRow) -> PaneFocus {
    PaneFocus {
        task: row.run.task.clone(),
        run: row.run.id.clone(),
    }
}

/// `“y”, then Enter`, `Ctrl+C`: what was sent, as its outcome names it.
fn shown(text: Option<&str>, keys: &[String]) -> String {
    let caps = keys.iter().map(|key| {
        KEYS.iter()
            .find(|(name, _)| name == key)
            .map_or(key.as_str(), |(_, cap)| cap)
    });
    let caps = caps.collect::<Vec<_>>().join(", ");
    match text {
        Some(text) => {
            let mut quoted: String = text.chars().take(SHOWN).collect();
            if text.chars().count() > SHOWN {
                quoted.push('…');
            }
            if caps.is_empty() {
                format!("“{quoted}”")
            } else {
                format!("“{quoted}”, then {caps}")
            }
        }
        None => caps,
    }
}

/// Why Focus pane cannot work for this run from this device, or none when it can: a herdr pane,
/// on a connection corrald reports as local (its socket or loopback), so the pane comes forward
/// where the owner is.
pub fn focus_refusal(feed: &daemon::Feed, row: Option<&AgentRow>) -> Option<String> {
    let Some(row) = row else {
        return Some("corrald lists no live run with this id now".into());
    };
    match row.run.session.as_ref().map(placement) {
        None => Some("No session was recorded for this run".into()),
        Some(SessionRef::Process { .. } | SessionRef::Pi { .. }) => {
            Some("A headless session has no pane to focus".into())
        }
        Some(SessionRef::Herdr { .. }) => {
            match feed.session.as_ref().and_then(|session| session.local) {
                Some(true) => None,
                Some(false) => Some(
                    "The pane is on corrald's machine; this device reaches it from elsewhere"
                        .into(),
                ),
                None => Some("corrald did not say whether this connection is local".into()),
            }
        }
    }
}

/// How the latest pane request of `session` ended, in its tone; `pane` names the pane.
fn status(state: &State, session: &str, pane: &str, feed: &daemon::Feed) -> Option<(Hue, String)> {
    let sent = state.pane.get(session)?;
    let what = match &sent.ask {
        PaneAsk::Focus => "Focus pane".to_owned(),
        PaneAsk::Keys { shown, .. } => shown.clone(),
    };
    if let Some(why) = &sent.unsent {
        return Some((Hue::Fail, format!("{what} · not sent: {why}")));
    }
    let operation = feed.operations.get(sent.key.as_str());
    Some(match operation {
        Some(operation) if !operation.loading => match (&operation.refusal, &operation.error) {
            (Some(refusal), _) => (Hue::Fail, format!("{what} · refused: {}", refusal.reason)),
            (None, Some(error)) => (Hue::Fail, format!("{what} · failed: {}", error.message)),
            (None, None) => match &sent.ask {
                PaneAsk::Focus => (Hue::Ok, format!("Focused {pane} on corrald's machine")),
                PaneAsk::Keys { .. } => (Hue::Ok, format!("{what} · sent to {pane}")),
            },
        },
        _ => (Hue::Neutral, format!("{what} · sending…")),
    })
}

/// The outcome of the latest pane request of `session`, for a surface that names no pane.
pub(super) fn outcome(state: &State, row: &AgentRow, feed: &daemon::Feed) -> Option<(Hue, String)> {
    let id = format!("{}_{}", row.run.task, row.run.id);
    let pane = session_name(row).unwrap_or_else(|| "the pane".into());
    status(state, &id, &pane, feed)
}

/// The keys bar and type field of run session `id`: open for a herdr pane, closed with why for
/// a headless session, none without a recorded session.
pub(super) fn input(
    state: &State,
    id: &Id,
    row: &AgentRow,
    feed: &daemon::Feed,
) -> Option<PaneInput> {
    let pane = session_name(row)?;
    let closed = match placement(row.run.session.as_ref()?) {
        SessionRef::Herdr { .. } => None,
        SessionRef::Process { .. } | SessionRef::Pi { .. } => Some(
            "Headless session: its log takes no keys. Reply, Stop and Retry are in the Session menu."
                .to_owned(),
        ),
    };
    let sent = state.pane.get(id.as_str());
    let delivered = sent
        .filter(|sent| matches!(sent.ask, PaneAsk::Keys { text: true, .. }))
        .filter(|sent| sent.unsent.is_none())
        .filter(|sent| {
            feed.operations
                .get(sent.key.as_str())
                .is_some_and(|operation| {
                    !operation.loading
                        && operation.error.is_none()
                        && operation.refusal.is_none()
                        && operation.value.is_some()
                })
        })
        .map(|sent| sent.key.to_string());
    Some(PaneInput {
        session: id.clone(),
        keys: KEYS
            .iter()
            .map(|(name, cap)| PaneKey {
                cap,
                test_id: format!("btn-key-{}", name.replace('+', "-")),
                action: Action::Pane {
                    session: id.clone(),
                    text: String::new(),
                    keys: vec![(*name).to_owned()],
                },
            })
            .collect(),
        placeholder: format!("Type into {pane}…"),
        accessibility_label: "Text to type into the pane",
        type_label: "Type",
        enter_label: "Type + Enter",
        hint: format!(
            "Typed into {pane} as is; Enter is pressed only with Type + Enter or its key."
        ),
        closed,
        status: status(state, id.as_str(), &pane, feed),
        delivered,
    })
}

#[cfg(test)]
mod tests {
    use corral_protocol::pane::PaneKeys;
    use corral_types::Role;

    use super::super::roster_tests::agent;
    use super::super::{Action, State, live_session};
    use crate::model::daemon::{self, ClientError, Operation};
    use crate::model::effect::{Effect, PaneRequest};
    use crate::model::health::daemon_env;
    use crate::model::hue::Hue;
    use crate::model::{Form, Id};

    /// Typed text goes out as one `pane.keys` under its own request id, the text before Enter;
    /// when corrald answers that request with an error, the session page says so in the
    /// failure tone rather than the error being dropped.
    #[test]
    fn typed_text_carries_a_request_id_and_its_failure_shows_on_the_session() {
        let id = Id::from("T-1_R1");
        let mut feed = daemon::Feed::default();
        feed.agents
            .insert(id.clone(), agent("T-1", "R1", Role::Worker));
        let mut state = State::default();
        let effects = state.act_live(
            Action::Pane {
                session: id.clone(),
                text: "y".into(),
                keys: vec!["enter".into()],
            },
            &feed,
            &Default::default(),
            daemon_env(&feed, Form::Terminal),
        );
        let [
            Effect::Pane {
                session,
                key,
                what,
                request: PaneRequest::Keys(PaneKeys { target, keys, text }),
            },
        ] = effects.as_slice()
        else {
            panic!("one pane request, got {effects:?}");
        };
        assert_eq!((session, target.run.as_str()), (&id, "R1"));
        assert_eq!(
            (text.as_deref(), keys.as_slice()),
            (Some("y"), ["enter".to_owned()].as_slice())
        );
        state.pane_sent(id.clone(), key.clone(), what.clone(), None);
        feed.operations.insert(
            key.clone(),
            Operation {
                method: corral_protocol::pane::PANE_KEYS.into(),
                loading: false,
                value: None,
                error: Some(ClientError {
                    code: "internal".into(),
                    message: "send text to herdr pane T-1-R1".into(),
                }),
                refusal: None,
            },
        );
        let page = live_session(
            &state,
            &id,
            &feed,
            daemon_env(&feed, Form::Terminal),
            "app:test",
        );
        let status = page.pane.and_then(|pane| pane.status);
        assert!(
            matches!(&status, Some((Hue::Fail, words)) if words.contains("send text to herdr pane")),
            "{status:?}"
        );
    }
}
