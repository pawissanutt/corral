//! Content descriptors for the list, review document, trackers and phone plan pages. The
//! fixture preview (`fixture.rs`) and corrald's plans (`live.rs`) both build these, so the
//! screens walk one shape whichever source is shown.

use super::{Action, GraphInput, Story};
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
        }
    }

    /// The same command in the quiet utility weight.
    pub fn quiet(mut self) -> Self {
        self.kind = ButtonKind::Quiet;
        self
    }
}

/// A member-strip square's state, preserving landed versus merely neutral waiting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SquareState {
    /// Needs an owner response.
    You,
    /// Landed on the delivery branch.
    Landed,
    /// Corral or a worker is progressing.
    Active,
    /// Held work.
    Held,
    /// Not moving yet.
    Waiting,
}

/// One member square, in member/id order.
#[derive(Debug, Clone, PartialEq)]
pub struct MemberSquare {
    /// Member id for accessibility.
    pub id: Id,
    /// Strip-state treatment.
    pub state: SquareState,
    /// State hue; landed and waiting are distinct neutral treatments.
    pub hue: Hue,
}

/// A list row, with all desktop and phone wording already resolved.
#[derive(Debug, Clone, PartialEq)]
pub struct PlanRow {
    /// Plan, tracker or S-900 id.
    pub id: Id,
    /// Revision retained as data even when the phone omits it from the id label.
    pub rev: Option<String>,
    /// Id plus revision on desktop; id only on phone.
    pub id_label: String,
    /// Full plan title.
    pub title: String,
    /// Actor colour for the row's state.
    pub hue: Hue,
    /// Complete second line, excluding the separately supplied member squares.
    pub sub: Rich,
    /// Executing members' state squares, when their states are known.
    pub members: Vec<MemberSquare>,
    /// Executing landed/total fraction, otherwise absent.
    pub progress: Option<String>,
    /// Neutral selection emphasis.
    pub selected: bool,
    /// Synthetic marker, absent for real plans and phone scale rows.
    pub illustrative: Option<&'static str>,
    /// Desktop selection or phone navigation action.
    pub action: Action,
}

/// One section of the grouped Plans list.
#[derive(Debug, Clone, PartialEq)]
pub struct ListSection {
    /// Group heading.
    pub label: &'static str,
    /// Section count, absent for the phone's Scale test.
    pub count: Option<u32>,
    /// Heading state colour.
    pub hue: Hue,
    /// Phone Scale test's synthetic badge.
    pub illustrative: Option<&'static str>,
    /// Ordered plan rows.
    pub rows: Vec<PlanRow>,
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

/// Plans list content, including the phone's large navigation title.
#[derive(Debug, Clone, PartialEq)]
pub struct PlanList {
    /// Navigation title.
    pub title: &'static str,
    /// Phone uses the large title treatment.
    pub large: bool,
    /// Loading, empty, error or stale state, absent when the rows are current.
    pub notice: Option<ListNotice>,
    /// Plan sections in order; empty sections are omitted for corrald's plans.
    pub sections: Vec<ListSection>,
}

/// A pipeline stage above a document or tracker.
#[derive(Debug, Clone, PartialEq)]
pub struct PipelineStage {
    /// Complete stage label including any progress count.
    pub label: String,
    /// Done, active or next mark.
    pub mark: Mark,
    /// Active stage colour; done and future stages are neutral.
    pub hue: Hue,
}

/// A detail header, resolved for the current idiom.
#[derive(Debug, Clone, PartialEq)]
pub struct Header {
    /// Status chips, in source order.
    pub chips: Vec<Chip>,
    /// Id and optional revision.
    pub id: String,
    /// Full screen title.
    pub title: String,
    /// Acceptance attribution or review timing replay.
    pub meta: Rich,
    /// Explicit illustrative marker, when the source has one.
    pub illustrative: Option<&'static str>,
    /// Desktop commands at the title's end (Ask, document); phone puts Ask in its bar.
    pub actions: Vec<Button>,
    /// The detail is the last copy received and is not current.
    pub stale: Option<String>,
}

/// One proposed-member row in the review document.
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
    /// Desktop or phone actions with their refused state resolved.
    pub buttons: Vec<Button>,
    /// The request-changes field, when open.
    pub compose: Option<Compose>,
    /// Refusal or command status; replaces the attribution when present.
    pub note: Option<DecideNote>,
    /// Exact command and actor/via attribution; empty on phone.
    pub attribution: Rich,
}

/// A revision under review, including the decide bar.
#[derive(Debug, Clone, PartialEq)]
pub struct AwaitingView {
    /// Needs-you/reviewed header.
    pub header: Header,
    /// Design, members, aggregate verification and final acceptance.
    pub pipeline: Vec<PipelineStage>,
    /// Complete document in source order.
    pub sections: Vec<DocumentSection>,
    /// Document artifact attribution; empty when corrald names none.
    pub artifact: Rich,
    /// Actions, field and note.
    pub decide: Decide,
}

/// One final-acceptance blocker.
#[derive(Debug, Clone, PartialEq)]
pub struct Blocker {
    /// Needs-your-answer active mark or not-applicable mark.
    pub mark: Mark,
    /// Full blocker phrase.
    pub text: String,
    /// Needs-you hue only for a blocker that waits on the owner.
    pub hue: Hue,
}

/// Published tracker content and its selected-member inspector.
#[derive(Debug, Clone, PartialEq)]
pub struct ExecutingView {
    /// State, acceptance attribution and commands.
    pub header: Header,
    /// Four plan stages.
    pub pipeline: Vec<PipelineStage>,
    /// Unmapped-scenario warning, absent when mapping is complete or unknown.
    pub warning: Option<String>,
    /// Warning state mark.
    pub warning_mark: Mark,
    /// Members heading.
    pub members_heading: &'static str,
    /// Members, layers and dependencies; the screen lays them out at its measured width.
    pub members: GraphInput,
    /// Blocking-final-acceptance heading.
    pub blockers_heading: &'static str,
    /// Number shown alongside the blockers heading.
    pub blocker_count: usize,
    /// Ordered blockers.
    pub blockers: Vec<Blocker>,
    /// Resolved selected member, with first-member fallback.
    pub selected: Option<Id>,
    /// Desktop/terminal inspector; absent on phone, where members push task pages.
    pub story: Option<Story>,
}

/// An approved plan: its identity, and the state of publication under it.
#[derive(Debug, Clone, PartialEq)]
pub struct ApprovedView {
    /// The plan's state, id and revision and title, carrying any stale-copy notice; absent in
    /// the fixture preview, whose list row beside it names the plan.
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

/// An expanded synthetic member that moves or needs attention.
#[derive(Debug, Clone, PartialEq)]
pub struct ScaleLine {
    /// Synthetic task id.
    pub id: &'static str,
    /// Deterministically generated name.
    pub name: String,
    /// Actor/state dot hue.
    pub hue: Hue,
    /// Needs you, held budget, or gating phrase.
    pub status: &'static str,
    /// Status text hue (moving is dim/neutral).
    pub status_hue: Hue,
}

/// A synthetic layer strip with limited expanded active members.
#[derive(Debug, Clone, PartialEq)]
pub struct ScaleLayerView {
    /// Zero-based layer number.
    pub index: usize,
    /// Layer heading.
    pub label: String,
    /// Every member square in id order, including collapsed landed and waiting members.
    pub squares: Vec<MemberSquare>,
    /// Counts phrase with prior-layer dependency already resolved.
    pub summary: String,
    /// Landed count.
    pub landed: usize,
    /// Moving or needs-you count, including held members.
    pub moving: usize,
    /// Waiting count.
    pub waiting: usize,
    /// At most four lines on desktop and three on phone.
    pub shown: Vec<ScaleLine>,
    /// Remaining expanded members, not remaining total members.
    pub hidden: usize,
    /// “+N more in this layer”, absent when all active lines are shown.
    pub more: Option<String>,
}

/// One scale legend item, keeping the two neutral strip treatments distinguishable.
#[derive(Debug, Clone, PartialEq)]
pub struct LegendEntry {
    /// Visible legend label.
    pub label: &'static str,
    /// Square treatment independent of the label.
    pub state: SquareState,
    /// State hue.
    pub hue: Hue,
}

/// Synthetic 64-member tracker, represented as strips on all forms.
#[derive(Debug, Clone, PartialEq)]
pub struct ScaleView {
    /// Executing header with the explicit synthetic marker.
    pub header: Header,
    /// Total member count.
    pub total: usize,
    /// Landed member count.
    pub landed: usize,
    /// Four plan stages with computed landed/total.
    pub pipeline: Vec<PipelineStage>,
    /// Explanation of strip collapse and square order.
    pub explanation: &'static str,
    /// All six layers.
    pub layers: Vec<ScaleLayerView>,
    /// Legend labels, strip states and hues in source order.
    pub legend: Vec<LegendEntry>,
}

/// Selected detail variant, ready for a renderer to walk.
#[derive(Debug, Clone, PartialEq)]
pub enum Detail {
    /// Revision review and decision bar.
    Awaiting(AwaitingView),
    /// Published tracker and member story.
    Executing(ExecutingView),
    /// Approved, but not yet published.
    Approved(ApprovedView),
    /// The plan's document outside review.
    Document(DocumentView),
    /// The diff between two revisions.
    Diff(DiffView),
    /// Explicitly synthetic 64-member tracker (fixture preview only).
    Scale(ScaleView),
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
    /// Parent list label.
    pub back_label: &'static str,
    /// Back navigation action.
    pub back: crate::model::effect::Effect,
    /// Navigation-bar Ask action, only for executing trackers.
    pub right: Option<Button>,
    /// Accessibility label for the navigation-bar action.
    pub right_label: Option<&'static str>,
    /// Review, executing, approved, document, diff or scale body and decide actions.
    pub detail: Detail,
}

/// Square treatment and hue for the member strips and the legend.
pub fn square(id: Id, state: SquareState) -> MemberSquare {
    MemberSquare {
        id,
        state,
        hue: match state {
            SquareState::You => Hue::You,
            SquareState::Active => Hue::Corral,
            SquareState::Held => Hue::Held,
            SquareState::Landed | SquareState::Waiting => Hue::Neutral,
        },
    }
}

/// The four plan stages: design, members, aggregate verification and final acceptance.
pub fn pipeline(first: &str, members: String, awaiting: bool) -> Vec<PipelineStage> {
    pipeline_marks([
        (
            first.to_owned(),
            if awaiting { Mark::Active } else { Mark::Done },
        ),
        (members, if awaiting { Mark::Next } else { Mark::Active }),
        ("Aggregate verify".into(), Mark::Next),
        ("Final accept".into(), Mark::Next),
    ])
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
