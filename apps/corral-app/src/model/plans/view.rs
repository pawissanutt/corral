//! Content descriptors for a served plan's detail: its head, document, decide bar, the executing
//! brief beside the member story, and the phone page.

use super::{Action, AwaitingBrief, ExecutingBrief, Story};
use crate::model::Id;
use crate::model::board::Mark;
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

/// One complete press target, including its refused state.
#[derive(Debug, Clone, PartialEq)]
pub struct Button {
    /// Visible action label.
    pub label: String,
    /// Domain reducer input.
    pub action: Action,
    /// False while the command is refused (offline, stale, not acceptable, in flight).
    pub enabled: bool,
    /// The decide bar's primary action.
    pub primary: bool,
    /// Command emphasis.
    pub kind: ButtonKind,
    /// What pressing it does, or why it is refused; empty when the label says enough.
    pub hint: String,
}

impl Button {
    /// A secondary command descriptor, useful in the selected-member story too.
    pub fn new(label: impl Into<String>, action: Action) -> Self {
        Self {
            label: label.into(),
            action,
            enabled: true,
            primary: false,
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

/// A pipeline stage above a plan that is still being designed.
#[derive(Debug, Clone, PartialEq)]
pub struct PipelineStage {
    /// Complete stage label.
    pub label: String,
    /// Done, active or next mark.
    pub mark: Mark,
    /// Active stage colour; done and future stages are neutral.
    pub hue: Hue,
}

/// A detail header, resolved for the current idiom.
#[derive(Debug, Clone, PartialEq)]
pub struct Header {
    /// Canonical plan or tracker identity for record actions, when available.
    pub plan: Option<Id>,
    /// Status chips, in source order.
    pub chips: Vec<Chip>,
    /// Id and optional revision.
    pub id: String,
    /// The projects corrald files the plan under, when it names any.
    pub project: Option<String>,
    /// Full screen title.
    pub title: String,
    /// Acceptance attribution or review timing replay; the state chip's hint.
    pub meta: Rich,
    /// Explicit illustrative marker, when the source has one.
    pub illustrative: Option<&'static str>,
    /// Desktop commands at the head (Ask, document); phone puts Ask in its bar.
    pub actions: Vec<Button>,
    /// The detail is the last copy received and is not current.
    pub stale: Option<String>,
}

/// One proposed-member row in a plan document.
#[derive(Debug, Clone, PartialEq)]
pub struct ProposedRow {
    /// Untouched member mark.
    pub mark: Mark,
    /// Proposed name, not a published task id.
    pub name: String,
    /// “can start now” or “after <name>”.
    pub sub: String,
}

/// A review verdict with supersession already resolved.
#[derive(Debug, Clone, PartialEq)]
pub struct ReviewRow {
    /// Reviewed revision.
    pub rev: String,
    /// Original verdict mark for revision-aware inspection.
    pub original_mark: Mark,
    /// Stale for a superseded revision, otherwise the original mark.
    pub mark: Mark,
    /// Verdict plus superseded aside, with styles preserved.
    pub verdict: Rich,
    /// Reviewer, time and summary with dim attribution.
    pub note: Rich,
    /// The subdued superseded row treatment.
    pub superseded: bool,
}

/// Renderable document-section content; the renderer does not branch on domain state.
#[derive(Debug, Clone, PartialEq)]
pub enum SectionBody {
    /// One complete paragraph.
    Paragraph(Rich),
    /// Plain bullet list.
    Bullets(Vec<String>),
    /// Proposed unpublished members.
    Proposed(Vec<ProposedRow>),
    /// Revision-bound review history.
    Reviews(Vec<ReviewRow>),
    /// Owner decisions the acceptance must resolve, each with the owner's resolution field.
    Decisions(Vec<DecisionRow>),
}

/// One owner decision and the owner's resolution as typed; acceptance records it as
/// `Resolved: <decision> => <resolution>`.
#[derive(Debug, Clone, PartialEq)]
pub struct DecisionRow {
    /// The plan the decision belongs to.
    pub plan: Id,
    /// The decision exactly as corrald states it.
    pub decision: String,
    /// The owner's resolution so far.
    pub resolution: String,
    /// Read-only while the acceptance is in flight.
    pub locked: bool,
}

/// Heading and content for one section of a plan document.
#[derive(Debug, Clone, PartialEq)]
pub struct DocumentSection {
    /// Section heading.
    pub heading: &'static str,
    /// Paragraph, list, proposed members or reviews.
    pub body: SectionBody,
}

/// The decide note: a refusal, a command's status, or nothing.
#[derive(Debug, Clone, PartialEq)]
pub struct DecideNote {
    /// The complete sentence.
    pub text: String,
    /// Failure for refusals and failed commands, held for unknown results, ok when done.
    pub hue: Hue,
}

/// The request-changes note being written, bound to the revision it answers.
#[derive(Debug, Clone, PartialEq)]
pub struct Compose {
    /// The plan the note is for.
    pub plan: Id,
    /// The owner's words so far.
    pub text: String,
    /// Field label and placeholder.
    pub placeholder: String,
    /// Read-only while the note is in flight.
    pub locked: bool,
    /// Send and Cancel.
    pub send: Button,
    pub cancel: Button,
}

/// The decide bar: actions, the request-changes field, and its note or attribution.
#[derive(Debug, Clone, PartialEq)]
pub struct Decide {
    /// Accept plan, Request changes, Ask, then the diff and any result check, each with its
    /// hint; refused ones stay, disabled, with the refusal as their hint.
    pub buttons: Vec<Button>,
    /// The request-changes field, when open.
    pub compose: Option<Compose>,
    /// Refusal or command status; replaces the attribution when present.
    pub note: Option<DecideNote>,
    /// Exact command and actor/via attribution; empty on phone.
    pub attribution: Rich,
}

/// A revision under review: its head, the document to read, and what accepting publishes,
/// what the reviewers said and the decide bar.
#[derive(Debug, Clone, PartialEq)]
pub struct AwaitingView {
    /// Needs-you/reviewed header.
    pub header: Header,
    /// The document in source order, without the proposed members and verdicts, which the
    /// brief states.
    pub sections: Vec<DocumentSection>,
    pub brief: AwaitingBrief,
}

/// An executing (or finished) published plan: its head, the brief with its member graph and
/// path to done, and the story of a member selected with J/K or a card press.
#[derive(Debug, Clone, PartialEq)]
pub struct ExecutingView {
    /// State, acceptance attribution and commands.
    pub header: Header,
    pub brief: ExecutingBrief,
    /// Members heading and what its layout means.
    pub members_heading: &'static str,
    pub members_hint: &'static str,
    /// What stands in for the graph while the detail lists no member.
    pub members_empty: &'static str,
    /// The member the owner selected, if any.
    pub selected: Option<Id>,
    /// The selected member's story; absent on phone, where members push task pages.
    pub story: Option<Story>,
}

/// Planning and review before an owner acceptance request exists.
#[derive(Debug, Clone, PartialEq)]
pub struct PlanningView {
    pub header: Header,
    pub heading: String,
    pub body: String,
    pub pipeline: Vec<PipelineStage>,
}

/// An approved plan: its identity, and the state of publication under it.
#[derive(Debug, Clone, PartialEq)]
pub struct ApprovedView {
    /// The plan's state, identity, revision and title, with any stale-copy notice.
    pub header: Option<Header>,
    /// Desktop and phone use different empty-state headings.
    pub heading: String,
    /// The idiom-specific explanation.
    pub body: String,
    /// Opens the published tracker and the approved document when corrald serves them.
    pub actions: Vec<Button>,
    /// The detail is the last copy received and is not current (when there is no header).
    pub stale: Option<String>,
}

/// A plan's document outside review: the executing or approved plan's accepted text.
#[derive(Debug, Clone, PartialEq)]
pub struct DocumentView {
    /// Header with the way back to the tracker.
    pub header: Header,
    /// Document sections in source order.
    pub sections: Vec<DocumentSection>,
}

/// One line of a revision diff.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffLineKind {
    /// Added in the newer revision.
    Added,
    /// Removed from the older revision.
    Removed,
    /// A hunk or file header.
    Header,
    /// Unchanged context.
    Context,
}

/// A diff line, as received.
#[derive(Debug, Clone, PartialEq)]
pub struct DiffLine {
    pub text: String,
    pub kind: DiffLineKind,
}

/// The revision diff between two plan revisions, paged as corrald bounds it.
#[derive(Debug, Clone, PartialEq)]
pub struct DiffView {
    /// "revision diff" header with the way back to the document.
    pub header: Header,
    /// Lines of every page received so far.
    pub lines: Vec<DiffLine>,
    /// Loading, error, empty or truncation sentence.
    pub note: Option<DecideNote>,
    /// "Show more" when corrald holds another page.
    pub more: Option<Button>,
    /// The awaiting revision keeps its decide bar beside the diff.
    pub decide: Option<Decide>,
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
    /// Revision review and decision bar.
    Awaiting(AwaitingView),
    /// Drafting, independent review or a planning failure, never an owner decide bar.
    Planning(PlanningView),
    /// Published tracker: brief, members and path to done.
    Executing(ExecutingView),
    /// Approved, but not yet published.
    Approved(ApprovedView),
    /// The plan's document outside review.
    Document(DocumentView),
    /// The diff between two revisions.
    Diff(DiffView),
    /// corrald has not delivered the plan yet.
    Loading,
    /// No plan, an unserved plan, or a failed request.
    Notice(NoticeView),
}

/// Phone plan-page navigation metadata plus its complete detail content.
#[derive(Debug, Clone, PartialEq)]
pub struct PhonePage {
    /// Plan review, tracker id or S-900.
    pub title: String,
    /// Navigation-bar Ask action, only for executing trackers.
    pub right: Option<Button>,
    /// Accessibility label for the navigation-bar action.
    pub right_label: Option<&'static str>,
    /// Review, executing, approved, document, diff or scale body and decide actions.
    pub detail: Detail,
}

/// Stages with explicit marks; the active one takes the Corral colour.
pub fn pipeline_marks(stages: [(String, Mark); 4]) -> Vec<PipelineStage> {
    stages
        .into_iter()
        .map(|(label, mark)| PipelineStage {
            label,
            mark,
            hue: if mark == Mark::Active {
                Hue::Corral
            } else {
                Hue::Neutral
            },
        })
        .collect()
}
