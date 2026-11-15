//! Inbox list and selected-item descriptors.

use super::{Action, Button, Chip, Notice, QuestionState, Sheet};
use crate::model::Id;
use crate::model::board::ChangeKind;
use crate::model::hue::Hue;
use crate::model::text::Rich;

/// One needs-you row or phone card, with every visible string resolved for the form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NeedYouItem {
    /// Canonical blocker and next move; absent from the illustrative fixture.
    pub attention: Option<super::AttentionLine>,
    /// The row's test id: `inbox-<key>` for served rows, the fixture's own names otherwise.
    pub test_id: String,
    /// Lead phrase, also available separately for accessibility.
    pub lead: String,
    /// Record id.
    pub id: String,
    /// Complete visible lead line, including sample attribution where appropriate.
    pub heading: Rich,
    /// Main title, with the phone plan prefix already included.
    pub title: String,
    /// Full resolved secondary text.
    pub meta: String,
    /// Right-side desktop age; phone ages are already included in heading.
    pub age: String,
    /// Question state, absent on other row kinds.
    pub state: Option<QuestionState>,
    /// Neutralises the owner dot when the item is no longer open.
    pub hue: Hue,
    /// Desktop selection state.
    pub selected: bool,
    /// Explicit sample label, absent for real rows.
    pub sample: Option<&'static str>,
    /// The desktop select or phone open action.
    pub action: Action,
}

/// A changes-feed row: time, id, kind and text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedRow {
    /// Local time of the change.
    pub at: String,
    /// The changed task, or empty for workspace-wide changes.
    pub id: String,
    /// The mark shape.
    pub kind: ChangeKind,
    /// What changed.
    pub text: String,
    /// Test id (`task-<id>`, or `change-<seq>` without a task).
    pub test_id: String,
    /// Unseen styling, absent on phone rows.
    pub unseen: bool,
    /// Open the changed task.
    pub action: Action,
}

/// Empty-list or empty-detail message and its optional navigation action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmptyState {
    /// Message heading.
    pub title: &'static str,
    /// Complete explanatory paragraph.
    pub body: String,
    /// Optional recovery/navigation control.
    pub action: Option<Button>,
}

/// What a loading list, item and question say once the wait is long enough to mention.
pub const LOADING_LIST: &str = "Loading inbox…";
pub const LOADING_ITEM: &str = "Loading this item…";
pub const LOADING_QUESTION: &str = "Loading the question…";

/// Inbox list content, including loading placeholders and stale/error fallback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InboxList {
    /// Loading suppresses both rows and the changes section.
    Loading {
        /// Five desktop list rows or two phone cards.
        skeleton_count: u8,
        /// Placeholder blocks per row/card.
        blocks_per_item: u8,
        /// The status the placeholders announce.
        caption: &'static str,
    },
    /// The projection is usable, including while stale or showing a refresh error.
    Ready {
        /// Desktop listbox accessibility name.
        label: &'static str,
        /// Refresh-error banner above the retained rows.
        error: Option<Notice>,
        /// Needs-you group heading.
        needs_title: &'static str,
        /// Count of rendered rows, including stamped answered questions.
        needs_count: usize,
        /// Needs-you rows/cards in order.
        items: Vec<NeedYouItem>,
        /// Empty-state message when there are no needs-you rows.
        empty: Option<EmptyState>,
        /// Desktop “Since your last visit” or phone “Since 18:40”.
        since_title: String,
        /// Desktop's time of the last visit in the group heading, when known.
        last_visit: Option<String>,
        /// Phone's count of changes since the last visit, independent of the preview length.
        changes_count: u64,
        /// Desktop or phone change rows.
        feed: Vec<FeedRow>,
        /// All-changes navigation control, already labelled for the form.
        all_changes: Button,
    },
}

/// A plan's small inbox preview, not the full plan-review document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanPreview {
    /// Canonical blocker and next move.
    pub attention: Option<super::AttentionLine>,
    /// Ordered needs-you and state chips.
    pub chips: Vec<Chip>,
    /// Plan id and revision.
    pub record: String,
    /// Full plan title.
    pub title: String,
    /// Plan goal, when the projection carries it.
    pub goal: Option<String>,
    /// Heading for the consequences paragraph.
    pub consequence_title: &'static str,
    /// Members, delivery destination and autopilot consequence.
    pub consequence: Rich,
    /// Open the full plan review.
    pub open: Button,
}

/// A served needs-you item that is neither a question, a prompt nor a plan: a review, a
/// stuck task or an escalation without a question record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemPreview {
    /// Canonical blocker and next move.
    pub attention: Option<super::AttentionLine>,
    /// Needs-you and state chips.
    pub chips: Vec<Chip>,
    /// Row id.
    pub record: String,
    /// Row title.
    pub title: String,
    /// Recorded facts, one per line.
    pub facts: Vec<String>,
    /// Open the item's own page, when it has one.
    pub open: Option<Button>,
}

/// The desktop right pane. Phone renders the list and pushes the selected item instead.
#[derive(Debug, Clone, PartialEq)]
pub enum InboxPreview {
    /// Skeleton blocks while the projection or the question's detail loads.
    Loading {
        /// Header/title/body placeholder count.
        skeleton_count: u8,
        /// The status the placeholders announce.
        caption: &'static str,
    },
    /// No item is waiting on the owner.
    Empty(EmptyState),
    /// The selected question's detail did not load.
    Failed(Notice),
    /// Full inline S, B, A, R, Thread, History handover.
    Question(Sheet),
    /// A plan's goal and publication consequence.
    Plan(PlanPreview),
    /// A served assistant prompt: the assistant domain renders its card from the thread.
    Prompt {
        /// The assistant thread (`quick` or `A-n`).
        thread: Id,
        /// The prompt key.
        key: Id,
    },
    /// A served review, stuck task or escalation.
    Item(ItemPreview),
}

/// Complete Inbox destination content without application-shell banners or presentation.
#[derive(Debug, Clone, PartialEq)]
pub struct Inbox {
    /// Phone page title and desktop accessibility title.
    pub title: &'static str,
    /// Whether the phone uses its large title treatment.
    pub large_title: bool,
    /// List or phone cards followed by the changes feed.
    pub list: InboxList,
    /// Desktop/terminal inline preview, absent on phone.
    pub preview: Option<InboxPreview>,
}
