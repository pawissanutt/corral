//! Typed session commands; see the family rules in the parent module.
//!
//! `id` is a session id as the Agents rows carry it (`AgentRow::id`): `pm`, an assistant
//! `A-<n>`, or a task container key. A command never names a pane.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "verb", rename_all = "kebab-case")]
pub enum SessionCommand {
    /// Cancel a turn of a live host, as Esc does in its terminal. The session stays live.
    Cancel {
        id: String,
        /// Refuse with `conflict` unless this is still the host's generation.
        generation: Option<u32>,
        /// Turn sequence to cancel (default: the current turn).
        turn: Option<u64>,
        json: bool,
    },
    /// Cancel the current turn and fence the session against automated input until `resume`.
    Pause {
        id: String,
        generation: Option<u32>,
        json: bool,
    },
    /// Lift a pause; input queued meanwhile is delivered in order.
    Resume { id: String, json: bool },
    /// Stop the host and end the session. A task keeps its worktree and waits in Needs me.
    Stop {
        id: String,
        generation: Option<u32>,
        json: bool,
    },
    /// Fence a lost host and start the next generation of the same session.
    Relaunch {
        id: String,
        /// Replace a host that is still alive after verifying its identity.
        replace: bool,
        json: bool,
    },
    /// Open the PM when none is live.
    PmOpen { json: bool },
    /// Owner words typed into the session. Lifts a pause.
    Send {
        id: String,
        text: String,
        /// `steer` or `follow-up`: what a busy host does with the words.
        policy: String,
        json: bool,
    },
    /// Keys, then optional text, typed into the session's pane. Only for a session with a pane
    /// and no host bridge.
    Keys {
        id: String,
        keys: Vec<String>,
        text: Option<String>,
        json: bool,
    },
    #[serde(other)]
    Other,
}

impl SessionCommand {
    pub fn verb(&self) -> &'static str {
        match self {
            Self::Cancel { .. } => "cancel",
            Self::Pause { .. } => "pause",
            Self::Resume { .. } => "resume",
            Self::Stop { .. } => "stop",
            Self::Relaunch { .. } => "relaunch",
            Self::PmOpen { .. } => "pm-open",
            Self::Send { .. } => "send",
            Self::Keys { .. } => "keys",
            Self::Other => "other",
        }
    }
}
