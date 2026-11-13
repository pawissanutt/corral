//! Task commands shared with the classic board; see the family rules in the parent module.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "verb", rename_all = "kebab-case")]
pub enum TaskCommand {
    New {
        title: Vec<String>,
        origin: String,
        project: Option<String>,
        status: String,
        body: String,
        profile: Option<String>,
        depends: Vec<String>,
        priority: Option<String>,
        kind: Option<String>,
        size: Option<String>,
        difficulty: Option<String>,
        design: bool,
        labels: Vec<String>,
    },
    Comment {
        id: String,
        text: String,
        author: String,
        send: bool,
    },
    Move {
        id: String,
        status: String,
        force: bool,
        owner_ok: bool,
        reason: Option<String>,
    },
    Idea {
        id: String,
        action: String,
        reason: String,
        into: Option<String>,
        body: Option<String>,
        owner_ok: bool,
    },
    Route {
        id: String,
        set_reviewer: Option<String>,
        clear_reviewer: bool,
        profile: Option<String>,
        model: Option<String>,
        effort: Option<String>,
        json: bool,
    },
    Hold {
        id: String,
        reason: Option<String>,
    },
    Release {
        id: String,
    },
    WaiveReview {
        id: String,
        reason: String,
    },
    Focus {
        id: Option<String>,
        all: bool,
    },
    RecoverInfrastructure {
        id: String,
    },
    #[serde(other)]
    Other,
}

impl TaskCommand {
    pub fn verb(&self) -> &'static str {
        match self {
            Self::New { .. } => "new",
            Self::Comment { .. } => "comment",
            Self::Move { .. } => "move",
            Self::Idea { .. } => "idea",
            Self::Route { .. } => "route",
            Self::Hold { .. } => "hold",
            Self::Release { .. } => "release",
            Self::WaiveReview { .. } => "waive-review",
            Self::Focus { .. } => "focus",
            Self::RecoverInfrastructure { .. } => "recover-infrastructure",
            Self::Other => "other",
        }
    }
}
