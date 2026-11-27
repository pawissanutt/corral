//! The one composer of a session page: who receives the message, the working indicator, the
//! draft, the model and effort the host reports, and the send controls. Enter sends; while the
//! agent works Enter steers the running turn and Queue holds the text for after it. The send's
//! command id is the input id the host and the transcript keep (`echo`). A capability the host
//! declares unavailable shows its control muted with the host's reason; one it does not declare
//! is not drawn. While the dock asks for the owner's reason or answer, Enter gives it instead.
//! A draft starting with `!` is a shell command the host runs for the owner (`!!`: its output
//! stays out of the agent's context), muted with why where the host does not run them; ↑ ↓ walk
//! the session's earlier owner messages (`prompt_history`); a last word starting with `@` opens
//! the file mention popup (`mention`).

use corral_protocol::session_host::SessionAction;
use corral_protocol::session_state::SendPolicy;

use crate::model::agents::Control;
use crate::model::daemon::Link;
use crate::model::hue::Hue;

use super::activity::{self, ActivityLine};
use super::dock::Compose;
use super::echo::{self, EchoState};
use super::mention::{self, MentionMenu};
use super::prompt_history::{self, HistoryKeys};
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
    /// While the draft is a `!command`: Enter runs it on the host instead of sending it.
    pub shell: Option<ShellMode>,
    /// What ↑ and ↓ do in the field: walk this session's earlier owner messages.
    pub history: HistoryKeys,
    /// The `@` file mention popup while the draft's last word is a mention.
    pub mention: Option<MentionMenu>,
}

/// `shell · output goes to the agent` (`!command`) or `shell · not in context` (`!!command`).
#[derive(Debug, Clone, PartialEq)]
pub struct ShellMode {
    pub hint: &'static str,
    /// Why the host cannot run it now; Enter then does nothing.
    pub unavailable: Option<String>,
    /// What Enter runs; none while the command is empty.
    pub run: Option<Action>,
}

/// The host action a `!command` draft runs: `!!` keeps its output out of the agent's context.
/// None when the draft is no shell command or names none.
pub fn shell_action(draft: &str) -> Option<SessionAction> {
    let rest = draft.trim().strip_prefix('!')?;
    let (command, exclude_from_context) = match rest.strip_prefix('!') {
        Some(command) => (command, true),
        None => (rest, false),
    };
    let command = command.trim();
    (!command.is_empty()).then(|| SessionAction::Shell {
        command: command.to_owned(),
        exclude_from_context,
    })
}

/// Shell mode for the draft of `cx`, with why the host cannot run it, as the catalog says of a
/// host action it does not serve.
fn shell(cx: &Cx<'_>, refused: Option<&String>) -> Option<ShellMode> {
    let draft = cx.ui.draft.trim_start();
    if !draft.starts_with('!') || cx.ui.dock.compose.is_some() {
        return None;
    }
    let unavailable = refused.cloned().or_else(|| match cx.can("shell") {
        Can::Absent => Some("this session's host does not run owner shell commands".into()),
        Can::Unavailable(why) => Some(why),
        Can::Yes | Can::Unknown => match cx.host {
            Host::NotRunning | Host::Ended => Some("the host is not running".into()),
            Host::Unreachable => Some("corrald cannot reach the host".into()),
            Host::Running | Host::Unknown => None,
        },
    });
    let run = shell_action(draft).map(|_| Action::Shell {
        session: cx.id.clone(),
        text: draft.to_owned(),
    });
    Some(ShellMode {
        hint: if draft.starts_with("!!") {
            "shell · not in context"
        } else {
            "shell · output goes to the agent"
        },
        unavailable,
        run,
    })
}

/// Whether the `/` menu may be open over the draft: a one-line `/` command not closed on.
fn slash_typed(cx: &Cx<'_>) -> bool {
    let draft = &cx.ui.draft;
    draft.starts_with('/')
        && !draft.contains('\n')
        && cx.ui.picker.slash_closed.as_deref() != Some(draft.as_str())
}

/// `Message → parent · last input queued`; `Message → land on main · your reason` (Held) while
/// the composer gives a reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposerTitle {
    pub lead: &'static str,
    /// `→ parent`, `→ a2`, `→ assistant`, `→ a2 · answer`.
    pub to: String,
    /// The receiver's tone: Held in reason mode, You in answer mode.
    pub hue: Hue,
    /// The newest send's standing, while it is current: `last input queued`.
    pub last: Option<(Hue, String)>,
}

/// What Enter does: send a new message, steer the running turn, or give what the dock asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendMode {
    Idle,
    Working,
    /// Enter chooses the docked option with the text as the owner's reason.
    Reason,
    /// Enter answers a child's question with the text.
    Answer,
}

/// `<provider>/<model> ▾`, as the host reports it; the picker that changes it is its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelPill {
    pub label: String,
    /// `→ <model>` until the session takes the pending one.
    pub pending: Option<String>,
    /// The host serves model changes but cannot change it now.
    pub unavailable: Option<String>,
    /// While an agent's conversation is open, the pill is that agent's: pressing runs this
    /// (its model choices) instead of opening the session's model list, and nothing while
    /// `unavailable`. None for the session's own agent.
    pub opens: Option<Action>,
}

/// `· effort high`, with the levels the model reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffortControl {
    pub level: Option<String>,
    pub levels: Vec<String>,
    /// An opened agent's effort, as reported: shown, not changed from the composer.
    pub fixed: bool,
}

/// `→ parent ▾`: who the message goes to; the parent or a child that can take a message.
#[derive(Debug, Clone, PartialEq)]
pub struct ReceiverPill {
    pub label: String,
    pub choices: Vec<ReceiverChoice>,
    /// The host serves its children but cannot take a message for them now: muted, this reason.
    pub unavailable: Option<String>,
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
        Host::NotRunning if *cx.role == SessionRole::Code => Some(HostBar {
            text: if cx.elsewhere() {
                format!(
                    "Continue in its terminal with corral code --resume {}.",
                    cx.id
                )
            } else {
                "The Code host is not running. Its transcript is kept.".into()
            },
            action: (!cx.elsewhere()).then(|| {
                (
                    "Resume",
                    Action::Control {
                        session: cx.id.clone(),
                        control: Control::ResumeCode,
                    },
                )
            }),
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

/// Why the composer cannot send yet.
fn refusal(cx: &Cx<'_>) -> Option<String> {
    if cx.env.conn.refuses() || cx.feed.link != Link::Up {
        return Some("corrald is unreachable".into());
    }
    if super::actions::model_changing(cx) {
        return Some("Waiting for the model change to finish".into());
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
    let label = match &cx.ui.dock.compose {
        Some(Compose::Answer { child, .. }) => {
            format!("→ {} · answer ▾", child.as_deref().unwrap_or(parent))
        }
        _ => format!("→ {} ▾", to.unwrap_or(parent)),
    };
    Some(ReceiverPill {
        label,
        choices,
        unavailable: unavailable(cx.can("children")),
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
        .find(|sent| !echo::retired(sent, cx.id.as_str(), page, cx.feed))?;
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
    let compose = cx.ui.dock.compose.as_ref();
    let unreachable = (cx.env.conn.refuses() || cx.feed.link != Link::Up)
        .then(|| "corrald is unreachable".to_owned());
    let shell = shell(cx, unreachable.as_ref());
    let (send, queue) = match (compose, &shell) {
        (None, Some(shell)) => {
            let run = SendButton {
                id: "btn-send",
                label: "Run",
                policy: SendPolicy::Steer,
                unavailable: shell.unavailable.clone(),
            };
            (run, None)
        }
        (Some(mode), _) => {
            let send = SendButton {
                id: "btn-send",
                label: match mode {
                    Compose::Reason { .. } => "Send reason",
                    Compose::Answer { .. } => "Send answer",
                },
                policy: SendPolicy::Steer,
                unavailable: refused,
            };
            (send, None)
        }
        (None, None) if working => {
            let steer = SendButton {
                id: "btn-steer",
                label: "Steer",
                policy: SendPolicy::Steer,
                unavailable: refused.clone().or_else(|| unavailable(cx.can("steer"))),
            };
            // The phone shows no queue, so it offers nothing to queue; Steer still reaches the
            // working agent.
            let queues = !matches!(cx.can("follow_up"), Can::Absent)
                && cx.env.form != crate::model::nav::Form::Phone;
            let queue = queues.then(|| SendButton {
                id: "btn-queue",
                label: "Queue",
                policy: SendPolicy::FollowUp,
                unavailable: refused
                    .clone()
                    .or_else(|| child.then(|| "Only the parent takes queued follow-ups".into()))
                    .or_else(|| unavailable(cx.can("follow_up"))),
            });
            (steer, queue)
        }
        (None, None) => {
            let send = SendButton {
                id: "btn-send",
                label: "Send",
                policy: SendPolicy::Steer,
                unavailable: refused,
            };
            (send, None)
        }
    };
    let stop = (working && cx.controls().contains(&Control::Cancel)).then(|| StopButton {
        label: "Stop",
        action: Action::Control {
            session: cx.id.clone(),
            control: Control::Cancel,
        },
    });
    // An open agent conversation shows that agent's model and effort, not the parent's.
    let (model, effort) = match super::fleet::opened_agent(cx) {
        Some(agent) => (
            Some(agent.model),
            agent.effort.map(|level| EffortControl {
                level: Some(level),
                levels: Vec::new(),
                fixed: true,
            }),
        ),
        None => (
            cx.model().map(|label| ModelPill {
                label,
                pending: cx.state.and_then(|state| state.pending_model.clone()),
                unavailable: unavailable(cx.can("model")),
                opens: None,
            }),
            cx.state
                .filter(|state| !state.effort_levels.is_empty())
                .map(|state| EffortControl {
                    level: state.effort.clone(),
                    levels: state.effort_levels.clone(),
                    fixed: false,
                }),
        ),
    };
    let attachments = match cx.can("attachments") {
        Can::Yes => Some(None),
        Can::Unavailable(why) => Some(Some(why)),
        Can::Absent | Can::Unknown => None,
    };
    let (to, hue, mode, keys) = match compose {
        Some(Compose::Reason { label, .. }) => (
            format!("→ {} · your reason", label.to_lowercase()),
            Hue::Held,
            SendMode::Reason,
            vec![("⏎", "send reason"), ("esc", "leave")],
        ),
        Some(Compose::Answer { child, .. }) => (
            format!(
                "→ {} · answer",
                child.clone().unwrap_or_else(|| receiver_word(cx))
            ),
            Hue::You,
            SendMode::Answer,
            vec![("⏎", "send answer"), ("esc", "leave")],
        ),
        None => (
            format!("→ {}", receiver_word(cx)),
            Hue::Neutral,
            if working {
                SendMode::Working
            } else {
                SendMode::Idle
            },
            super::keys::legend(super::Focus::Composer, super::Tab::Chat, working),
        ),
    };
    let mention = mention::menu(cx);
    let menus = cx.ui.picker.open.is_some() || slash_typed(cx) || mention.is_some();
    Some(SessionComposer {
        shell,
        history: prompt_history::keys(cx, menus),
        mention,
        title: ComposerTitle {
            lead: "Message",
            to,
            hue,
            last: last(cx),
        },
        activity: activity::line(cx.state, cx.live, cx.feed),
        draft: cx.ui.draft.clone(),
        mode,
        model,
        effort,
        receiver: receiver(cx),
        attachments,
        keys,
        send,
        queue,
        stop,
    })
}

#[cfg(test)]
mod tests {
    use corral_protocol::session_host::SessionActionRequest;

    use super::*;
    use crate::model::Id;
    use crate::model::daemon::Feed;
    use crate::model::effect::Effect;
    use crate::model::session::State;

    /// `!!ls` went to the agent as a message, so a command the owner meant to keep out of the
    /// agent's context was put in it.
    #[test]
    fn double_bang_runs_the_shell_command_kept_out_of_context() {
        let id = Id::from("B-1");
        let mut state = State::default();
        let effects = state.act(
            Action::Shell {
                session: id.clone(),
                text: "!!ls -la".into(),
            },
            &Feed::default(),
        );
        let sent: Vec<&SessionAction> = effects
            .iter()
            .filter_map(|effect| match effect {
                Effect::SessionAction {
                    request: SessionActionRequest { action, .. },
                    ..
                } => Some(action),
                _ => None,
            })
            .collect();
        assert_eq!(
            sent,
            [&SessionAction::Shell {
                command: "ls -la".into(),
                exclude_from_context: true,
            }]
        );
        assert!(
            !effects
                .iter()
                .any(|effect| matches!(effect, Effect::SessionSend { .. }))
        );
    }
}
