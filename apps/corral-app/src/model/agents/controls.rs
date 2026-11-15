//! The session page's action bar and the wall's Launch PM: Cancel, Pause or Resume, Relaunch
//! (Launch PM), Take over and Terminate, built only from the controls corrald reports on the
//! session's row, with the words of each refused one and the outcome of the latest one sent.
//! The reducers send the canonical `session` command; an assistant's controls go to the
//! `assistant.stop` / `assistant.session` calls its host takes, since corrald refuses the
//! `session` family for assistants.

use corral_protocol::assistant::{AssistantSessionOp, AssistantStop};
use corral_protocol::cmd::Command;
use corral_protocol::cmd::session::SessionCommand;
use corral_protocol::rows::{AgentRow, AgentStatus as RowStatus, ControlState, SessionControls};

use crate::model::act::{self, Act, ActStatus, Authority};
use crate::model::daemon::{self, Link, Operation};
use crate::model::effect::{Effect, Env};
use crate::model::hue::Hue;
use crate::model::{Id, Live};

use super::live::{shown_id, stale};
use super::{Action, State};

/// Refusals that only say the control does not apply now (the session is not paused, nothing
/// is running): the control is left out rather than shown refused.
const NOT_NOW: [&str; 5] = [
    "ended",
    "not_paused",
    "already_paused",
    "no_turn",
    "pause_unsupported",
];

pub(super) const OFFLINE: &str =
    "Needs a live connection to corrald; nothing is sent or queued offline.";
pub(super) const NO_SESSION: &str = "Waiting for the app session; nothing was sent.";

/// One control of a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlKind {
    Cancel,
    Pause,
    Resume,
    /// Terminate: stop the host and end the session.
    Stop,
    Relaunch,
    TakeOver,
}

impl ControlKind {
    const fn label(self) -> &'static str {
        match self {
            Self::Cancel => "Cancel",
            Self::Pause => "Pause",
            Self::Resume => "Resume",
            Self::Stop => "Terminate",
            Self::Relaunch => "Relaunch",
            Self::TakeOver => "Take over",
        }
    }

    const fn test_id(self) -> &'static str {
        match self {
            Self::Cancel => "btn-session-cancel",
            Self::Pause => "btn-session-pause",
            Self::Resume => "btn-session-resume",
            Self::Stop => "btn-session-stop",
            Self::Relaunch => "btn-session-relaunch",
            Self::TakeOver => "btn-session-takeover",
        }
    }

    const fn busy(self) -> &'static str {
        match self {
            Self::Cancel => "Cancelling…",
            Self::Pause => "Pausing…",
            Self::Resume => "Resuming…",
            Self::Stop => "Terminating…",
            Self::Relaunch => "Relaunching…",
            Self::TakeOver => "Taking over…",
        }
    }

    const fn done(self) -> &'static str {
        match self {
            Self::Cancel => "Turn cancelled",
            Self::Pause => "Paused",
            Self::Resume => "Resumed",
            Self::Stop => "Terminated",
            Self::Relaunch => "Relaunched",
            Self::TakeOver => "Taken over",
        }
    }

    fn of(self, controls: &SessionControls) -> &ControlState {
        match self {
            Self::Cancel => &controls.cancel,
            Self::Pause => &controls.pause,
            Self::Resume => &controls.resume,
            Self::Stop => &controls.stop,
            Self::Relaunch => &controls.relaunch,
            Self::TakeOver => &controls.take_over,
        }
    }
}

/// How much a control weighs on the bar: the one main action, a second one, or Terminate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Weight {
    Primary,
    Secondary,
    Danger,
}

/// One button of the bar. A disabled one says why in the bar's notes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Control {
    pub kind: ControlKind,
    pub label: String,
    pub test_id: &'static str,
    pub action: Action,
    pub enabled: bool,
    pub weight: Weight,
    /// The key that presses it where there is a keyboard: Esc for Cancel.
    pub key: Option<&'static str>,
}

/// A confirmation this device asks for itself before an assistant call that has no corrald
/// preview: Take over and an assistant's Terminate (End). Confirm sends
/// `Action::ConfirmControl`; Cancel, Escape and the scrim send `Action::DismissControl`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ask {
    pub test_id: &'static str,
    pub title: String,
    pub sub: String,
    pub body: String,
    pub label: &'static str,
    /// The word a terminal asks to be typed.
    pub word: &'static str,
}

/// The session's action bar: its state, the controls corrald offers, why the shown-but-refused
/// ones are held back, and the latest control command's outcome.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Bar {
    /// The session's state chip.
    pub state: Option<(&'static str, Hue)>,
    /// What the state means for the owner, while it needs saying (a paused session).
    pub lead: Option<String>,
    pub controls: Vec<Control>,
    /// Why refused controls are held back, and why none can be pressed from this device.
    pub notes: Vec<String>,
    pub outcome: Option<(Hue, String)>,
    /// "Check result" for a command whose outcome is unknown.
    pub recover: Option<Action>,
    /// What Esc presses: Cancel, while it can be pressed.
    pub esc: Option<Action>,
    pub ask: Option<Ask>,
}

/// The wall's Launch PM, while no PM is live.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    pub words: String,
    pub label: &'static str,
    pub action: Action,
    pub enabled: bool,
    pub status: Option<(Hue, String)>,
    pub recover: Option<Action>,
}

/// How a control command was sent, which says where its outcome is read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Channel {
    /// A canonical command in the client's command journal.
    Act,
    /// An `assistant.session` operation, read by the caller's key.
    Op,
    /// An `assistant.stop`, which has no reply: done once the row's turn moves on.
    Stop { turn: u64 },
}

/// The latest control command of a session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sent {
    pub kind: ControlKind,
    pub key: Id,
    pub channel: Channel,
}

/// The status of an `assistant.session` operation from its reply, once the client tracks it.
pub fn op_status(op: &Operation) -> ActStatus {
    match &op.error {
        _ if op.loading => ActStatus::Pending,
        Some(error) => ActStatus::Refused(error.message.clone()),
        None => ActStatus::Done,
    }
}

/// A refusal code as the owner reads it; a code this build does not know is shown as sent.
pub fn refusal_words(code: &str) -> String {
    match code {
        "host_lost" => "its host is gone".into(),
        "no_bridge" => "this harness has no corral bridge".into(),
        "ended" => "the session has ended".into(),
        "not_owned" => "corral does not own this session's pane".into(),
        "no_pane" => "this session has no pane".into(),
        "no_supported_session_reply" => {
            "this session cannot receive app messages; no supported live host or pane is available"
                .into()
        }
        "adopted_pane" => {
            "you opened this pane yourself, so corral never types into or closes it".into()
        }
        "not_paused" => "the session is not paused".into(),
        "already_paused" => "the session is already paused".into(),
        "no_turn" => "nothing is running to cancel".into(),
        "pause_unsupported" => "this role is not paused; a hold already cancels it".into(),
        other => format!("refused ({other})"),
    }
}

/// Why no control can be pressed from this device now: no live link, no app session yet, or
/// the authority the call needs (`assistant` for an assistant's host calls, else `act`).
pub(super) fn device_refusal(feed: &daemon::Feed, env: Env, assistant: bool) -> Option<String> {
    if env.conn.refuses() || feed.link != Link::Up {
        return Some(OFFLINE.into());
    }
    let Some(session) = &feed.session else {
        return Some(NO_SESSION.into());
    };
    let (granted, authority) = if assistant {
        (session.authority.assistant, Authority::Assistant)
    } else {
        (session.authority.act, Authority::Act)
    };
    (!granted).then(|| authority.refusal().into())
}

pub(super) fn in_flight(status: &ActStatus) -> bool {
    matches!(
        status,
        ActStatus::Previewing
            | ActStatus::Confirming
            | ActStatus::Queued
            | ActStatus::Pending
            | ActStatus::Running
    )
}

/// The PM row's Relaunch opens the PM when none is live.
fn opens_pm(row: &AgentRow) -> bool {
    row.id == "pm" && row.status == RowStatus::Ended
}

fn label(kind: ControlKind, row: &AgentRow) -> &'static str {
    if kind == ControlKind::Relaunch && opens_pm(row) {
        "Launch PM"
    } else {
        kind.label()
    }
}

/// Whether a control shows: `None` leaves it out (not offered and nothing worth saying),
/// `Some(None)` offers it, `Some(Some(words))` shows it held back for corrald's reason.
fn offer(kind: ControlKind, row: &AgentRow) -> Option<Option<String>> {
    let assistant = row.role == "assistant";
    let applies = match kind {
        // An assistant's turn is stopped by its number.
        ControlKind::Cancel => !assistant || row.turn.is_some(),
        // An assistant's next generation starts on its next message; it has no relaunch call.
        ControlKind::Relaunch => !assistant,
        ControlKind::TakeOver => assistant,
        _ => true,
    };
    if !applies {
        return None;
    }
    let state = kind.of(&row.controls);
    if state.available {
        return Some(None);
    }
    let code = state.refusal.as_deref()?;
    let worth = match kind {
        // Relaunch is for a lost host; refused on a running one it says nothing new.
        ControlKind::Relaunch => row.status == RowStatus::Unavailable,
        // Only a pane-hosted assistant can be taken over.
        ControlKind::TakeOver => row.pane.is_some(),
        _ => true,
    };
    (worth && !NOT_NOW.contains(&code)).then(|| Some(refusal_words(code)))
}

/// Where the latest control command stands: from the command journal, the operation's reply
/// (`op`, read by the screen) or the row's turn.
fn sent_status(
    sent: &Sent,
    row: &AgentRow,
    feed: &daemon::Feed,
    op: Option<&ActStatus>,
) -> Option<ActStatus> {
    match sent.channel {
        Channel::Act => act::status(feed, &sent.key),
        Channel::Op => op.cloned(),
        Channel::Stop { turn } => Some(
            if row.turn == Some(turn) && row.status == RowStatus::Working {
                ActStatus::Pending
            } else {
                ActStatus::Done
            },
        ),
    }
}

/// A command's outcome in its tone, under the control's name.
fn outcome_line(label: &str, kind: ControlKind, status: &ActStatus) -> (Hue, String) {
    match status {
        ActStatus::Done => (Hue::Ok, kind.done().into()),
        ActStatus::Failed(_) | ActStatus::Refused(_) | ActStatus::Conflict(_) => {
            (Hue::Fail, format!("{label} · {}", status.line()))
        }
        ActStatus::Unknown | ActStatus::Abandoned => {
            (Hue::Held, format!("{label} · {}", status.line()))
        }
        _ => (Hue::Neutral, format!("{} {}", kind.busy(), status.line())),
    }
}

/// The action bar of a daemon session from its row. `op` is the reply to the session's latest
/// `assistant.session` operation, when one was sent.
pub(super) fn live_bar(
    state: &State,
    id: &Id,
    row: Option<&AgentRow>,
    feed: &daemon::Feed,
    env: Env,
    op: Option<&ActStatus>,
) -> Bar {
    let Some(row) = row else {
        return Bar::default();
    };
    let device = device_refusal(feed, env, row.role == "assistant");
    let paused = row.status == RowStatus::Paused;
    let sent = state.sent.get(id.as_str());
    let status = sent.and_then(|sent| sent_status(sent, row, feed, op));
    let flying = sent
        .filter(|_| status.as_ref().is_some_and(in_flight))
        .map(|sent| sent.kind);
    use ControlKind::{Cancel, Pause, Relaunch, Resume, Stop, TakeOver};
    let order = if paused {
        [Resume, Cancel, Pause, Relaunch, TakeOver, Stop]
    } else {
        [Cancel, Pause, Resume, Relaunch, TakeOver, Stop]
    };
    let primary = if paused {
        Some(Resume)
    } else if row.status == RowStatus::Unavailable || opens_pm(row) {
        Some(Relaunch)
    } else {
        None
    };
    let mut controls = Vec::new();
    let mut refused: Vec<(String, Vec<&'static str>)> = Vec::new();
    for kind in order {
        let Some(why) = offer(kind, row) else {
            continue;
        };
        let name = label(kind, row);
        let busy = flying == Some(kind);
        let enabled = why.is_none() && device.is_none() && !busy;
        if let Some(words) = why {
            match refused.iter_mut().find(|(said, _)| *said == words) {
                Some((_, names)) => names.push(name),
                None => refused.push((words, vec![name])),
            }
        }
        controls.push(Control {
            kind,
            label: if busy {
                kind.busy().into()
            } else {
                name.into()
            },
            test_id: kind.test_id(),
            action: Action::Control(kind),
            enabled,
            weight: if kind == Stop {
                Weight::Danger
            } else if primary == Some(kind) {
                Weight::Primary
            } else {
                Weight::Secondary
            },
            key: (kind == Cancel).then_some("Esc"),
        });
    }
    let mut notes: Vec<String> = refused
        .into_iter()
        .map(|(words, names)| format!("{} unavailable: {words}.", names.join(", ")))
        .collect();
    if !controls.is_empty() {
        notes.extend(device);
    }
    let (outcome, recover) = match (sent, &status) {
        (Some(sent), Some(status)) => (
            Some(outcome_line(label(sent.kind, row), sent.kind, status)),
            (*status == ActStatus::Unknown && sent.channel == Channel::Act)
                .then(|| Action::Recover(sent.key.clone())),
        ),
        _ => (None, None),
    };
    Bar {
        state: Some(super::live::status(row.status).chip()),
        lead: paused.then(|| {
            "Paused · automated input waits until you resume; a message you send resumes it.".into()
        }),
        esc: controls
            .iter()
            .find(|control| control.kind == Cancel && control.enabled)
            .map(|control| control.action.clone()),
        controls,
        notes,
        outcome,
        recover,
        ask: ask(state, id, row),
    }
}

/// The local confirmation open for this session, if any.
fn ask(state: &State, id: &Id, row: &AgentRow) -> Option<Ask> {
    let (asked, kind) = state.asking.as_ref()?;
    if asked != id {
        return None;
    }
    Some(match kind {
        ControlKind::TakeOver => Ask {
            test_id: "session-takeover",
            title: format!("Take over {}", row.id),
            sub: "Moves this assistant out of its pane and into corrald.".into(),
            body: format!(
                "corrald asks the host in pane {} to stop. The session then continues in the \
                 daemon with the same thread and transcript; the pane is never typed into.",
                row.pane.as_deref().unwrap_or("of this session")
            ),
            label: "Take over",
            word: "take",
        },
        _ => Ask {
            test_id: "session-terminate",
            title: format!("Terminate {}", row.id),
            sub: "Ends this session for good; it cannot be resumed.".into(),
            body: "corrald asks its host to stop, cancels what it was waiting to do and closes \
                   the session. Its transcript, summary and scratch records stay readable."
                .into(),
            label: "Terminate",
            word: "terminate",
        },
    })
}

/// Launch PM on the wall: offered while corrald lists the PM row ended with `relaunch` offered,
/// or lists no PM row at all, over a current agents list.
pub(super) fn launch(state: &State, feed: &daemon::Feed, env: Env) -> Option<Launch> {
    let listed = feed
        .topics
        .get("agents")
        .is_some_and(|topic| !topic.loading && topic.error.is_none());
    if !listed || stale(feed, "agents") {
        return None;
    }
    if !feed
        .agents
        .get("pm")
        .is_none_or(|row| opens_pm(row) && row.controls.relaunch.available)
    {
        return None;
    }
    let device = device_refusal(feed, env, false);
    let status = state.launch.as_ref().and_then(|key| act::status(feed, key));
    let flying = status.as_ref().is_some_and(in_flight);
    let mut words = "The PM is not running. Launch PM opens it in a herdr pane.".to_owned();
    if let Some(why) = &device {
        words.push(' ');
        words.push_str(why);
    }
    Some(Launch {
        words,
        label: if flying { "Launching…" } else { "Launch PM" },
        action: Action::LaunchPm,
        enabled: device.is_none() && !flying,
        recover: matches!(status, Some(ActStatus::Unknown))
            .then(|| state.launch.clone().map(Action::Recover))
            .flatten(),
        status: status
            .as_ref()
            .map(|status| outcome_line("Launch PM", ControlKind::Relaunch, status)),
    })
}

impl State {
    /// A control pressed on a daemon session's bar. Only a control corrald offers on the row is
    /// sent; Terminate is Confirm-class, so the root previews it and asks this device first.
    pub(super) fn press(
        &mut self,
        session: Id,
        kind: ControlKind,
        feed: &daemon::Feed,
    ) -> Vec<Effect> {
        let Some(row) = feed.agents.get(session.as_str()) else {
            return Vec::new();
        };
        if offer(kind, row) != Some(None) {
            return Vec::new();
        }
        if let Some(effects) = self.held(&session, kind, row, feed) {
            return effects;
        }
        let id = row.id.clone();
        if row.role == "assistant" {
            return match kind {
                ControlKind::Cancel => {
                    let Some(turn) = row.turn else {
                        return Vec::new();
                    };
                    self.sent.insert(
                        session,
                        Sent {
                            kind,
                            key: self.ids.next_id(),
                            channel: Channel::Stop { turn },
                        },
                    );
                    vec![Effect::Stop(AssistantStop { session: id, turn })]
                }
                ControlKind::Pause => {
                    self.op(session, kind, AssistantSessionOp::Pause { session: id })
                }
                ControlKind::Resume => {
                    self.op(session, kind, AssistantSessionOp::Resume { session: id })
                }
                ControlKind::Stop | ControlKind::TakeOver => {
                    self.asking = Some((session, kind));
                    Vec::new()
                }
                ControlKind::Relaunch => Vec::new(),
            };
        }
        let shown = shown_id(row);
        let generation = row.generation;
        let (title, command) = match kind {
            ControlKind::Cancel => (
                format!("Cancel {shown}'s turn"),
                SessionCommand::Cancel {
                    id,
                    generation,
                    turn: row.turn,
                    json: false,
                },
            ),
            ControlKind::Pause => (
                format!("Pause {shown}"),
                SessionCommand::Pause {
                    id,
                    generation,
                    json: false,
                },
            ),
            ControlKind::Resume => (
                format!("Resume {shown}"),
                SessionCommand::Resume { id, json: false },
            ),
            ControlKind::Stop => (
                format!("Terminate {shown}"),
                SessionCommand::Stop {
                    id,
                    generation,
                    json: false,
                },
            ),
            ControlKind::Relaunch => (
                if opens_pm(row) {
                    "Launch PM".to_owned()
                } else {
                    format!("Relaunch {shown}")
                },
                SessionCommand::Relaunch {
                    id,
                    replace: false,
                    json: false,
                },
            ),
            ControlKind::TakeOver => return Vec::new(),
        };
        let act = Act::new(&mut self.ids, title, Command::Session(command))
            .about(Live::Session(session.clone()));
        self.sent.insert(
            session,
            Sent {
                kind,
                key: act.key.clone(),
                channel: Channel::Act,
            },
        );
        vec![Effect::Act(act)]
    }

    /// The same control already sent: an unknown outcome is recovered under its identity, one
    /// still in flight is left alone.
    fn held(
        &self,
        session: &Id,
        kind: ControlKind,
        row: &AgentRow,
        feed: &daemon::Feed,
    ) -> Option<Vec<Effect>> {
        let sent = self
            .sent
            .get(session.as_str())
            .filter(|sent| sent.kind == kind)?;
        match sent_status(sent, row, feed, None)? {
            ActStatus::Unknown if sent.channel == Channel::Act => {
                Some(vec![Effect::Recover(sent.key.clone())])
            }
            status if in_flight(&status) => Some(Vec::new()),
            _ => None,
        }
    }

    fn op(&mut self, session: Id, kind: ControlKind, op: AssistantSessionOp) -> Vec<Effect> {
        let key = self.ids.next_id();
        self.sent.insert(
            session,
            Sent {
                kind,
                key: key.clone(),
                channel: Channel::Op,
            },
        );
        vec![Effect::Session { key, op }]
    }

    /// The owner confirmed the local confirmation: the assistant's End or Take over, if corrald
    /// still offers it.
    pub(super) fn confirm_control(&mut self, feed: &daemon::Feed) -> Vec<Effect> {
        let Some((session, kind)) = self.asking.take() else {
            return Vec::new();
        };
        let Some(row) = feed
            .agents
            .get(session.as_str())
            .filter(|row| row.role == "assistant")
        else {
            return Vec::new();
        };
        if offer(kind, row) != Some(None) {
            return vec![Effect::Toast(format!(
                "{} is no longer offered for {}; nothing was sent.",
                kind.label(),
                row.id
            ))];
        }
        let id = row.id.clone();
        match kind {
            ControlKind::Stop => self.op(session, kind, AssistantSessionOp::End { session: id }),
            ControlKind::TakeOver => {
                self.op(session, kind, AssistantSessionOp::TakeOver { session: id })
            }
            _ => Vec::new(),
        }
    }

    /// Launch PM from the wall: the PM row's relaunch, or opening a PM corrald does not list.
    pub(super) fn launch_pm(&mut self, feed: &daemon::Feed) -> Vec<Effect> {
        if let Some(key) = &self.launch {
            match act::status(feed, key) {
                Some(ActStatus::Unknown) => return vec![Effect::Recover(key.clone())],
                Some(status) if in_flight(&status) => return Vec::new(),
                _ => {}
            }
        }
        let command = match feed.agents.get("pm") {
            Some(row) if opens_pm(row) && row.controls.relaunch.available => {
                SessionCommand::Relaunch {
                    id: row.id.clone(),
                    replace: false,
                    json: false,
                }
            }
            Some(_) => return Vec::new(),
            None => SessionCommand::PmOpen { json: false },
        };
        let act = Act::new(&mut self.ids, "Launch PM", Command::Session(command))
            .about(Live::Session(Id::from("pm")));
        self.launch = Some(act.key.clone());
        vec![Effect::Act(act)]
    }
}

#[cfg(test)]
#[path = "controls_tests.rs"]
mod tests;
