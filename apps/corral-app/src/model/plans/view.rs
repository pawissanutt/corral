//! Content descriptors for a served plan's detail: its head, its document, the brief beside the
//! member story, and the phone page.

use super::{Action, PlanBrief, Story};
use crate::model::Id;
use crate::model::hue::Hue;
use crate::model::text::Rich;

/// A labelled state chip; selection never changes its hue.
#[derive(Debug, Clone, PartialEq)]
pub struct Chip {
    /// Complete chip text.
    pub label: String,
    /// State colour.
    pub hue: Hue,
}

/// Button emphasis from the source template, not renderer-specific styling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonKind {
    /// Normal command button.
    Standard,
    /// Secondary command.
    Ghost,
    /// Quiet utility action.
    Quiet,
}

/// One press target. Engine operations are never disabled: the engine decides.
#[derive(Debug, Clone, PartialEq)]
pub struct Button {
    /// Visible action label.
    pub label: String,
    /// Domain reducer input.
    pub action: Action,
    /// Command emphasis.
    pub kind: ButtonKind,
    /// What pressing it does; empty when the label says enough.
    pub hint: String,
}

impl Button {
    /// A secondary command descriptor, useful in the selected-member story too.
    pub fn new(label: impl Into<String>, action: Action) -> Self {
        Self {
            label: label.into(),
            action,
            kind: ButtonKind::Ghost,
            hint: String::new(),
        }
    }

    /// The same command in the quiet utility weight.
    pub fn quiet(mut self) -> Self {
        self.kind = ButtonKind::Quiet;
        self
    }

    /// The same command with the sentence behind it.
    pub fn hinted(mut self, hint: impl Into<String>) -> Self {
        self.hint = hint.into();
        self
    }
}

/// What the list says instead of, or above, its rows.
#[derive(Debug, Clone, PartialEq)]
pub enum ListNotice {
    /// The plans topic has not delivered its first snapshot.
    Loading,
    /// A complete snapshot holds no plan.
    Empty {
        heading: &'static str,
        body: &'static str,
    },
    /// The plans topic failed before any row arrived.
    Error(String),
    /// Rows are the last ones received; the list is not current.
    Stale(String),
}

/// A detail header, resolved for the current idiom.
#[derive(Debug, Clone, PartialEq)]
pub struct Header {
    /// The plan's parent task, whose Actions menu the head carries.
    pub plan: Id,
    /// Status chips, in source order.
    pub chips: Vec<Chip>,
    pub id: String,
    /// The project corrald files the plan's task under, when it names one.
    pub project: Option<String>,
    /// Full screen title.
    pub title: String,
    /// When the plan's task last changed; the state chip's hint.
    pub meta: Rich,
    /// Desktop commands at the head (Ask); the phone puts Ask in its bar.
    pub actions: Vec<Button>,
    /// The detail is the last copy received and is not current.
    pub stale: Option<String>,
}

/// A served plan: its head, the brief with its member graph and path to done, its document
/// (the parent task's body, Markdown), and the story of a member selected with J/K or a card
/// press.
#[derive(Debug, Clone, PartialEq)]
pub struct PlanView {
    pub header: Header,
    pub brief: PlanBrief,
    /// The parent task's body as written; empty when it has none.
    pub document: String,
    /// On the phone, a long document shows its opening and this toggle; absent elsewhere and
    /// for a short one.
    pub document_fold: Option<DocumentFold>,
    /// Members heading and what its layout means.
    pub members_heading: &'static str,
    pub members_hint: &'static str,
    /// What stands in for the graph while the plan has no member.
    pub members_empty: &'static str,
    /// The member the owner selected, if any.
    pub selected: Option<Id>,
    /// The selected member's story; absent on phone, where members push task pages.
    pub story: Option<Story>,
}

/// A long document on the phone: its opening, cut at a paragraph and never inside a code
/// fence, and the press that shows the rest or folds it again.
#[derive(Debug, Clone, PartialEq)]
pub struct DocumentFold {
    pub preview: String,
    /// Whether the whole document shows.
    pub open: bool,
    pub toggle: Button,
}

/// What the detail says when there is no plan content to show.
#[derive(Debug, Clone, PartialEq)]
pub struct NoticeView {
    pub heading: String,
    pub body: String,
    /// Failure tone for errors, neutral otherwise.
    pub hue: Hue,
    /// Retry the request, for errors.
    pub action: Option<Button>,
}

/// Selected detail variant, ready for a renderer to walk.
#[derive(Debug, Clone, PartialEq)]
pub enum Detail {
    Plan(Box<PlanView>),
    /// corrald has not delivered the plan yet.
    Loading,
    /// No plan, an unserved plan, or a failed request.
    Notice(NoticeView),
}

/// Phone plan-page navigation metadata plus its complete detail content.
#[derive(Debug, Clone, PartialEq)]
pub struct PhonePage {
    /// The plan's id.
    pub title: String,
    /// Navigation-bar Ask action.
    pub right: Button,
    /// Accessibility label for the navigation-bar action.
    pub right_label: &'static str,
    pub detail: Detail,
}
