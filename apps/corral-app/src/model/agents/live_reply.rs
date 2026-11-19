//! A run session's reply box: the words go to the run's task as the engine's `reply`, which hands
//! them to its live session. The engine decides whether the task takes a reply now; a refusal
//! comes back with its next steps through the shared refusal sheet, never predicted here.

use corral_protocol::rows::AgentRow;
use corral_types::Op;

use crate::model::daemon::{self, Link};
use crate::model::effect::{Effect, Env};
use crate::model::hue::Hue;
use crate::model::text::span;
use crate::model::{Act, ActStatus, Id, act};

use super::live::role_label;
use super::{Reply, ReplyMode, State, reply_busy};

const OFFLINE: &str = "Needs a live connection to corrald; nothing is sent or queued offline.";

/// The reply box of session `id`. `via` is this device's authenticated attribution.
pub(super) fn reply(
    id: &Id,
    row: Option<&AgentRow>,
    shown: &str,
    state: &State,
    feed: &daemon::Feed,
    env: Env,
    via: &str,
) -> Reply {
    let mode = match row {
        None => ReplyMode::Closed {
            reason: "Messages open once corrald lists this session.".into(),
            action: None,
        },
        Some(_) if env.conn.refuses() || feed.link != Link::Up => ReplyMode::Closed {
            reason: OFFLINE.into(),
            action: None,
        },
        Some(_) if reply_busy(state, id, feed) => ReplyMode::Busy,
        Some(_) => ReplyMode::Open,
    };
    let key = state.replies.get(id.as_str());
    let outcome = key.and_then(|key| act::status(feed, key));
    let status = outcome.as_ref().map(|status| match status {
        ActStatus::Done => (Hue::Ok, format!("Delivered to {shown}")),
        ActStatus::Failed(_) | ActStatus::Refused(_) => (Hue::Fail, status.line()),
        _ => (Hue::Neutral, status.line()),
    });
    let role = row.map_or("session", |row| row.run.role.as_str());
    Reply {
        placeholder: format!("Message {} {shown}…", role_label(role)),
        accessibility_label: "Message the session",
        button: "Send",
        hint: vec![
            span("Arrives in "),
            span(shown.to_owned()).mono(),
            span(" as owner input · "),
            span(via.to_owned()).mono(),
        ],
        mode,
        status,
        delivered: matches!(outcome, Some(ActStatus::Done))
            .then(|| key.map(ToString::to_string))
            .flatten(),
    }
}

impl State {
    /// Sends the typed words to the run's task as `reply`; a reply in flight blocks another.
    pub(super) fn reply(&mut self, session: Id, text: String, feed: &daemon::Feed) -> Vec<Effect> {
        let text = text.trim();
        if text.is_empty() || reply_busy(self, &session, feed) {
            return vec![];
        }
        let Some(row) = feed.agents.get(session.as_str()) else {
            return vec![];
        };
        let act = Act::new(
            &mut self.ids,
            Op::Reply {
                id: row.run.task.clone(),
                text: text.to_owned(),
            },
            false,
        );
        self.replies.insert(session, act.key.clone());
        vec![Effect::Act(act)]
    }
}
