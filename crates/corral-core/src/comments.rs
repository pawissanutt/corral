use jiff::Timestamp;
use serde::{Deserialize, Serialize};

/// Attribution for task context, independent of the task specification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommentAuthor {
    Owner,
    Pm,
}

impl CommentAuthor {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Pm => "pm",
        }
    }
}

/// An append-only note stored in the task's frontmatter, outside its spec body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskComment {
    pub author: CommentAuthor,
    pub at: Timestamp,
    pub text: String,
}
