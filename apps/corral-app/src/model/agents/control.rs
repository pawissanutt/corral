//! Session controls addressed by a session's public id (`T-1_R1`, a chat's `A-1` or `quick`),
//! never by its pane: Cancel ends the current turn, Pause also holds automated input until
//! Resume (a message the owner types resumes it too), Terminate ends the host and keeps the
//! transcript, Relaunch starts a lost host again, Rewind restores a worker's tree to the start
//! of an earlier turn after saving the current work under a rescue ref. Which controls a menu
//! offers follows the role and what the rows report; whether the host supports one is
//! corrald's to say, and its refusal is shown with its reason.

use corral_protocol::rows::{AgentRow, AssistantKind, AssistantLocation, AssistantState};
use corral_protocol::session::{
    RewindReply, Rewound, SESSION_CANCEL, SESSION_PAUSE, SESSION_RELAUNCH, SESSION_RESUME,
    SESSION_REWIND, SESSION_TERMINATE, SessionCancelReply, SessionPauseReply, SessionRelaunched,
};
use corral_types::{Refusal, Role, SessionStatus};
use serde_json::{Value, json};

use crate::model::Id;
use crate::model::act::RefusalSheet;
use crate::model::daemon::{Feed, Operation};
use crate::model::effect::Effect;
use crate::model::hue::Hue;
use crate::model::work::{Press, VerbAction};

use super::State;

/// One owner control of a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Control {
    /// End the current turn, as Esc in the terminal UI; the host stays and idles.
    Cancel,
    /// Cancel the turn and hold automated input until Resume or an owner message.
    Pause,
    /// Lift the hold; held input is delivered in order.
    Resume,
    /// Stop the host and end the session; the transcript stays, and so do a worker's tree and
    /// branch.
    Terminate,
    /// Start a lost host again on the same session.
    Relaunch,
    /// Resume a cold trusted Code host in its checkout.
    ResumeCode,
    /// Ask for the turns a worker can be rewound to and the rewinds already done.
    Rewinds,
    /// Rewind the worker to the start of this turn.
    Rewind(u64),
}

/// Which controls a session's role takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Worker,
    Reviewer,
    Chat,
    Build,
    Code,
}

impl Control {
    /// The words of its menu entry.
    pub fn label(self) -> String {
        match self {
            Self::Cancel => "Cancel turn".into(),
            Self::Pause => "Pause".into(),
            Self::Resume | Self::ResumeCode => "Resume".into(),
            Self::Terminate => "Terminate".into(),
            Self::Relaunch => "Relaunch".into(),
            Self::Rewinds => "Rewind…".into(),
            Self::Rewind(turn) => format!("Rewind to turn {turn}"),
        }
    }

    /// Its control's test id is `btn-<slug>`.
    pub const fn slug(self) -> &'static str {
        match self {
            Self::Cancel => "cancel-turn",
            Self::Pause => "pause",
            Self::Resume | Self::ResumeCode => "resume",
            Self::Terminate => "terminate",
            Self::Relaunch => "relaunch",
            Self::Rewinds => "rewind",
            Self::Rewind(_) => "rewind-turn",
        }
    }

    pub const fn method(self) -> &'static str {
        match self {
            Self::Cancel => SESSION_CANCEL,
            Self::Pause => SESSION_PAUSE,
            Self::Resume => SESSION_RESUME,
            Self::ResumeCode => corral_protocol::build::CODE_RESUME,
            Self::Terminate => SESSION_TERMINATE,
            Self::Relaunch => SESSION_RELAUNCH,
            Self::Rewinds | Self::Rewind(_) => SESSION_REWIND,
        }
    }

    /// The request's parameters for `session`.
    pub fn params(self, session: &str) -> Value {
        match self {
            Self::Rewinds => json!({"id": session, "action": "list"}),
            Self::Rewind(turn) => json!({"id": session, "action": "turn", "turn": turn}),
            _ => json!({"id": session}),
        }
    }

    /// Terminate and a rewind change what cannot be changed back by pressing again, so they
    /// ask once on this device before they are sent.
    pub const fn asks(self) -> bool {
        matches!(self, Self::Terminate | Self::Rewind(_))
    }

    /// What it does, as the confirm sheet says it.
    fn effect(self, kind: Option<Kind>) -> &'static str {
        match (self, kind) {
            (Self::Terminate, Some(Kind::Worker)) => {
                "Stops the host and ends the run as stopped by you. The worktree, the branch and the transcript stay; the task waits in Needs you."
            }
            (Self::Terminate, Some(Kind::Reviewer)) => {
                "Stops the reviewer host. This round ends with no verdict; the transcript stays."
            }
            (Self::Terminate, Some(Kind::Build)) => {
                "Stops the build host. A session with nothing to keep is cleaned up by its host; otherwise the worktree, the branch and the transcript stay and Resume continues it."
            }
            (Self::Terminate, _) => {
                "Stops the host. The transcript stays readable and Relaunch starts it again."
            }
            (Self::Rewind(_), _) => {
                "Cancels the current turn, saves the current work under a rescue ref, then restores the worktree and the conversation to the start of that turn."
            }
            _ => "",
        }
    }
}

/// The session's role: a chat or build id, else its run's role; none for a run corrald does not
/// list.
pub fn kind(feed: &Feed, session: &str) -> Option<Kind> {
    if feed.assistants.get(session).is_some_and(super::is_code) {
        return Some(Kind::Code);
    }
    let built = feed
        .assistants
        .get(session)
        .is_some_and(|row| row.kind == AssistantKind::Build);
    if built || session.starts_with("B-") {
        return Some(Kind::Build);
    }
    if session == crate::model::assistant::QUICK || session.starts_with("A-") {
        return Some(Kind::Chat);
    }
    if session.ends_with("/reviewer") {
        return Some(Kind::Reviewer);
    }
    feed.agents.get(session).map(|row| match row.run.role {
        Role::Worker => Kind::Worker,
        Role::Reviewer => Kind::Reviewer,
    })
}

/// A control's words for a session of `kind`: a build is resumed and stopped, a run or a chat
/// relaunched and terminated.
pub fn control_label(control: Control, kind: Option<Kind>) -> String {
    match (control, kind) {
        (Control::Relaunch, Some(Kind::Build)) => "Resume".into(),
        (Control::Terminate, Some(Kind::Build)) => "Stop session".into(),
        _ => control.label(),
    }
}

/// The controls that apply to the session now, in menu order. Pause is not a reviewer's:
/// a reviewer is cancelled by its task's hold already. Rewind is a worker's. Relaunch is for a
/// host that is lost, Cancel for one working or waiting. A worker shows Pause or Resume as its
/// row's `paused` says; a daemon that does not report it gets both. A chat's row says, so it
/// offers one. A run (`<task>_<run>`) corrald no longer lists has ended, terminated or stopped:
/// Relaunch is what applies to it, a new run of the same role and profile.
pub fn controls(feed: &Feed, session: &str) -> Vec<Control> {
    match kind(feed, session) {
        Some(Kind::Chat | Kind::Build | Kind::Code) => chat_controls(feed, session),
        Some(kind) => feed
            .agents
            .get(session)
            .map(|row| run_controls(row, kind))
            .unwrap_or_default(),
        None if session.contains('_') => vec![Control::Relaunch],
        None => Vec::new(),
    }
}

fn run_controls(row: &AgentRow, kind: Kind) -> Vec<Control> {
    let lost = matches!(row.status, SessionStatus::Gone);
    let busy = matches!(
        row.status,
        SessionStatus::Working | SessionStatus::Blocked { .. }
    );
    let mut controls = Vec::new();
    if busy {
        controls.push(Control::Cancel);
    }
    if kind == Kind::Worker && !lost {
        match row.paused {
            Some(true) => controls.push(Control::Resume),
            Some(false) => controls.push(Control::Pause),
            None => controls.extend([Control::Pause, Control::Resume]),
        }
        controls.push(Control::Rewinds);
    }
    controls.push(if lost {
        Control::Relaunch
    } else {
        Control::Terminate
    });
    controls
}

/// A chat's or a build's controls follow its row's state. A build whose host runs in a terminal
/// corrald cannot reach is continued there, not resumed here.
fn chat_controls(feed: &Feed, session: &str) -> Vec<Control> {
    let Some(row) = crate::model::assistant::row(feed, session) else {
        return Vec::new();
    };
    if super::is_code(row) {
        if row.live_in == AssistantLocation::None && row.state == AssistantState::Cold {
            return vec![Control::ResumeCode];
        }
        return if row.live_in != AssistantLocation::None
            && matches!(
                row.state,
                AssistantState::Working | AssistantState::Waiting | AssistantState::Starting
            ) {
            vec![Control::Cancel]
        } else {
            Vec::new()
        };
    }
    if row.state == AssistantState::Cold && row.live_in == AssistantLocation::Pane {
        return Vec::new();
    }
    match row.state {
        AssistantState::Ended => Vec::new(),
        AssistantState::Cold | AssistantState::Failed => vec![Control::Relaunch],
        AssistantState::Paused => vec![Control::Resume, Control::Terminate],
        AssistantState::Working | AssistantState::Waiting | AssistantState::Starting => {
            vec![Control::Cancel, Control::Pause, Control::Terminate]
        }
        _ => vec![Control::Pause, Control::Terminate],
    }
}

/// The controls as Session menu entries.
pub fn control_verbs(feed: &Feed, session: &Id) -> Vec<VerbAction> {
    let kind = kind(feed, session);
    controls(feed, session)
        .into_iter()
        .map(|control| VerbAction {
            slug: control.slug(),
            label: control_label(control, kind),
            enabled: true,
            reason: None,
            press: Press::Control {
                session: session.clone(),
                control,
            },
        })
        .collect()
}

/// A control waiting for its one local confirmation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlAsk {
    pub session: Id,
    pub key: Id,
    pub control: Control,
    pub title: String,
    pub rows: Vec<(String, String)>,
}

/// The confirm sheet's words for `control` of `session`.
pub fn ask(feed: &Feed, session: Id, key: Id, control: Control) -> ControlAsk {
    let title = format!("{} {session}", control_label(control, kind(feed, &session)));
    let mut rows = vec![(
        "runs".to_owned(),
        format!("{} {}", control.method(), control.params(&session)),
    )];
    let effect = control.effect(kind(feed, &session));
    if !effect.is_empty() {
        rows.push(("effect".to_owned(), effect.to_owned()));
    }
    ControlAsk {
        session,
        key,
        control,
        title,
        rows,
    }
}

/// The latest control sent for one session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlSent {
    pub key: Id,
    pub control: Control,
    /// Why it never left this device, when it did not.
    pub unsent: Option<String>,
    /// Whether its outcome was already told.
    pub told: bool,
}

/// The rewind sheet: the session and its list request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rewinding {
    pub session: Id,
    pub key: Id,
}

/// How a sent control ended.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Pending,
    Done(String),
    Refused(Refusal),
    Failed(String),
}

/// The outcome of `control` from its reply.
pub fn outcome(control: Control, operation: Option<&Operation>) -> Outcome {
    let Some(operation) = operation.filter(|operation| !operation.loading) else {
        return Outcome::Pending;
    };
    if let Some(refusal) = &operation.refusal {
        return Outcome::Refused(refusal.clone());
    }
    if let Some(error) = &operation.error {
        return Outcome::Failed(error.message.clone());
    }
    if operation.value.is_none() {
        return Outcome::Pending;
    }
    Outcome::Done(match control {
        Control::Cancel => match operation.decode::<SessionCancelReply>() {
            Some(reply) if reply.cancelled => match reply.turn {
                Some(turn) => format!("Turn {turn} cancelled; the host is idle"),
                None => "Turn cancelled; the host is idle".into(),
            },
            Some(reply) => reply.reason.map_or_else(
                || "No turn was running".into(),
                |why| format!("Nothing cancelled: {why}"),
            ),
            None => "Cancel sent".into(),
        },
        Control::Pause => match operation.decode::<SessionPauseReply>() {
            Some(reply) if !reply.paused => "Not paused".into(),
            _ => "Paused · automated input waits; Resume or a message from you continues".into(),
        },
        Control::Resume => "Resumed · held input is delivered in order".into(),
        Control::ResumeCode => "Code session resumed".into(),
        Control::Terminate => {
            "Terminated · the transcript stays; Relaunch starts the session again".into()
        }
        Control::Relaunch => match operation.decode::<SessionRelaunched>() {
            Some(reply) => format!("Relaunched as {}", reply.id),
            None => "Relaunched".into(),
        },
        Control::Rewinds => "Rewind points loaded".into(),
        Control::Rewind(turn) => match operation.decode::<RewindReply>() {
            Some(RewindReply::Turn(rewound)) => rewound_line(&rewound),
            _ => format!("Rewound to turn {turn}"),
        },
    })
}

fn rewound_line(rewound: &Rewound) -> String {
    format!(
        "Rewound to turn {} · the work before it is saved as {}",
        rewound.turn, rewound.rescue_ref
    )
}

/// The session's latest control as the strip under its head says it; none once nothing
/// was sent here.
pub fn status(state: &State, session: &str, feed: &Feed) -> Option<(Hue, String)> {
    let sent = state.controls.get(session)?;
    if matches!(sent.control, Control::Rewinds) {
        return None;
    }
    let what = sent.control.label();
    if let Some(why) = &sent.unsent {
        return Some((Hue::Fail, format!("{what} · not sent: {why}")));
    }
    Some(
        match outcome(sent.control, feed.operations.get(sent.key.as_str())) {
            Outcome::Pending => (Hue::Neutral, format!("{what} · sending…")),
            Outcome::Done(line) => (
                if matches!(sent.control, Control::Pause) {
                    Hue::Held
                } else {
                    Hue::Ok
                },
                line,
            ),
            Outcome::Refused(refusal) => (Hue::Fail, format!("{what} refused: {}", refusal.reason)),
            Outcome::Failed(why) => (Hue::Fail, format!("{what} failed: {why}")),
        },
    )
}

impl State {
    /// A control for `session` under `key`; the confirm sheet comes first for one that asks
    /// and was not confirmed yet.
    pub fn control_sent(&mut self, session: Id, key: Id, control: Control, unsent: Option<String>) {
        if control == Control::Rewinds && unsent.is_none() {
            self.rewind = Some(Rewinding {
                session: session.clone(),
                key: key.clone(),
            });
        }
        self.controls.insert(
            session,
            ControlSent {
                key,
                control,
                unsent,
                told: false,
            },
        );
    }

    /// Tells each settled control once: a refusal opens the refusal sheet with corrald's
    /// reason, anything else is a toast. The rewind list is read by its sheet instead.
    pub fn settle_controls(&mut self, feed: &Feed) -> Vec<Effect> {
        let mut effects = Vec::new();
        for (session, sent) in &mut self.controls {
            if sent.told || sent.unsent.is_some() {
                continue;
            }
            let outcome = outcome(sent.control, feed.operations.get(sent.key.as_str()));
            let title = format!("{} {session}", sent.control.label());
            match outcome {
                Outcome::Pending => continue,
                Outcome::Refused(refusal) if sent.control != Control::Rewinds => {
                    effects.push(Effect::Refused(RefusalSheet { title, refusal }));
                }
                Outcome::Done(line) if sent.control != Control::Rewinds => {
                    effects.push(Effect::Toast(format!("{session} · {line}")));
                }
                Outcome::Failed(why) => {
                    effects.push(Effect::Toast(format!("{title} · failed: {why}")))
                }
                _ => {}
            }
            sent.told = true;
        }
        effects
    }

    /// Picks the turn to rewind to: the sheet closes and the rewind asks once before it goes.
    pub(super) fn rewind_to(&mut self, turn: u64) -> Vec<Effect> {
        let Some(rewinding) = self.rewind.take() else {
            return Vec::new();
        };
        vec![Effect::Control {
            session: rewinding.session,
            key: self.ids.next_id(),
            control: Control::Rewind(turn),
            confirmed: false,
        }]
    }
}

/// One point the rewind sheet offers: the start of a turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RewindPoint {
    pub turn: u64,
    /// `Turn 4`.
    pub label: String,
    /// `start a1b2c3d`, or why the checkpoint cannot be restored.
    pub detail: String,
    /// When the host recorded the checkpoint (`14:02`), absent from hosts that do not say.
    pub when: Option<String>,
    pub usable: bool,
}

/// What the rewind sheet shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RewindSheet {
    pub session: Id,
    pub title: String,
    /// What a rewind does, said once before the owner picks.
    pub note: &'static str,
    /// Loading, refused or empty, in words; none once points are listed.
    pub status: Option<String>,
    /// The heading over the points.
    pub turns: &'static str,
    /// Newest turn first.
    pub points: Vec<RewindPoint>,
    /// The heading over the rewinds already done.
    pub done: &'static str,
    /// Rewinds already done, newest first, each with the rescue ref its work was saved under.
    pub earlier: Vec<String>,
}

const REWIND_NOTE: &str = "Pick the turn to go back to. The current work is saved under a rescue ref first; you confirm once before anything changes.";

/// The rewind sheet while it is open.
pub fn rewind_sheet(state: &State, feed: &Feed) -> Option<RewindSheet> {
    let rewinding = state.rewind.as_ref()?;
    let mut sheet = RewindSheet {
        session: rewinding.session.clone(),
        title: format!("Rewind {}", rewinding.session),
        note: REWIND_NOTE,
        status: None,
        turns: "Go back to the start of",
        points: Vec::new(),
        done: "Earlier rewinds",
        earlier: Vec::new(),
    };
    match outcome(
        Control::Rewinds,
        feed.operations.get(rewinding.key.as_str()),
    ) {
        Outcome::Pending => sheet.status = Some("Asking the host for its turns…".into()),
        Outcome::Refused(refusal) => sheet.status = Some(format!("Refused: {}", refusal.reason)),
        Outcome::Failed(why) => sheet.status = Some(format!("Could not list turns: {why}")),
        Outcome::Done(_) => {
            let operation = feed.operations.get(rewinding.key.as_str());
            match operation.and_then(Operation::decode::<RewindReply>) {
                Some(RewindReply::List {
                    checkpoints,
                    rewinds,
                }) => {
                    let mut turns: Vec<RewindPoint> = Vec::new();
                    for point in checkpoints.iter().filter(|point| point.phase == "start") {
                        if turns.iter().any(|known| known.turn == point.turn) {
                            continue;
                        }
                        let usable = point.error.is_none() && point.commit.is_some();
                        let detail = match (&point.error, &point.commit) {
                            (Some(error), _) => format!("cannot be restored: {error}"),
                            (None, Some(commit)) => {
                                format!("start {}", commit.chars().take(7).collect::<String>())
                            }
                            (None, None) => "no snapshot recorded".into(),
                        };
                        turns.push(RewindPoint {
                            turn: point.turn,
                            label: format!("Turn {}", point.turn),
                            detail,
                            when: super::live::clock(point.at.as_deref(), feed),
                            usable,
                        });
                    }
                    turns.sort_by(|a, b| b.turn.cmp(&a.turn));
                    sheet.points = turns;
                    sheet.earlier = rewinds
                        .iter()
                        .rev()
                        .map(|done| {
                            let what =
                                format!("to turn {} · saved as {}", done.turn, done.rescue_ref);
                            match super::live::clock(done.at.as_deref(), feed) {
                                Some(when) => format!("{when} · {what}"),
                                None => what,
                            }
                        })
                        .collect();
                    if sheet.points.is_empty() {
                        sheet.status = Some("The host recorded no turn to go back to yet.".into());
                    }
                }
                _ => sheet.status = Some("The host's answer could not be read.".into()),
            }
        }
    }
    Some(sheet)
}
