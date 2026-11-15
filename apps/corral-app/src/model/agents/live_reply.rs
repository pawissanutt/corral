//! A daemon session's reply box: open while corrald offers `send` on the session's row, the words
//! sent as `session send`, after the current turn by default and as a steer when the owner
//! chooses to interrupt a working session; closed otherwise with corrald's reason. An assistant
//! chats through the shared composer instead (`Session::chat`); its box shows only why it is
//! closed.

use corral_protocol::cmd::Command;
use corral_protocol::cmd::session::SessionCommand;
use corral_protocol::rows::{AgentRow, AgentStatus as RowStatus};

use crate::model::daemon;
use crate::model::effect::{Effect, Env};
use crate::model::hue::Hue;
use crate::model::text::span;
use crate::model::{Act, ActStatus, Id, Live, act};

use super::controls::{device_refusal, refusal_words};
use super::live::{role_label, shown_id};
use super::{Action, Reply, ReplyMode, State, reply_busy};

/// What a busy host does with the owner's words.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Policy {
    /// After its current turn.
    FollowUp,
    /// Now, interrupting the turn.
    Steer,
}

impl Policy {
    const fn wire(self) -> &'static str {
        match self {
            Self::FollowUp => "follow-up",
            Self::Steer => "steer",
        }
    }
}

/// Why words do not go to this session, from corrald's refusal, else from its role.
fn closed_reason(row: &AgentRow) -> String {
    match row.controls.send.refusal.as_deref() {
        Some(code) => format!("Messages are closed: {}.", refusal_words(code)),
        None if matches!(row.role.as_str(), "reviewer" | "plan_reviewer") => {
            "A reviewer takes no messages; they go to the task's worker.".into()
        }
        None => "corrald does not take messages for this session now.".into(),
    }
}

/// The reply box of session `id`. `via` is this device's authenticated attribution.
#[allow(clippy::too_many_arguments)]
pub(super) fn reply(
    id: &Id,
    row: Option<&AgentRow>,
    role: &str,
    shown: &str,
    open_question: bool,
    state: &State,
    feed: &daemon::Feed,
    env: Env,
    via: &str,
) -> Reply {
    let working = row.is_some_and(|row| row.status == RowStatus::Working);
    let paused = row.is_some_and(|row| row.status == RowStatus::Paused);
    let mode = match row {
        None => ReplyMode::Closed {
            reason: "Messages open once corrald lists this session.".into(),
            action: None,
        },
        Some(row) if row.controls.send.available => match device_refusal(feed, env, false) {
            Some(why) => ReplyMode::Closed {
                reason: why,
                action: None,
            },
            None if reply_busy(state, id, feed) => ReplyMode::Busy,
            None => ReplyMode::Open,
        },
        Some(row) => ReplyMode::Closed {
            reason: closed_reason(row),
            action: (row.role == "assistant")
                .then(|| ("Open thread", Action::OpenThread(id.clone()))),
        },
    };
    let steer = (working && mode == ReplyMode::Open).then_some("Steer");
    let key = state.replies.get(id.as_str());
    let outcome = key.and_then(|key| act::status(feed, key));
    let status = outcome.as_ref().map(|status| match status {
        ActStatus::Done => (Hue::Ok, format!("Delivered to {shown}")),
        ActStatus::Failed(_) | ActStatus::Refused(_) | ActStatus::Conflict(_) => {
            (Hue::Fail, status.line())
        }
        ActStatus::Unknown => (
            Hue::Held,
            "Delivery unknown · it may have arrived; check before sending again".into(),
        ),
        ActStatus::Abandoned => (Hue::Held, status.line()),
        _ => (Hue::Neutral, status.line()),
    });
    let mut hint = vec![
        span("Arrives in "),
        span(shown.to_owned()).mono(),
        span(" as owner input · "),
        span(via.to_owned()).mono(),
    ];
    if paused {
        hint.push(span(" · sending resumes the paused session"));
    }
    if steer.is_some() {
        hint.push(span(
            " · Send waits for the current turn; Steer interrupts it",
        ));
    }
    Reply {
        placeholder: format!("Message {} {shown}…", role_label(role)),
        accessibility_label: "Message the session",
        button: "Send",
        steer,
        hint,
        question_hint: if open_question {
            "Answering the open question happens on its handover sheet".into()
        } else {
            String::new()
        },
        mode,
        status,
        recover: matches!(outcome, Some(ActStatus::Unknown))
            .then(|| key.map(|key| Action::Recover(key.clone())))
            .flatten(),
        delivered: matches!(outcome, Some(ActStatus::Done))
            .then(|| key.map(ToString::to_string))
            .flatten(),
    }
}

impl State {
    /// Sends the typed words to the session as owner input (`session send`), while corrald
    /// offers `send` on its row; a reply being prepared or in flight blocks another. A steer is
    /// sent as one only while the session works; otherwise the words simply follow.
    pub(super) fn reply(
        &mut self,
        session: Id,
        text: String,
        policy: Policy,
        feed: &daemon::Feed,
    ) -> Vec<Effect> {
        let text = text.trim();
        if text.is_empty() || reply_busy(self, &session, feed) {
            return vec![];
        }
        let Some(row) = feed
            .agents
            .get(session.as_str())
            .filter(|row| row.controls.send.available && row.role != "assistant")
        else {
            return vec![];
        };
        let policy = if row.status == RowStatus::Working {
            policy
        } else {
            Policy::FollowUp
        };
        let act = Act::new(
            &mut self.ids,
            format!("Message {}", shown_id(row)),
            Command::Session(SessionCommand::Send {
                id: row.id.clone(),
                text: text.to_owned(),
                policy: policy.wire().into(),
                json: false,
            }),
        )
        .about(Live::Session(session.clone()));
        self.replies.insert(session, act.key.clone());
        vec![Effect::Act(act)]
    }
}
