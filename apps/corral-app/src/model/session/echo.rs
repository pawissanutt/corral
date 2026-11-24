//! This device's own sends to a session, drawn until the transcript holds them (plan §2.3, point
//! 5). A send is known only by its command id, which travels unchanged as the host's input id:
//! its echo is drawn after the last served message while no served message has the id
//! `<session>/input/<command id>`, and is retired the moment one has, or once the acknowledgement
//! says the host handled it without a message (an answer, a child steer, a slash command). A
//! late acknowledgement under an ended thread also takes its echo out of any other scope.
//! Text and time never match a send to a message.

use corral_protocol::assistant::SendAttachment;
use corral_protocol::detail::SessionPage;
use corral_protocol::session_host::{InputDisposition, SessionSent};
use corral_protocol::session_state::SendPolicy;
use corral_protocol::subscription::SessionLiveInputState;

use crate::model::Id;
use crate::model::daemon::Feed;
use crate::model::hue::Hue;

use super::Ui;

/// A send this device made to one session, kept under its command id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sent {
    /// The command id, which is also the input id.
    pub key: Id,
    pub text: String,
    pub policy: SendPolicy,
    /// The child it steers; none for the session's own agent.
    pub to: Option<String>,
    /// RFC 3339 time the owner pressed Send, when the clock had ticked.
    pub typed_at: Option<String>,
    pub attachments: Vec<SendAttachment>,
    /// Why it never left this device.
    pub unsent: Option<String>,
    /// Its refusal was told once: the text went back to the draft and a notice said why.
    pub told: bool,
}

/// Where an echo stands, as the acknowledgement and the host's input events say.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EchoState {
    /// Sent; no acknowledgement yet.
    Sending,
    /// The host has it but has not admitted it yet.
    Pending,
    /// Waiting in the host's queue; `position` when the host said it.
    Queued { position: Option<u32> },
    /// The host started a turn with it or steered the running one; the transcript holds it once
    /// read again.
    Started,
    /// Taken back out of the queue; it was not sent to the agent.
    Withdrawn,
    /// Refused by corrald or the host, or never left this device.
    Refused(String),
}

impl EchoState {
    /// Whether nothing more happens to it: it will never be served.
    pub fn settled(&self) -> bool {
        matches!(self, Self::Withdrawn | Self::Refused(_))
    }
}

/// One echo as the transcript draws it: an owner bubble with its state under it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Echo {
    /// `echo-<command id>`.
    pub id: String,
    /// The command id, shown muted where served owner messages show their input id.
    pub input: String,
    pub text: String,
    /// `→ a2` for a child steer.
    pub to: Option<String>,
    pub policy: SendPolicy,
    pub state: EchoState,
    /// `sending…`, `queued · 2nd in line`, `refused: <reason>`; empty once started.
    pub words: String,
    pub hue: Hue,
}

/// The id the transcript serves the message of input `key` under, in concrete session `session`.
pub fn served_id(session: &str, key: &str) -> String {
    format!("{session}/input/{key}")
}

/// The acknowledgement of send `key`, once corrald answered it.
pub fn ack(feed: &Feed, key: &str) -> Option<SessionSent> {
    feed.operations.get(key)?.decode::<SessionSent>()
}

/// Whether this scope no longer needs the echo. An acknowledgement under an ended thread
/// must not follow Ask back to `quick` or into a fresh thread.
pub fn retired(sent: &Sent, scope: &str, page: Option<&SessionPage>, feed: &Feed) -> bool {
    let ack = ack(feed, sent.key.as_str());
    if ack.as_ref().is_some_and(|ack| {
        ack.disposition == InputDisposition::Handled
            || (ack.session != scope && crate::model::assistant::ended(feed, &ack.session))
    }) {
        return true;
    }
    let Some(page) = page else {
        return false;
    };
    // The acknowledgement names the concrete session (`A-7` for a send to `quick`); a page
    // read before it arrived names it too.
    let mut ids = vec![served_id(&page.id, sent.key.as_str())];
    if let Some(ack) = &ack {
        ids.push(ack.entry.clone());
        ids.push(served_id(&ack.session, sent.key.as_str()));
    }
    page.messages
        .iter()
        .any(|message| !message.id.is_empty() && ids.contains(&message.id))
}

/// Where send `sent` to `scope` stands.
pub fn state(sent: &Sent, scope: &str, feed: &Feed) -> EchoState {
    if let Some(why) = &sent.unsent {
        return EchoState::Refused(why.clone());
    }
    let operation = feed
        .operations
        .get(sent.key.as_str())
        .filter(|operation| !operation.loading);
    if let Some(operation) = operation {
        if let Some(refusal) = &operation.refusal {
            return EchoState::Refused(refusal.reason.clone());
        }
        if let Some(error) = &operation.error {
            return EchoState::Refused(error.message.clone());
        }
    }
    let announced = feed
        .live
        .get(scope)
        .and_then(|live| live.input(sent.key.as_str()));
    match announced {
        Some(SessionLiveInputState::Accepted) => return EchoState::Pending,
        Some(SessionLiveInputState::Queued { position, .. }) => {
            return EchoState::Queued {
                position: Some(*position),
            };
        }
        Some(SessionLiveInputState::Started { .. }) => return EchoState::Started,
        Some(SessionLiveInputState::Withdrawn) => return EchoState::Withdrawn,
        Some(SessionLiveInputState::Rejected { reason }) => {
            return EchoState::Refused(reason.clone());
        }
        Some(SessionLiveInputState::Other) | None => {}
    }
    match operation.and_then(|operation| operation.decode::<SessionSent>()) {
        None => EchoState::Sending,
        Some(ack) => match ack.disposition {
            InputDisposition::Started => EchoState::Started,
            InputDisposition::Queued => EchoState::Queued { position: None },
            InputDisposition::Pending | InputDisposition::Handled | InputDisposition::Other => {
                EchoState::Pending
            }
        },
    }
}

/// The words and tone under an echo.
fn words(state: &EchoState) -> (String, Hue) {
    match state {
        EchoState::Sending => ("sending…".into(), Hue::Neutral),
        EchoState::Pending => ("pending admission".into(), Hue::Neutral),
        EchoState::Queued { position: Some(at) } => {
            (format!("queued · {} in line", at + 1), Hue::Neutral)
        }
        EchoState::Queued { position: None } => ("queued".into(), Hue::Neutral),
        EchoState::Started => (String::new(), Hue::Neutral),
        EchoState::Withdrawn => ("withdrawn · not sent".into(), Hue::Neutral),
        EchoState::Refused(why) => (format!("refused: {why}"), Hue::Fail),
    }
}

/// The echoes of session `scope` the transcript draws after its served messages, in send order.
pub fn shown(scope: &Id, ui: &Ui, page: Option<&SessionPage>, feed: &Feed) -> Vec<Echo> {
    ui.echoes
        .iter()
        .filter(|sent| !retired(sent, scope.as_str(), page, feed))
        .map(|sent| {
            let state = state(sent, scope.as_str(), feed);
            let (words, hue) = words(&state);
            Echo {
                id: format!("echo-{}", sent.key),
                input: sent.key.to_string(),
                text: sent.text.clone(),
                to: sent.to.clone(),
                policy: sent.policy,
                state,
                words,
                hue,
            }
        })
        .collect()
}
