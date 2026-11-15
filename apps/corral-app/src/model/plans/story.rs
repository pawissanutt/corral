//! Shared selected-member Story descriptors and rich-text helpers.

use super::{Button, Chip};
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

/// Fully resolved inspector. Empty stage/step/button lists are the queued-member variant.
#[derive(Debug, Clone, PartialEq)]
pub struct Story {
    /// Selected task id.
    pub id: Id,
    /// The full task title.
    pub title: String,
    /// Optional parent-plan label next to the id.
    pub plan: Option<String>,
    /// Changed-since-last-visit chip, absent for the compact queued story.
    pub changed: Option<Chip>,
    /// Task context, excluding the separate illustrative-machine note.
    pub context: Rich,
    pub context_note: Option<&'static str>,
    /// Delivery stepper, omitted for queued tasks without steps.
    pub stages: Vec<StoryStage>,
    /// Progress heading, absent for the queued-member variant.
    pub progress: Option<&'static str>,
    /// Done/total fraction shown at the progress heading's trailing edge.
    pub progress_count: Option<String>,
    /// Worker progress marks and text.
    pub steps: Vec<(Mark, String)>,
    /// Next, You, Needs, Unblocks and Cost, or the queued task's compact fields.
    pub fields: Vec<StoryField>,
    /// Open session, Ask about this and Review candidate, absent for queued tasks.
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
