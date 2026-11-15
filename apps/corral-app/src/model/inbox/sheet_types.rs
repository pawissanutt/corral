//! Handover descriptors shared by fixture and served question builders.
use super::{Action, QuestionState, WaitKind};
use crate::model::Id;
use crate::model::board::LineKind;
use crate::model::hue::Hue;
use crate::model::text::Rich;
/// A labelled state chip in a header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chip {
    /// Verbatim chip text.
    pub label: &'static str,
    /// State colour, never selection colour.
    pub hue: Hue,
}

/// The source's three button emphasis levels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonKind {
    /// Main action.
    Primary,
    /// Secondary outlined action.
    Ghost,
    /// Low-emphasis text action.
    Quiet,
}

/// A fully labelled control; dispatch its action rather than interpreting its text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Button {
    /// Visible text, already resolved for form and state.
    pub label: String,
    /// Reducer action.
    pub action: Action,
    /// Whether the renderer accepts activation.
    pub enabled: bool,
    /// Source emphasis.
    pub kind: ButtonKind,
    /// Display-only shortcut hint, when present.
    pub shortcut: Option<&'static str>,
}
/// A stamped answer or a recoverable error with all inline attribution preserved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    /// Optional uppercase stamp.
    pub stamp: Option<&'static str>,
    /// State colour of the notice.
    pub hue: Hue,
    /// Complete notice body, including bold/mono runs.
    pub body: Rich,
    /// Retry control, when the source offers one.
    pub retry: Option<Button>,
}

/// Question title and its complete metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    /// Ordered chips; phone uses owner-only where desktop has the kind chip.
    pub chips: Vec<Chip>,
    /// Mono question key and revision.
    pub record: String,
    /// Asker, escalation and age line.
    pub attribution: String,
    /// Full title.
    pub title: String,
}

/// One dependency in “Waiting on this answer”.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WaitLine {
    /// Task or plan id.
    pub id: String,
    /// Dependency explanation.
    pub text: String,
    /// The mark and emphasis of this dependency.
    pub kind: WaitKind,
}

/// S: the decision, affected work and optional immutable answer stamp.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Situation {
    /// Canonical blocker and next move, not an inferred task state.
    pub attention: Option<super::AttentionLine>,
    /// The complete question.
    pub text: String,
    /// The recorded deadline and what happens at it, when there is one.
    pub deadline: Option<Rich>,
    /// Dependency-list heading.
    pub waits_title: &'static str,
    /// Waiting tasks, including explicitly independent ones.
    pub waits: Vec<WaitLine>,
    /// Shown under the heading when nothing is recorded as waiting.
    pub waits_empty: Option<&'static str>,
    /// Answered, answered-elsewhere, pending, refused or queued stamp, absent when open.
    pub notice: Option<Notice>,
}

/// A card in the miniature plan graph, in source SVG units.
#[derive(Debug, Clone, PartialEq)]
pub struct MiniNode {
    /// Task id displayed in the card.
    pub id: &'static str,
    /// Card subtitle.
    pub subtitle: &'static str,
    /// Upper-left position in source units.
    pub position: (f32, f32),
    /// Card dimensions in source units.
    pub size: (f32, f32),
    /// State colour of the node.
    pub hue: Hue,
    /// Whether the source dims the queued node.
    pub dim: bool,
}

/// A miniature dependency edge, preserving the source's line or cubic geometry.
#[derive(Debug, Clone, PartialEq)]
pub enum MiniEdge {
    /// Horizontal segment with its endpoints.
    Line { from: (f32, f32), to: (f32, f32) },
    /// Cubic segment with two controls and endpoints.
    Cubic {
        from: (f32, f32),
        control1: (f32, f32),
        control2: (f32, f32),
        to: (f32, f32),
    },
}

/// The position-in-plan graph, with its accessibility name and caption.
#[derive(Debug, Clone, PartialEq)]
pub struct MiniGraph {
    /// Source view-box width and height.
    pub size: (f32, f32),
    /// Full textual alternative.
    pub label: &'static str,
    /// Nodes in source order.
    pub nodes: Vec<MiniNode>,
    /// Dependencies in source order.
    pub edges: Vec<MiniEdge>,
    /// Plan, layer, member and final-accept line.
    pub caption: &'static str,
}

/// B: lazy task detail or the task's goal and position in the plan.
#[derive(Debug, Clone, PartialEq)]
pub enum Background {
    /// Task detail is independent of the board-projected question.
    Loading {
        /// Number of skeleton blocks.
        skeleton_count: u8,
        /// Explanation below the placeholders.
        note: &'static str,
    },
    /// Loaded task and plan context.
    Ready {
        /// Bold task title followed by the task goal.
        goal: Rich,
        /// Plan, governing revision, run and step metadata.
        plan_line: Rich,
        /// Source miniature graph, never inferred by the UI; absent when none was served.
        graph: Option<MiniGraph>,
    },
    /// Canonical plan members, laid out for the actual form with the asking task selected.
    Plan {
        goal: Rich,
        plan_line: Rich,
        graph: crate::model::plans::GraphInput,
        selected: Option<Id>,
    },
    /// Nothing about the asking task or plan is recorded.
    Absent {
        /// What is missing.
        note: &'static str,
    },
}

/// A labelled evidence-tab control.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvidenceTab {
    /// The tab's label.
    pub label: String,
    /// Its test id (`seg-evidence-<name>`).
    pub test_id: String,
    /// Its pressed state.
    pub selected: bool,
    /// Reducer action selecting the tab.
    pub action: Action,
}

/// A numbered patch line; absent numbers are intentionally blank cells.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    /// Old line number, absent for additions.
    pub old: Option<u32>,
    /// New line number, absent for deletions.
    pub new: Option<u32>,
    /// Addition, deletion or context mark.
    pub kind: LineKind,
    /// Prefix cell; context uses an empty prefix.
    pub prefix: &'static str,
    /// Source code text.
    pub text: String,
}

/// The selected evidence contents, including file titles and real/illustrative labels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvidenceBody {
    /// A patch hunk and its explanation.
    Diff {
        /// File path.
        file: String,
        /// Hunk header, empty when the excerpt carries none.
        at: String,
        /// Display-ready line numbering.
        lines: Vec<DiffLine>,
        /// Explanation below the hunk.
        note: Rich,
    },
    /// Interface excerpt or log tail.
    Text {
        /// File title.
        file: String,
        /// Illustrative attribution, if present.
        attribution: Option<&'static str>,
        /// Styled content lines.
        lines: Vec<Rich>,
        /// Truncation or provenance line under the box.
        note: Option<Rich>,
    },
    /// Rendered illustrative terminal screenshot.
    Shot {
        /// Real artifact title and rendered suffix.
        file: &'static str,
        /// Explicit illustrative label.
        attribution: &'static str,
        /// Styled terminal lines, preserving blank lines.
        lines: Vec<Rich>,
        /// Whether the terminal ends with a cursor after the final prompt.
        cursor: bool,
        /// Caption beneath the screenshot.
        note: &'static str,
    },
    /// Canonical raster artifact, loaded only through the daemon detail reader.
    Image {
        file: String,
        preview: crate::model::assistant::Readback,
    },
    /// A reference corrald could not resolve, with its reason.
    Unresolved {
        /// Reference title.
        file: String,
        /// Why it is not shown.
        reason: String,
    },
}

/// A: evidence is independently loading, failed, absent or ready with its tabs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Assessment {
    /// No evidence tabs are shown while the content streams in.
    Loading {
        /// Number of evidence skeleton blocks.
        skeleton_count: u8,
        /// Explanatory copy.
        note: &'static str,
    },
    /// The projected options remain current despite an artifact error.
    Error(Notice),
    /// The asker attached nothing.
    Empty {
        /// Explanatory copy.
        note: &'static str,
    },
    /// Loaded evidence toolbar and selected body.
    Ready {
        /// Toolbar accessibility name.
        label: &'static str,
        /// Ordered evidence tabs.
        tabs: Vec<EvidenceTab>,
        /// The selected body.
        body: EvidenceBody,
    },
}

/// One rendered answer radio, its consequences and its evidence link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnswerOption {
    /// One-based number printed on the option.
    pub number: usize,
    /// Complete option label.
    pub label: String,
    /// Asker-recommendation chip, absent on the other options.
    pub recommendation_chip: Option<String>,
    /// Whether this option is selected, including a winning answer from elsewhere.
    pub selected: bool,
    /// Whether the radio is interactive.
    pub enabled: bool,
    /// Choose this option.
    pub action: Action,
    /// Then, Unblocks, Costs: each label with its value, only those that are recorded.
    pub consequences: Vec<(&'static str, String)>,
    /// “see” evidence link with the correct tab action, when the option cites evidence.
    pub proof: Option<Button>,
}

/// R: options, free-answer field and recommendation explanation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recommendation {
    /// Radiogroup accessibility name.
    pub label: &'static str,
    /// Every numbered option.
    pub options: Vec<AnswerOption>,
    /// Whether this answer is read-only.
    pub locked: bool,
    /// Pencil glyph on the alternate-answer row.
    pub draft_mark: &'static str,
    /// Alternate-answer placeholder.
    pub draft_placeholder: &'static str,
    /// The live question whose alternate answer the field edits; None in the fixture.
    pub draft_key: Option<Id>,
    /// Bold lead-in and complete recommendation reason, when the asker gave one.
    pub reason: Option<Rich>,
}

/// One follow-up in the thread with the asker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadLine {
    /// Speaker shown in bold.
    pub who: String,
    /// Device or surface attribution, when present.
    pub via: Option<String>,
    /// Message time.
    pub at: String,
    /// Message body.
    pub text: String,
}

/// Thread content shared by the inline band and phone sheet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadView {
    /// Phone sheet title.
    pub title: String,
    /// Messages, oldest first.
    pub messages: Vec<ThreadLine>,
    /// Shown instead of messages when there are none.
    pub empty: Option<&'static str>,
    /// Reply textarea placeholder.
    pub reply_placeholder: String,
    /// Textarea accessibility name.
    pub reply_label: &'static str,
    /// The live question whose follow-up the field edits; None in the fixture.
    pub reply_key: Option<Id>,
    /// Whether the reply field accepts typing.
    pub reply_enabled: bool,
    /// Send control.
    pub send: Button,
    /// Illustrative attribution beneath the reply, fixture only.
    pub attribution: Option<&'static str>,
    /// The follow-up's pending, refused or unknown state.
    pub status: Option<Notice>,
    /// Done control for dismissing the sheet.
    pub done: Button,
}

/// One history row with its owner-decision versus attempt label resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryRow {
    /// Complete date and kind label.
    pub label: String,
    /// Whether this row uses the owner-decision mark.
    pub owner_decision: bool,
    /// The previous decision or attempt.
    pub text: String,
}

/// The source's SBAR, Thread and History band identities.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BandKind {
    /// S.
    Situation,
    /// B.
    Background,
    /// A.
    Assessment,
    /// R.
    Recommendation,
    /// T.
    Thread,
    /// H.
    History,
}

/// Ordered band metadata, sufficient to render the correct headings in each form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Band {
    /// Which typed sheet payload to render.
    pub kind: BandKind,
    /// Single-letter left-margin label.
    pub letter: &'static str,
    /// Complete heading.
    pub title: &'static str,
    /// Right-side context or count, when the source has one.
    pub detail: Option<String>,
}

/// Sticky answer controls in all states; answered stamps replace the answer controls in place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecideBar {
    /// Ordered controls for the current form and answer state.
    pub buttons: Vec<Button>,
    /// Desktop's answered stamp and its hue.
    pub stamp: Option<(String, Hue)>,
    /// Command attribution, connection rule, or one-answer-wins explanation.
    pub note: Rich,
    /// What Ctrl+Enter runs: the answer control's action while it is enabled.
    pub submit: Option<Action>,
}

/// A complete handover with payloads separate from source-defined band order.
#[derive(Debug, Clone, PartialEq)]
pub struct Sheet {
    /// Screen title used by the phone navigation bar.
    pub title: &'static str,
    /// Header metadata and question title.
    pub header: Header,
    /// Effective answer state, including the elsewhere override.
    pub state: QuestionState,
    /// Whether the sheet carries answered/read-only presentation.
    pub answered: bool,
    /// Main bands, in actual render order.
    pub bands: Vec<Band>,
    /// Focus-mode context bands, empty for inline desktop and phone.
    pub side_bands: Vec<Band>,
    /// S payload.
    pub situation: Situation,
    /// B payload.
    pub background: Background,
    /// A payload.
    pub assessment: Assessment,
    /// R payload.
    pub recommendation: Recommendation,
    /// T payload, full inline on desktop or in the phone sheet.
    pub thread: ThreadView,
    /// Phone's collapsed thread preview.
    pub thread_preview: Rich,
    /// Open the phone thread preview.
    pub thread_action: Action,
    /// Whether to show the phone thread sheet.
    pub thread_open: bool,
    /// H payload.
    pub history: Vec<HistoryRow>,
    /// Shown instead of history rows when none are recorded.
    pub history_empty: Option<&'static str>,
    /// Pinned decision controls.
    pub decide: DecideBar,
    /// Focus-page back control, absent inline.
    pub back: Option<Button>,
    /// Pager wording, absent inline.
    pub pager: Option<String>,
    /// Next-item control, absent inline or when nothing else needs you.
    pub next: Option<Button>,
    /// Focus desktop shortcut help, absent inline and on phone.
    pub key_help: Option<Rich>,
    /// Ask about this question, absent inline.
    pub ask: Option<Button>,
}
