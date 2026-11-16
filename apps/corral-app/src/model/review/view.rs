//! Typed content for feed-backed Review renderers.
//! No widgets, dimensions or styling APIs live here.

use crate::model::board::Mark;
use crate::model::effect::Env;
use crate::model::hue::Hue;
use crate::model::nav::Form;
use crate::model::text::Rich;

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
    /// The label where a row of controls runs out of room (a terminal's decide line).
    pub short: Option<&'static str>,
    /// Reducer intent.
    pub action: Action,
    /// Whether the control can be activated at all.
    pub enabled: bool,
    /// Source emphasis.
    pub style: ButtonStyle,
    /// Why activating it will not run the command: the control's hint.
    pub refusal: Option<String>,
}

pub(super) fn button(label: impl Into<String>, action: Action, style: ButtonStyle) -> Button {
    Button {
        label: label.into(),
        short: None,
        action,
        enabled: true,
        style,
        refusal: None,
    }
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
    /// Optional patch-source disclosure.
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

/// Only content widths ≥ 1100 pt have enough room for readable split code columns.
pub fn effective_layout(state: &State, env: Env, width: f32) -> Layout {
    if env.form == Form::Desktop && width >= 1100.0 {
        state.layout
    } else {
        Layout::Unified
    }
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
