//! Pure Review model: the T-868 candidate replay, candidate-bound verdicts and evidence.
//!
//! Render [`page`] for the active form, or [`page_for_width`] with the available desktop width.
//! These builders resolve the evidence tab, phone wording, stale verdict styles, diff pairing,
//! inline discussion placement and command availability. Render their fields without consulting
//! state again. Dispatch every button's [`Action`] through [`State::apply`].
//!
//! The source candidate list and collapsed files only acknowledge clicks with a toast: they do
//! not switch the replay or reveal code that was not supplied. The comment box is a static
//! affordance with no source action. [`State::comment_box`] preserves that inert behavior rather
//! than inventing persistence. Approval always goes through a fresh device confirmation.

mod data;
mod diff;
mod view;

pub use data::{
    Acceptance, CANDIDATES, Candidate, Comment, FILES, File, GATE_LOG, GATE_LOG_NOTE, HunkSource,
    LogLine, REPORT, Report, SCREENSHOT, Screenshot, SourceLine, VERDICTS, Verdict,
};
pub use diff::{
    Highlight, NumberedLine, SplitRow, UnifiedRow, hunk_header, hunk_lines, number_lines, split,
    split_rows, unified, word_mark,
};
pub use view::{
    Body, Button, ButtonStyle, CandidateGroup, CandidateRow, Chip, CommentView, DecideBar,
    DiffContent, DiffView, FileView, Header, LayoutChoice, PageView, ReportView, ScreenshotView,
    TabView, VerdictView, effective_layout, files, page, page_for_width, phone, verdicts,
};

use crate::model::effect::{Effect, Env, Page, Risk};
use crate::model::nav::Form;

/// Review's keyboard shortcut toggles the stored split/unified preference.
pub const KEYS: &[&str] = &["s"];

/// The desktop preference; phone, terminal and narrow desktop output remain unified.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Layout {
    /// Old and new code side by side when at least 900 pt are available.
    #[default]
    Split,
    /// Old/new gutters on a single stream of source lines.
    Unified,
}

/// Evidence tabs; the phone omits Gate log and calls Screenshot `Screen`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    /// Five changed files.
    #[default]
    Diff,
    /// Rendered implementation report and acceptance list.
    Report,
    /// Terminal screenshot evidence.
    Shot,
    /// Illustrative gate output with a real receipt id.
    Log,
}

/// Review's fields of the mockup state. The selected replay is deliberately fixed at T-868.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct State {
    /// Preferred desktop diff presentation.
    pub layout: Layout,
    /// Active evidence tab; the default is Diff.
    pub tab: Tab,
}

/// Button intents, including toast-only affordances from the mockup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Acknowledge a candidate-list click without changing the replay detail.
    Select(&'static str),
    /// Select an evidence tab (`rtab`).
    Tab(Tab),
    /// Set the preferred diff presentation (`rlayout`).
    Layout(Layout),
    /// Request fresh confirmation for candidate 85b45e1e.
    Approve,
    /// Show the form-specific request-changes toast.
    RequestChanges,
    /// Open Ask with the diff-hunk context chip.
    AskHunk,
    /// Show the source's `Expanded {path}` toast; no hidden lines were supplied.
    ExpandFile(&'static str),
    /// The displayed comment box; the mockup has no editor action or toast.
    CommentBox,
}

/// Refusal shown instead of opening confirmation while offline or down.
pub const APPROVAL_REFUSED: &str = "Refused: needs a live connection. Nothing was queued.";

impl State {
    /// Selects the evidence body without a toast, as `rtab` does.
    pub fn rtab(&mut self, tab: Tab) -> Vec<Effect> {
        self.tab = tab;
        Vec::new()
    }

    /// Sets the desktop preference without a toast, as `rlayout` does.
    pub fn rlayout(&mut self, layout: Layout) -> Vec<Effect> {
        self.layout = layout;
        Vec::new()
    }

    /// Requests confirmation; offline/down refuses and never queues approval.
    pub fn approve(&mut self, env: Env) -> Vec<Effect> {
        if env.conn.refuses() {
            vec![Effect::Toast(APPROVAL_REFUSED.into())]
        } else {
            vec![Effect::Confirm(Risk::ApproveReview, None)]
        }
    }

    /// Shows the mockup's phone or desktop toast, including when disconnected.
    /// This affordance neither submits nor queues a worker instruction.
    pub fn request_changes(&mut self, env: Env) -> Vec<Effect> {
        let message = if env.form == Form::Phone {
            "Request changes"
        } else {
            "Request changes: comments go to the worker as round 2 input"
        };
        vec![Effect::Toast(message.into())]
    }

    /// Opens Ask with hunk context, without inventing a draft or sending a message.
    pub fn ask_hunk(&mut self) -> Vec<Effect> {
        vec![Effect::Ask {
            chip: Some("hunk"),
            text: None,
        }]
    }

    /// Acknowledges the collapsed-file click without fabricating unavailable code.
    pub fn expand_file(&mut self, path: &'static str) -> Vec<Effect> {
        vec![Effect::Toast(format!("Expanded {path}"))]
    }

    /// Preserves the source's inert reply affordance: no editor state and no invented toast.
    pub fn comment_box(&mut self) -> Vec<Effect> {
        Vec::new()
    }

    /// Acknowledges candidate selection; the source retains T-868 as its selected replay.
    pub fn select(&mut self, id: &'static str) -> Vec<Effect> {
        vec![Effect::Toast(format!("{id} selected"))]
    }

    /// Applies a view descriptor's intent and returns only cross-app effects.
    pub fn apply(&mut self, action: Action, env: Env) -> Vec<Effect> {
        match action {
            Action::Select(id) => self.select(id),
            Action::Tab(tab) => self.rtab(tab),
            Action::Layout(layout) => self.rlayout(layout),
            Action::Approve => self.approve(env),
            Action::RequestChanges => self.request_changes(env),
            Action::AskHunk => self.ask_hunk(),
            Action::ExpandFile(path) => self.expand_file(path),
            Action::CommentBox => self.comment_box(),
        }
    }

    /// Handles `s` on Review, but not while a pushed cross-domain page is on top.
    /// Toggling on phone or terminal changes the preference, not their unified output.
    pub fn key(&mut self, key: &str, page: Option<Page>, _env: Env) -> Option<Vec<Effect>> {
        if page.is_some() || key != "s" {
            return None;
        }
        self.layout = match self.layout {
            Layout::Split => Layout::Unified,
            Layout::Unified => Layout::Split,
        };
        Some(Vec::new())
    }
}
