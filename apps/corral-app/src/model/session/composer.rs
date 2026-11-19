//! The one composer of a session page: who receives the message, the working indicator, the
//! draft, the model and effort the host reports, and the send controls. Enter sends; while the
//! agent works Enter steers the running turn and Queue holds the text for after it. The send's
//! command id is the input id the host and the transcript keep (`echo`). A capability the host
//! declares unavailable shows its control muted with the host's reason; one it does not declare
//! is not drawn.

use corral_protocol::session_state::SendPolicy;

use crate::model::agents::Control;
use crate::model::daemon::Link;
use crate::model::hue::Hue;

use super::activity::{self, ActivityLine};
use super::echo::{self, EchoState};
use super::{Action, Can, Cx, Host, SessionRole};

#[derive(Debug, Clone, PartialEq)]
pub struct SessionComposer {
    pub title: ComposerTitle,
    pub activity: Option<ActivityLine>,
    pub draft: String,
    pub mode: SendMode,
    pub model: Option<ModelPill>,
    pub effort: Option<EffortControl>,
    pub receiver: Option<ReceiverPill>,
    /// The attachment tray's `+`; absent where the host takes none, muted with its reason.
    pub attachments: Option<Option<String>>,
    pub send: SendButton,
    /// `Queue` (Alt+Enter) while the agent works.
    pub queue: Option<SendButton>,
    /// `Stop` while the agent works.
    pub stop: Option<StopButton>,
    /// The key line under the field: `⏎ steer  esc stop  Alt+⏎ queue`.
    pub keys: Vec<(&'static str, &'static str)>,
}

/// `Message → parent · last input queued`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposerTitle {
    pub lead: &'static str,
    /// `→ parent`, `→ a2`, `→ assistant`.
    pub to: String,
    /// The newest send's standing, while it is current: `last input queued`.
    pub last: Option<(Hue, String)>,
}

/// Whether Enter sends a new message or steers a running turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendMode {
    Idle,
    Working,
}

/// `<provider>/<model> ▾`, as the host reports it; the picker that changes it is its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelPill {
    pub label: String,
    /// `→ <model>` until the session takes the pending one.
    pub pending: Option<String>,
    /// The host serves model changes but cannot change it now.
    pub unavailable: Option<String>,
}

/// `· effort high`, with the levels the model reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffortControl {
    pub level: Option<String>,
    pub levels: Vec<String>,
}

/// `→ parent ▾`: who the message goes to; the parent or a child that can take a message.
#[derive(Debug, Clone, PartialEq)]
pub struct ReceiverPill {
    pub label: String,
    pub choices: Vec<ReceiverChoice>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReceiverChoice {
    /// None for the parent.
    pub child: Option<String>,
    /// `parent`, `a2 · running`.
    pub label: String,
    pub selected: bool,
    pub action: Action,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SendButton {
    /// `btn-send`, `btn-steer`, `btn-queue`.
    pub id: &'static str,
    pub label: &'static str,
    pub policy: SendPolicy,
    /// Muted with this reason; pressing it does nothing.
    pub unavailable: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StopButton {
    pub label: &'static str,
    pub action: Action,
}

/// What the composer's place shows when the session takes no message now.
#[derive(Debug, Clone, PartialEq)]
pub struct HostBar {
    /// `The host is not running. Its transcript is kept.`
    pub text: String,
    /// `Resume` (build) or `Relaunch` (assistant, run), when it can be started again here.
    pub action: Option<(&'static str, Action)>,
}

/// Whether the session's agent works now.
pub fn working(cx: &Cx<'_>) -> bool {
    cx.state.is_some_and(|state| state.open) || cx.live.is_some_and(|live| live.open)
}

/// The bar in the composer's place, when the session takes no message: a child transcript, an
/// ended session, a stopped host that must be started first, a session without a Pi host.
pub fn host_bar(cx: &Cx<'_>) -> Option<HostBar> {
    if *cx.role == SessionRole::Child {
        return Some(HostBar {
            text:
                "A child agent's conversation is read-only here; steer it from its parent's page."
                    .into(),
            action: None,
        });
    }
    if cx.log_only() {
        return Some(HostBar {
            text: "This session has no Pi host; its log is read-only.".into(),
            action: None,
        });
    }
    let relaunch = |label: &'static str| {
        Some((
            label,
            Action::Control {
                session: cx.id.clone(),
                control: Control::Relaunch,
            },
        ))
    };
    match cx.host {
        Host::Ended => Some(HostBar {
            text: if cx.role.task_bound() {
                "This run has ended.".into()
            } else {
                "This session has ended.".into()
            },
            action: None,
        }),
        Host::NotRunning if cx.elsewhere() => Some(HostBar {
            text:
                "This build runs in another terminal that corrald cannot reach; continue it there."
                    .into(),
            action: None,
        }),
        // A message to a stopped chat starts its host; any other role is started first.
        Host::NotRunning if *cx.role == SessionRole::Assistant => None,
        Host::NotRunning => Some(HostBar {
            text: "The host is not running. Its transcript is kept.".into(),
            action: relaunch(if *cx.role == SessionRole::Build {
                "Resume"
            } else {
                "Relaunch"
            }),
        }),
        Host::Running | Host::Unreachable | Host::Unknown => None,
    }
}

/// The send refusal the page knows before sending: no link, or the host's own reason.
fn refusal(cx: &Cx<'_>) -> Option<String> {
    if cx.env.conn.refuses() || cx.feed.link != Link::Up {
        return Some("corrald is unreachable".into());
    }
    match cx.can("send") {
        Can::Unavailable(why) => return Some(why),
        Can::Absent => return Some("this session takes no messages".into()),
        Can::Yes | Can::Unknown => {}
    }
    cx.page.filter(|page| !page.replyable).map(|page| {
        page.reply_refusal
            .clone()
            .unwrap_or_else(|| "this session takes no messages now".into())
    })
}

fn receiver(cx: &Cx<'_>) -> Option<ReceiverPill> {
    let fleet = cx.live.and_then(|live| live.fleet.as_ref())?;
    let steerable: Vec<_> = fleet
        .children
        .iter()
        .filter(|child| child.steerable)
        .collect();
    if steerable.is_empty() || matches!(cx.can("children"), Can::Absent) {
        return None;
    }
    let parent = if *cx.role == SessionRole::Build {
        "parent"
    } else {
        "agent"
    };
    let to = cx.ui.to.as_deref();
    let mut choices = vec![ReceiverChoice {
        child: None,
        label: parent.into(),
        selected: to.is_none(),
        action: Action::Receiver {
            session: cx.id.clone(),
            to: None,
        },
    }];
    choices.extend(steerable.iter().map(|child| ReceiverChoice {
        child: Some(child.id.clone()),
        label: format!("{} · {}", child.id, child.state),
        selected: to == Some(child.id.as_str()),
        action: Action::Receiver {
            session: cx.id.clone(),
            to: Some(child.id.clone()),
        },
    }));
    Some(ReceiverPill {
        label: format!("→ {} ▾", to.unwrap_or(parent)),
        choices,
    })
}

fn receiver_word(cx: &Cx<'_>) -> String {
    if let Some(child) = &cx.ui.to {
        return child.clone();
    }
    match cx.role {
        SessionRole::Build => "parent".into(),
        SessionRole::Assistant => "assistant".into(),
        role => role.label(),
    }
}

/// The newest send's standing while it is not served yet.
fn last(cx: &Cx<'_>) -> Option<(Hue, String)> {
    let page = cx.page;
    let sent = cx
        .ui
        .echoes
        .iter()
        .rev()
        .find(|sent| !echo::retired(sent, page, cx.feed))?;
    match echo::state(sent, cx.id.as_str(), cx.feed) {
        EchoState::Pending => Some((Hue::Neutral, "last input pending admission".into())),
        EchoState::Queued { .. } => Some((Hue::Neutral, "last input queued".into())),
        EchoState::Refused(why) => Some((Hue::Fail, format!("last input refused: {why}"))),
        EchoState::Sending | EchoState::Started | EchoState::Withdrawn => None,
    }
}

fn unavailable(can: Can) -> Option<String> {
    match can {
        Can::Unavailable(why) => Some(why),
        _ => None,
    }
}

/// The composer of the session `cx` reads; none where the session takes no message (see
/// [`host_bar`]).
pub fn composer(cx: &Cx<'_>) -> Option<SessionComposer> {
    if host_bar(cx).is_some() {
        return None;
    }
    let working = working(cx);
    let refused = refusal(cx);
    let child = cx.ui.to.is_some();
    let (send, queue) = if working {
        let steer = SendButton {
            id: "btn-steer",
            label: "Steer",
            policy: SendPolicy::Steer,
            unavailable: refused.clone().or_else(|| unavailable(cx.can("steer"))),
        };
        let queue = (!matches!(cx.can("follow_up"), Can::Absent)).then(|| SendButton {
            id: "btn-queue",
            label: "Queue",
            policy: SendPolicy::FollowUp,
            unavailable: refused
                .clone()
                .or_else(|| child.then(|| "Only the parent takes queued follow-ups".into()))
                .or_else(|| unavailable(cx.can("follow_up"))),
        });
        (steer, queue)
    } else {
        let send = SendButton {
            id: "btn-send",
            label: "Send",
            policy: SendPolicy::Steer,
            unavailable: refused,
        };
        (send, None)
    };
    let stop = (working && cx.controls().contains(&Control::Cancel)).then(|| StopButton {
        label: "Stop",
        action: Action::Control {
            session: cx.id.clone(),
            control: Control::Cancel,
        },
    });
    let model = cx.model().map(|label| ModelPill {
        label,
        pending: cx.state.and_then(|state| state.pending_model.clone()),
        unavailable: unavailable(cx.can("model")),
    });
    let effort = cx
        .state
        .filter(|state| !state.effort_levels.is_empty())
        .map(|state| EffortControl {
            level: state.effort.clone(),
            levels: state.effort_levels.clone(),
        });
    let attachments = match cx.can("attachments") {
        Can::Yes => Some(None),
        Can::Unavailable(why) => Some(Some(why)),
        Can::Absent | Can::Unknown => None,
    };
    Some(SessionComposer {
        title: ComposerTitle {
            lead: "Message",
            to: format!("→ {}", receiver_word(cx)),
            last: last(cx),
        },
        activity: activity::line(cx.state, cx.live, cx.feed),
        draft: cx.ui.draft.clone(),
        mode: if working {
            SendMode::Working
        } else {
            SendMode::Idle
        },
        model,
        effort,
        receiver: receiver(cx),
        attachments,
        keys: super::keys::legend(super::Focus::Composer, super::Tab::Chat, working),
        send,
        queue,
        stop,
    })
}
