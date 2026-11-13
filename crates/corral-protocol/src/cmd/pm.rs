//! Typed pm commands; see the family rules in the parent module.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "verb", rename_all = "kebab-case")]
pub enum PmCommand {
    GuardResume {
        json: bool,
    },
    Present {
        decision: String,
        delivery: String,
        confirmed: bool,
    },
    Resolve {
        decision: String,
        answer: String,
    },
    InboxPost {
        kind: String,
        subject: String,
        body: String,
        refs: Vec<String>,
        action: Option<String>,
        quote: Option<String>,
        input: Option<String>,
        key: Option<String>,
        json: bool,
    },
    InboxClaim {
        id: String,
        expected_revision: u32,
        json: bool,
    },
    InboxResolve {
        id: String,
        expected_revision: u32,
        outcome: String,
        text: String,
        refs: Vec<String>,
        json: bool,
    },
    InboxWithdraw {
        id: String,
        expected_revision: u32,
        reason: String,
        json: bool,
    },
    NotesAdd {
        kind: String,
        text: String,
        provenance: Option<String>,
        reason: Option<String>,
        resume_when: Option<String>,
        expect_revision: Option<u64>,
        json: bool,
    },
    NotesSupersede {
        id: String,
        text: String,
        provenance: Option<String>,
        reason: Option<String>,
        resume_when: Option<String>,
        expect_revision: Option<u64>,
        json: bool,
    },
    NotesRemove {
        id: String,
        expect_revision: Option<u64>,
        json: bool,
    },
    #[serde(other)]
    Other,
}

impl PmCommand {
    pub fn verb(&self) -> &'static str {
        match self {
            Self::GuardResume { .. } => "guard-resume",
            Self::Present { .. } => "present",
            Self::Resolve { .. } => "resolve",
            Self::InboxPost { .. } => "inbox-post",
            Self::InboxClaim { .. } => "inbox-claim",
            Self::InboxResolve { .. } => "inbox-resolve",
            Self::InboxWithdraw { .. } => "inbox-withdraw",
            Self::NotesAdd { .. } => "notes-add",
            Self::NotesSupersede { .. } => "notes-supersede",
            Self::NotesRemove { .. } => "notes-remove",
            Self::Other => "other",
        }
    }
}
