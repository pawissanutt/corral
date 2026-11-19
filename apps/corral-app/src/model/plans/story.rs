//! Shared selected-member Story descriptors and rich-text helpers.

use super::Button;
use crate::model::Id;
use crate::model::board::Mark;
use crate::model::hue::Hue;
use crate::model::text::{Rich, span};

/// A stage in the selected member's run/gate/review/land stepper.
#[derive(Debug, Clone, PartialEq)]
pub struct StoryStage {
    /// Stage label.
    pub label: &'static str,
    /// Delivery mark.
    pub mark: Mark,
    /// State word, or “waits on you” for the owner's run stage.
    pub status: &'static str,
    /// Run/profile, candidate, prerequisite stage or delivery branch.
    pub sub: String,
    /// Colour of the active state, independent of selection.
    pub hue: Hue,
}

/// A definition-list line in the selected member's story.
#[derive(Debug, Clone, PartialEq)]
pub struct StoryField {
    /// Left-hand field label.
    pub label: &'static str,
    /// Complete styled value, with separators already present.
    pub value: Rich,
    /// Optional action instead of a textual value.
    pub button: Option<Button>,
}

/// The selected member's story: who acts, its four stages, its facts and commands.
#[derive(Debug, Clone, PartialEq)]
pub struct Story {
    /// Selected task id.
    pub id: Id,
    /// The full task title.
    pub title: String,
    /// Optional parent-plan label next to the id.
    pub plan: Option<String>,
    /// The agent working the task, or the member's place in the plan.
    pub context: Rich,
    /// Run, gate, review and land.
    pub stages: Vec<StoryStage>,
    /// Next, You, Needs and Unblocks.
    pub fields: Vec<StoryField>,
    /// Open session, Ask about this and Review candidate.
    pub buttons: Vec<Button>,
}

/// Ids in mono joined by `separator`, or `−` when there are none.
pub(super) fn ids<S: AsRef<str>>(ids: &[S], separator: &str) -> Rich {
    let mut rich = Vec::new();
    for (i, id) in ids.iter().enumerate() {
        if i > 0 {
            rich.push(span(separator));
        }
        rich.push(span(id.as_ref()).mono());
    }
    if rich.is_empty() {
        rich.push(span("−"));
    }
    rich
}

pub(super) fn field(label: &'static str, value: Rich) -> StoryField {
    StoryField {
        label,
        value,
        button: None,
    }
}
