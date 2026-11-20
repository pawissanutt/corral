//! Typed content for feed-backed Review renderers.
//! No widgets, dimensions or styling APIs live here.

use crate::model::effect::Env;
use crate::model::hue::Hue;
use crate::model::nav::Form;
use crate::model::text::Rich;

use super::diff::{split_rows, unified_rows};
use super::{Action, Layout, State, Tab};

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

/// A labelled intent. Engine operations are never disabled: the engine decides.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Button {
    /// Visible label.
    pub label: String,
    /// The label where a row of controls runs out of room (a terminal's decide line).
    pub short: Option<&'static str>,
    /// Reducer intent.
    pub action: Action,
    /// Source emphasis.
    pub style: ButtonStyle,
    /// What pressing it does beyond its label: the control's hint.
    pub hint: Option<String>,
}

pub(super) fn button(label: impl Into<String>, action: Action, style: ButtonStyle) -> Button {
    Button {
        label: label.into(),
        short: None,
        action,
        style,
        hint: None,
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

/// An operation this page sent and where it stands, with Dismiss once it settled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusLine {
    pub text: Rich,
    pub hue: Hue,
    pub actions: Vec<Button>,
}

/// Unified or side-by-side rows of one hunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffContent {
    /// One-stream gutters and code.
    Unified(Vec<super::UnifiedRow>),
    /// Paired old/new cells.
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
    /// Hunks and any whole-file notes. The client assembles every bounded page of the patch
    /// before it is shown.
    Hunks {
        hunks: Vec<HunkView>,
        notes: Vec<String>,
        /// Why the patch ends early: corrald stopped serving it at its bound.
        tail: Option<String>,
    },
}

/// A file heading with its counts, actions and resolved body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileView {
    /// Full path, also the stable rendering key.
    pub path: String,
    /// Monospace path followed by coloured added and deleted counts.
    pub heading: Rich,
    /// Ask about this file or hunk.
    pub ask: Option<Button>,
    /// Collapse an expanded served file.
    pub collapse: Option<Button>,
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

/// The worker run's summary with its provenance sentence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportView {
    pub heading: String,
    pub paragraphs: Vec<String>,
    /// Source label and provenance.
    pub source: Rich,
}

/// One artifact record and, once opened, its contents from corrald's catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactView {
    pub title: String,
    /// The artifact's reference (`T-42:3`), shown as text and never opened as a host path.
    pub source: String,
    /// Declared kind, or `unknown kind`.
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

/// The task's artifact records and how they reach this device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactsView {
    pub rows: Vec<ArtifactView>,
    /// Shown when the task recorded none.
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
    /// The task's artifact records.
    Artifacts(ArtifactsView),
    Log {
        lines: Vec<LogView>,
        note: String,
    },
    /// No evidence to show yet, or none recorded: the line says which.
    Notice(String),
}

/// The branch approving lands onto: the run's base until another is typed. Any existing branch
/// is a target; the engine refuses a missing one and names the branches that exist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// The field's caption.
    pub label: &'static str,
    /// What the field holds now.
    pub value: String,
    pub placeholder: &'static str,
    /// What the field decides, for the pointer and the screen reader.
    pub hint: String,
    /// The project's local branches offered for what the field holds, one press each.
    pub branches: Vec<String>,
}

/// The decide bar: every operation that decides a candidate, always pressable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecideBar {
    /// Approve and land, Request changes and Ask (Request changes first on the phone).
    pub buttons: Vec<Button>,
    /// Where Approve and Land anyway land: beside Approve on desktop and terminal, in the page
    /// on the phone.
    pub target: Target,
    /// Run gate, Start review and, while the gate is red or after a refused Approve, Land
    /// anyway: at the trailing edge on desktop and terminal, in the page on the phone.
    pub secondary: Vec<Button>,
    /// What approving does to this candidate; absent on phone.
    pub explanation: Option<Rich>,
    /// The device the owner acts through; absent on phone.
    pub attribution: Option<Rich>,
    /// Operations sent from this page and where they stand.
    pub status: Vec<StatusLine>,
}

/// A page-level notice about the candidate detail: loading, stale, error or no run.
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

/// Rows for one hunk at the effective layout.
pub(super) fn hunk_content(lines: Vec<super::NumberedLine>, layout: Layout) -> DiffContent {
    match layout {
        Layout::Split => DiffContent::Split(split_rows(&lines)),
        Layout::Unified => DiffContent::Unified(unified_rows(lines)),
    }
}
