//! App operations forwarded to the owner controls of a Pi session.
use crate::assistant::SendAttachment;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub use crate::session_state::SendPolicy;

pub const SESSION_SEND: &str = "session.send";
pub const SESSION_QUEUE_WITHDRAW: &str = "session.queue.withdraw";
pub const SESSION_QUEUE_MOVE: &str = "session.queue.move";
pub const SESSION_MODEL: &str = "session.model";
pub const SESSION_CARD_ANSWER: &str = "session.card.answer";
pub const SESSION_CHILD: &str = "session.child";
pub const SESSION_JOB: &str = "session.job";
pub const SESSION_ACTION: &str = "session.action";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionSend {
    pub id: String,
    /// Sender-chosen input ID: `[A-Za-z0-9._:-]{1,80}`.
    pub cmd_id: String,
    /// At most `crate::assistant::MESSAGE_TEXT_MAX` bytes.
    pub text: String,
    pub policy: SendPolicy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub typed_at: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<SendAttachment>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionSent {
    /// Concrete session ID, even when the request named `quick`.
    pub session: String,
    /// `<session>/input/<cmd_id>`, matching the served message's ID.
    pub entry: String,
    pub disposition: InputDisposition,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum InputDisposition {
    Started,
    Queued,
    Handled,
    Pending,
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionQueueKey {
    pub id: String,
    pub key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct QueueWithdrawn {
    pub outcome: WithdrawOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<u64>,
    /// Whole input, not the queue preview.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WithdrawOutcome {
    Withdrawn,
    AlreadyStarted,
    UnknownItem,
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionQueueMove {
    pub id: String,
    pub key: String,
    /// `-1` or `1`.
    pub delta: i8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct QueueMoved {
    pub moved: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionModel {
    pub id: String,
    /// `<provider>/<model>`.
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionModelSet {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionCardAnswer {
    pub id: String,
    pub card: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub answers: Vec<CardAnswer>,
    #[serde(default)]
    pub dismiss: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CardAnswer {
    /// Option IDs from the card question.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionCardAnswered {
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionChildAct {
    pub id: String,
    pub child: String,
    #[serde(flatten)]
    pub action: ChildAction,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum ChildAction {
    Cancel,
    SetModel {
        model: String,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionJobAct {
    pub id: String,
    pub job: String,
    pub action: JobAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum JobAction {
    Stop,
    Background,
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionJobActed {
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionActed {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionActionRequest {
    pub id: String,
    #[serde(flatten)]
    pub action: SessionAction,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum SessionAction {
    ExitOpen,
    BoardRefresh,
    Compact {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        instructions: Option<String>,
    },
    Export,
    Shell {
        command: String,
        exclude_from_context: bool,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionActionDone {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detail::{CardKind, DetailBody};
    use serde_json::{Value, json};

    fn round_trip<T>(wire: Value) -> T
    where
        T: Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
    {
        let value: T = serde_json::from_value(wire).unwrap();
        let again: T = serde_json::from_value(serde_json::to_value(&value).unwrap()).unwrap();
        assert_eq!(value, again);
        value
    }

    #[test]
    fn unknown_card_kinds_and_actions_survive_a_round_trip() {
        let body: DetailBody = round_trip(json!({
            "kind": "session_cards",
            "body": { "cards": [{
                "id": "card-1", "kind": "future_card", "title": "Owner input",
                "body_more": false, "opened_at": "2026-11-19T12:00:00Z",
                "future_field": true
            }] }
        }));
        let DetailBody::SessionCards(cards) = body else {
            panic!("expected session cards");
        };
        assert_eq!(cards.cards[0].kind, CardKind::Other);
        assert_eq!(cards.cards[0].id, "card-1");

        let action: SessionActionRequest = round_trip(json!({
            "id": "B-one", "action": "future_action", "future_field": true
        }));
        assert_eq!(action.action, SessionAction::Other);
        assert_eq!(action.id, "B-one");
        let child: SessionChildAct = round_trip(json!({
            "id": "B-one", "child": "review", "action": "future_action", "future_field": true
        }));
        assert_eq!(child.action, ChildAction::Other);
        let job: SessionJobAct = round_trip(json!({
            "id": "B-one", "job": "job-1", "action": "future_action"
        }));
        assert_eq!(job.action, JobAction::Other);
    }
}
