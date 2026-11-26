//! Reconnect-safe observations and tagged delivery for board obligations.

#[cfg(target_os = "linux")]
use super::task_host;
use super::{
    Client, Disposition, InputTag, Origin, PiRefusal, PiRunner, Runner, SessionRef, SessionStatus,
    UncertainSubmit, request_id, unavailable,
};
use crate::refuse;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::io;

#[derive(Serialize, Deserialize)]
pub(crate) struct Pickup {
    pub input_id: String,
    pub rejected: bool,
    pub before_turn: Option<u64>,
}

#[derive(Deserialize)]
struct State {
    paused: bool,
    turn: Turn,
    owner_questions: Vec<Question>,
    last_error: Option<String>,
    activity: Option<Activity>,
}

#[derive(Deserialize)]
pub(crate) struct Activity {
    pub settled: bool,
    pub busy: bool,
    pub waiting: bool,
    pub turn: u64,
    pub inputs: std::collections::HashMap<String, InputState>,
}

#[derive(Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case")]
pub(crate) enum InputState {
    Pending { quiet_ms: u64 },
    Applied,
    Failed { turn: u64 },
}

#[derive(Deserialize)]
struct Turn {
    open: bool,
}

#[derive(Deserialize)]
struct Question {
    question: String,
}

impl PiRunner<'_> {
    /// Read status and the pause flag from the same observation of the Pi host.
    pub fn observe(&self, session: &SessionRef) -> Result<(SessionStatus, Option<bool>)> {
        self.observe_activity(session)
            .map(|(status, paused, _)| (status, paused))
    }

    pub(crate) fn observe_activity(
        &self,
        session: &SessionRef,
    ) -> Result<(SessionStatus, Option<bool>, Option<Activity>)> {
        let SessionRef::Pi {
            socket,
            inner,
            dir: _dir,
        } = session
        else {
            return Ok((self.inner.status(session)?, None, None));
        };
        let state: State = match Client::new(socket).request("get_state", json!({})) {
            Ok(state) => state,
            #[cfg(target_os = "linux")]
            Err(error)
                if error.downcast_ref::<io::Error>().is_some_and(|e| {
                    matches!(
                        e.kind(),
                        io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
                    )
                }) && task_host::confirmed_absent(_dir, socket) =>
            {
                return Ok((SessionStatus::Gone, None, None));
            }
            Err(error) if unavailable(&error) => {
                return Ok((self.inner.status(inner)?, None, None));
            }
            Err(error) => return Err(error),
        };
        let status = if let Some(question) = state.owner_questions.into_iter().next() {
            SessionStatus::Blocked {
                excerpt: question.question,
            }
        } else if state.turn.open {
            SessionStatus::Working
        } else if !state.paused
            && let Some(excerpt) = state.last_error
        {
            SessionStatus::Blocked { excerpt }
        } else {
            SessionStatus::Idle
        };
        Ok((status, Some(state.paused), state.activity))
    }

    /// Tag board obligations without mistaking a queue acknowledgement for pickup.
    /// An explicit rejection is evidence; an I/O error is not a rejection.
    pub(crate) fn send_obligation(
        &self,
        session: &SessionRef,
        text: &str,
        origin: Origin,
    ) -> Result<Option<Pickup>> {
        let SessionRef::Pi { socket, dir, .. } = session else {
            self.inner.send(session, text, origin)?;
            return Ok(None);
        };
        let before_turn = self
            .observe_activity(session)
            .ok()
            .and_then(|(_, _, activity)| activity)
            .map(|activity| activity.turn);
        let id = format!("board-{}", request_id());
        let reply = Client::with_dir(socket, dir).submit(
            text,
            origin,
            Some(InputTag {
                input_id: &id,
                to: None,
                typed_at: None,
                device: None,
                follow_up: false,
                attachments: &[],
            }),
        );
        let rejected = match reply {
            Ok(reply) => {
                reply.disposition == Disposition::Rejected
                    && reply.input.as_deref() == Some(id.as_str())
            }
            Err(error)
                if error.downcast_ref::<PiRefusal>().is_some_and(|refusal| {
                    matches!(refusal.code.as_str(), "busy" | "starting" | "bad_input")
                }) =>
            {
                true
            }
            // The frame may have arrived without a usable acknowledgement. Retain
            // its ID through timeouts, malformed frames and post-admission errors.
            Err(error) if error.is::<UncertainSubmit>() => false,
            Err(error) if unavailable(&error) => {
                return Err(refuse(
                    "session has no running Pi host; relaunch it first",
                    Vec::new(),
                ));
            }
            Err(error) => return Err(error),
        };
        Ok(Some(Pickup {
            input_id: id,
            rejected,
            before_turn,
        }))
    }

    /// An older Pi host can answer but fail to fit its diagnostic state in a frame.
    /// Its refusal proves liveness, not a lifecycle status. Never use this in reconciliation.
    pub(crate) fn is_live(&self, session: &SessionRef) -> Result<bool> {
        match self.status(session) {
            Ok(status) => Ok(status != SessionStatus::Gone),
            Err(error)
                if matches!(session, SessionRef::Pi { .. })
                    && error.downcast_ref::<PiRefusal>().is_some_and(|refusal| {
                        refusal.command == "get_state" && refusal.code == "response_too_large"
                    }) =>
            {
                Ok(true)
            }
            Err(error) => Err(error),
        }
    }
}
