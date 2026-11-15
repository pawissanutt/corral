//! Typed content for Review renderers, shared by the fixture replay and the served candidate.
//! No widgets, dimensions or styling APIs live here.

use crate::model::board::Mark;
use crate::model::effect::{Conn, Env};
use crate::model::hue::Hue;
use crate::model::nav::Form;
use crate::model::text::{Rich, span};

use super::data::Screenshot;
use super::diff::{Anchor, split_rows, unified_rows};
use super::{Action, Compose, Layout, State, Tab};

/// A neutral or state-coloured chip whose copy is already resolved for the form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chip {
    /// Chip text, without extra punctuation added by a renderer.
    pub label: String,
    /// Semantic state colour.
    pub hue: Hue,
}

pub(super) fn chip(label: impl Into<String>, hue: Hue) -> Chip {
    Chip {
        label: label.into(),
        hue,
    }
}

/// Button emphasis from the source, not a platform-specific layout instruction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonStyle {
    /// Main decision.
    Primary,
    /// Secondary outlined decision or collapsed-file action.
    Ghost,
    /// Quiet inline action.
    Quiet,
}

/// A labelled intent; a refused control remains pressable so its reducer can explain itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Button {
    /// Visible label.
    pub label: String,
    /// Reducer intent.
    pub action: Action,
    /// Whether the control can be activated at all.
    pub enabled: bool,
    /// Source emphasis.
    pub style: ButtonStyle,
    /// Why activating it will not run the command, shown beside the control.
    pub refusal: Option<String>,
}

pub(super) fn button(label: impl Into<String>, action: Action, style: ButtonStyle) -> Button {
    Button {
        label: label.into(),
        action,
        enabled: true,
        style,
        refusal: None,
    }
}

/// One candidate-list row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateRow {
    /// Task id, the row's stable key and test id.
    pub task: String,
    /// Short title below the identity.
    pub title: String,
    /// Stage line below the title.
    pub state: String,
    /// Actor mark colour, independent of selection.
    pub hue: Hue,
    /// Formatted task id and candidate identity.
    pub identity: Rich,
    /// Whether this is the candidate shown beside the list.
    pub selected: bool,
    /// Intent for the whole row.
    pub action: Action,
}

/// Candidate group heading and its rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateGroup {
    /// `Your verdict` or `Verifying`.
    pub label: &'static str,
    /// Group count displayed alongside the heading.
    pub count: u32,
    /// Heading's semantic state colour.
    pub hue: Hue,
    /// Rows in order.
    pub rows: Vec<CandidateRow>,
}

/// The selected candidate's title, metadata and tags.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    /// Candidate title.
    pub title: String,
    /// Form-specific metadata, including desktop base and totals.
    pub metadata: Rich,
    /// Replay, review state and freshness tags.
    pub chips: Vec<Chip>,
}

/// A verdict's complete presentation, including stale mark, strike and older-candidate tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerdictView {
    /// Display mark; stale overrides the original mark.
    pub mark: Mark,
    /// Who supplied the verdict, in bold.
    pub who: String,
    /// Candidate SHA with stale hue already applied.
    pub sha: Rich,
    /// Clock time, or empty when none was recorded.
    pub time: String,
    /// Phone attribution line: bold actor, monospace SHA and optional clock time.
    pub line: Rich,
    /// Result text with strike/dim/owner hue resolved.
    pub text: Rich,
    /// Older-candidate tag, including its form-specific wording.
    pub tag: Option<Chip>,
}

/// A tab button in the desktop tab list or phone segmented list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabView {
    pub tab: Tab,
    /// Form-specific label.
    pub label: &'static str,
    pub selected: bool,
    pub action: Action,
}

/// A layout preference button, shown only alongside the desktop Diff tab.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayoutChoice {
    pub layout: Layout,
    /// `Side by side` or `Unified`.
    pub label: &'static str,
    /// Whether it matches the stored preference, not the width-constrained output.
    pub selected: bool,
    pub action: Action,
}

/// One message of a thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageView {
    /// Author, time and candidate.
    pub attribution: Rich,
    pub text: String,
}

/// A text field and its send control. The renderer keeps the draft under `draft`, and on Send
/// dispatches `Action::Submit { target, text }` with the draft's text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Composer {
    /// Draft key, unique on the page.
    pub draft: String,
    pub target: Compose,
    pub placeholder: &'static str,
    /// Accessible name of the field.
    pub label: &'static str,
    pub send: &'static str,
    /// Closes the composer without sending.
    pub cancel: Option<Button>,
    /// Why Send will not run, shown under the field; Send stays pressable to explain.
    pub refusal: Option<String>,
}

/// A command this page sent and its truthful status, with what can still be done about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusLine {
    pub text: Rich,
    pub hue: Hue,
    /// Check again, restore the unsent text, or dismiss a settled line.
    pub actions: Vec<Button>,
}

/// A discussion: the thread's messages, its state, a reply composer and thread actions. A new
/// line comment is a discussion with no messages yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommentView {
    /// Thread id, or the anchor of a new comment; a stable rendering key.
    pub key: String,
    /// Where the thread is anchored, for a thread shown away from its line.
    pub anchor: Option<String>,
    pub messages: Vec<MessageView>,
    /// Resolved, stale (older candidate), sending.
    pub chips: Vec<Chip>,
    pub composer: Option<Composer>,
    /// Resolve or reopen.
    pub actions: Vec<Button>,
    pub status: Vec<StatusLine>,
}

/// Unified or side-by-side rows of one hunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffContent {
    /// One-stream gutters and code; insert discussions after rows that name them.
    Unified(Vec<super::UnifiedRow>),
    /// Paired old/new cells; insert discussions after rows that name them.
    Split(Vec<super::SplitRow>),
}

/// One hunk of a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HunkView {
    pub header: String,
    pub content: DiffContent,
}

/// A file body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileBody {
    /// Changed-line summary; pressing it expands the file.
    Collapsed(Button),
    /// The patch was requested and has not arrived.
    Loading(String),
    /// The patch could not be shown; the line says why and what to do.
    Unavailable(String),
    /// Hunks with their discussions and any whole-file notes. The client assembles every
    /// bounded page of the patch before it is shown.
    Hunks {
        hunks: Vec<HunkView>,
        notes: Vec<String>,
        /// Why the patch ends early: corrald stopped serving it at its bound.
        tail: Option<String>,
    },
}

/// A file heading with its counts, illustrative tag, actions and resolved body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileView {
    /// Full path, also the stable rendering key.
    pub path: String,
    /// Monospace path followed by coloured added and deleted counts.
    pub heading: Rich,
    /// `lines illustrative` only for the fixture's fabricated lines.
    pub illustrative: Option<&'static str>,
    /// Ask about this file or hunk.
    pub ask: Option<Button>,
    /// Collapse an expanded served file.
    pub collapse: Option<Button>,
    /// Every discussion of the file; rows and `loose` name them by index.
    pub discussions: Vec<CommentView>,
    /// Discussions whose anchored line is not among the shown lines, shown above the hunks.
    pub loose: Vec<usize>,
    /// Whether pressing a code line opens a new line comment there.
    pub annotatable: bool,
    /// Identity captured by the displayed patch's line actions, never inferred on click.
    pub candidate: Option<super::CandidateAt>,
    pub body: FileBody,
}

/// The Diff tab with the effective, form/width-constrained presentation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffView {
    /// Actual row presentation, not merely the stored preference.
    pub layout: Layout,
    /// Phone's explanatory sentence; absent from desktop and terminal.
    pub notice: Option<&'static str>,
    pub files: Vec<FileView>,
    /// Shown instead of files when the candidate changed none.
    pub empty: Option<String>,
}

/// A marked or plain acceptance criterion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptanceView {
    /// Pass mark; absent when the record carries no per-criterion result.
    pub mark: Option<Mark>,
    pub text: String,
}

/// Rendered report data with its provenance sentence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportView {
    pub heading: String,
    pub paragraphs: Vec<String>,
    pub acceptance_heading: &'static str,
    pub acceptance: Vec<AcceptanceView>,
    /// Source label and provenance.
    pub source: Rich,
}

/// Fixture terminal screenshot illustration and its artifact caption.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenshotView {
    /// Original text and description, including illustrative provenance.
    pub evidence: &'static Screenshot,
    /// Preformatted terminal text with the source's bold and success runs.
    pub terminal: Rich,
    /// Artifact path and the real-title/illustrative-content explanation.
    pub caption: Rich,
    /// Source shows a cursor after the final prompt.
    pub cursor: bool,
}

/// One canonical artifact record and, once opened, its contents from corrald's catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactView {
    pub title: String,
    /// The canonical catalog key, shown as text and never opened as a host path.
    pub source: String,
    /// Declared media type, or `unknown type`.
    pub kind: String,
    /// Show or hide the contents.
    pub toggle: Button,
    pub content: ArtifactContent,
}

/// An artifact's contents as this form can show them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArtifactContent {
    Closed,
    Loading,
    Error(String),
    /// Text without terminal control sequences.
    Text {
        text: String,
        truncated: bool,
    },
    /// A `data:` URI of the assembled image.
    Image {
        uri: String,
        truncated: bool,
    },
    /// What this form cannot show, and where it can be seen.
    Unsupported(&'static str),
}

/// The candidate's artifact records and how they reach this device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactsView {
    pub rows: Vec<ArtifactView>,
    /// Shown when the candidate recorded none.
    pub empty: Option<&'static str>,
    pub note: &'static str,
}

/// One preformatted gate-log line with its result colour resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogView {
    pub text: String,
    pub hue: Hue,
}

/// Exactly one evidence body selected by state and form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Body {
    Diff(DiffView),
    Report(ReportView),
    /// The fixture's terminal capture.
    Screenshot(ScreenshotView),
    /// The served candidate's artifact records.
    Artifacts(ArtifactsView),
    Log {
        lines: Vec<LogView>,
        note: String,
    },
    /// No evidence to show yet, or none recorded: the line says which.
    Notice(String),
}

/// Candidate-bound decision controls; approval always goes through a fresh confirmation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecideBar {
    /// Buttons in source order: approve/changes on desktop, changes/approve on phone.
    pub buttons: Vec<Button>,
    /// Retry and Discard: in the bar on desktop and terminal, in the page on phone.
    pub secondary: Vec<Button>,
    /// Desktop's candidate-bound verdict explanation; absent on phone.
    pub explanation: Option<Rich>,
    /// The authenticated device the owner acts through; absent on phone.
    pub attribution: Option<Rich>,
    /// The request-changes note while it is open.
    pub composer: Option<Composer>,
    /// Decisions sent from this page and where they stand.
    pub status: Vec<StatusLine>,
}

/// A page-level notice about the candidate detail: loading, stale, error or superseded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub text: String,
    pub hue: Hue,
    pub action: Option<Button>,
}

/// The selected candidate's detail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetailView {
    pub header: Header,
    pub notice: Option<Notice>,
    /// Form-specific verdict section heading.
    pub verdict_heading: &'static str,
    /// Phone's right-hand verdict heading label.
    pub verdict_binding: Option<&'static str>,
    pub verdicts: Vec<VerdictView>,
    /// Shown instead of verdict rows when none are recorded.
    pub verdicts_empty: Option<&'static str>,
    pub tabs: Vec<TabView>,
    /// Desktop Diff preference controls; empty on phone, terminal or other tabs.
    pub layouts: Vec<LayoutChoice>,
    pub body: Body,
}

/// Complete content of Review; the shell supplies shared connection banners and navigation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageView {
    /// Pushed-page title on phone.
    pub title: String,
    pub phone: bool,
    /// Reading the last known state while offline, down or revalidating.
    pub stale: bool,
    pub conn: Conn,
    /// Candidate groups; empty on the fixture phone page.
    pub candidates: Vec<CandidateGroup>,
    pub candidate_note: Option<String>,
    /// The selected candidate, absent when there is none.
    pub detail: Option<DetailView>,
    /// Why no candidate is shown.
    pub empty: Option<String>,
    pub decide: DecideBar,
}

/// Only content widths ≥ 1100 pt have enough room for readable split code columns.
pub fn effective_layout(state: &State, env: Env, width: f32) -> Layout {
    if env.form == Form::Desktop && width >= 1100.0 {
        state.layout
    } else {
        Layout::Unified
    }
}

/// One tab per form-specific label, in order.
pub(super) fn tabs(state: &State, specs: &[(Tab, &'static str)]) -> Vec<TabView> {
    specs
        .iter()
        .map(|&(tab, label)| TabView {
            tab,
            label,
            selected: state.tab == tab,
            action: Action::Tab(tab),
        })
        .collect()
}

/// Desktop Side by side | Unified switch, shown with the Diff tab.
pub(super) fn layouts(state: &State, env: Env) -> Vec<LayoutChoice> {
    if state.tab != Tab::Diff || env.form != Form::Desktop {
        return Vec::new();
    }
    [
        (Layout::Split, "Side by side"),
        (Layout::Unified, "Unified"),
    ]
    .into_iter()
    .map(|(layout, label)| LayoutChoice {
        layout,
        label,
        selected: state.layout == layout,
        action: Action::Layout(layout),
    })
    .collect()
}

/// Rows for one hunk at the effective layout, discussions after their anchored rows.
pub(super) fn hunk_content(
    lines: Vec<super::NumberedLine>,
    anchors: &[Anchor],
    layout: Layout,
) -> DiffContent {
    match layout {
        Layout::Split => DiffContent::Split(split_rows(&lines, anchors)),
        Layout::Unified => DiffContent::Unified(unified_rows(lines, anchors)),
    }
}

/// One verdict row's presentation, shared by fixture and served verdicts.
pub(super) fn verdict_view(
    phone: bool,
    who: &str,
    sha: &str,
    time: &str,
    mark: Mark,
    text: &str,
    stale: bool,
) -> VerdictView {
    let mut line = vec![span(who).bold(), span(" · "), span(sha).mono()];
    if !time.is_empty() {
        line.push(span(" "));
        line.push(span(time));
    }
    VerdictView {
        mark: if stale { Mark::Stale } else { mark },
        who: who.into(),
        sha: vec![span(sha).mono().hue(if stale && !phone {
            Hue::Held
        } else {
            Hue::Neutral
        })],
        time: time.into(),
        line,
        text: vec![if stale {
            span(text).strike().dim()
        } else {
            span(text).hue(if who == "you" && !phone {
                Hue::You
            } else {
                Hue::Neutral
            })
        }],
        tag: stale.then(|| {
            chip(
                if phone {
                    "older candidate"
                } else {
                    "stale · older candidate"
                },
                Hue::Held,
            )
        }),
    }
}
