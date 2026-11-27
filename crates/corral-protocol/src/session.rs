//! Direct owner controls addressed to a public session scope, never to a pane.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const SESSION_CANCEL: &str = "session.cancel";
pub const SESSION_PAUSE: &str = "session.pause";
pub const SESSION_RESUME: &str = "session.resume";
pub const SESSION_TERMINATE: &str = "session.terminate";
pub const SESSION_RELAUNCH: &str = "session.relaunch";
pub const SESSION_REWIND: &str = "session.rewind";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionTarget {
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionCancel {
    pub id: String,
    /// If present, cancel only this turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionCancelReply {
    pub cancelled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionPauseReply {
    pub paused: bool,
    #[serde(flatten)]
    pub turn: SessionCancelReply,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionResumeReply {
    pub paused: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionRelaunched {
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionRewind {
    pub id: String,
    #[serde(flatten)]
    pub action: RewindAction,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum RewindAction {
    List,
    Turn { turn: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RewindReply {
    List {
        checkpoints: Vec<RewindCheckpoint>,
        rewinds: Vec<Rewound>,
    },
    Turn(Rewound),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RewindScope {
    Conversation,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RewindCheckpoint {
    pub id: String,
    pub phase: String,
    pub turn: u64,
    /// Absent on checkpoints backed by Git.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<RewindScope>,
    pub commit: Option<String>,
    pub head: Option<String>,
    #[serde(rename = "ref")]
    pub reference: Option<String>,
    pub leaf: Option<String>,
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Rewound {
    pub turn: u64,
    pub checkpoint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rescue_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_head: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn rewind_list_preserves_times_and_accepts_rows_without_them() {
        let checkpoint_time = "2026-11-18T12:01:00.000Z";
        let rewind_time = "2026-11-18T12:03:00.000Z";
        let reply: RewindReply = serde_json::from_value(json!({
            "kind": "list",
            "checkpoints": [
                {"id": "R1-t1-start", "phase": "start", "turn": 1},
                {"id": "R1-t2-start", "phase": "start", "turn": 2, "at": checkpoint_time,
                 "commit": "snapshot", "head": "second", "ref": "refs/worktree/checkpoints/2"}
            ],
            "rewinds": [
                {"turn": 1, "checkpoint": "R1-t1-start", "rescue_ref": "rescue1",
                 "from_head": "latest", "head": "initial"},
                {"turn": 2, "checkpoint": "R1-t2-start", "rescue_ref": "rescue2",
                 "from_head": "latest", "head": "second", "at": rewind_time}
            ]
        }))
        .unwrap();
        let RewindReply::List {
            checkpoints,
            rewinds,
        } = &reply
        else {
            panic!("expected a rewind list");
        };
        assert_eq!(checkpoints[0].at, None);
        assert_eq!(checkpoints[1].at.as_deref(), Some(checkpoint_time));
        assert_eq!(rewinds[0].at, None);
        assert_eq!(rewinds[1].at.as_deref(), Some(rewind_time));

        let wire = serde_json::to_value(reply).unwrap();
        assert!(wire["checkpoints"][0].get("at").is_none());
        assert_eq!(wire["checkpoints"][1]["at"], checkpoint_time);
        assert!(wire["rewinds"][0].get("at").is_none());
        assert_eq!(wire["rewinds"][1]["at"], rewind_time);
        assert!(wire["checkpoints"][1].get("scope").is_none());
        assert_eq!(wire["checkpoints"][1]["commit"], "snapshot");
        assert_eq!(wire["checkpoints"][1]["head"], "second");
        assert_eq!(wire["checkpoints"][1]["ref"], "refs/worktree/checkpoints/2");
        assert_eq!(wire["rewinds"][0]["rescue_ref"], "rescue1");
        assert_eq!(wire["rewinds"][0]["from_head"], "latest");
        assert_eq!(wire["rewinds"][0]["head"], "initial");
    }

    #[test]
    fn conversation_rewind_has_no_git_metadata() {
        let checkpoint = json!({"id": "t1-start", "phase": "start", "turn": 1,
            "scope": "conversation", "commit": null, "head": null, "ref": null, "leaf": null, "error": null});
        let turn = json!({"kind": "turn", "turn": 1, "checkpoint": "t1-start"});
        let list = json!({"kind": "list", "checkpoints": [checkpoint],
            "rewinds": [{"turn": 1, "checkpoint": "t1-start"}]});
        for wire in [list, turn] {
            let reply: RewindReply = serde_json::from_value(wire.clone()).unwrap();
            assert_eq!(serde_json::to_value(reply).unwrap(), wire);
        }
    }
}
