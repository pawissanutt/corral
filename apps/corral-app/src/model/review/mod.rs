//! Pure Review model: candidate-bound verdicts, evidence, line discussions and decisions.
//!
//! The feed-backed candidate lists tasks awaiting review and reads their immutable candidate,
//! task details and expanded file patches. Every decision and comment is a canonical command
//! carrying the current candidate's preconditions.
//! Render the builders' fields without consulting state again.
//! Patches and new-comment composers carry the same immutable run/base/head identity. An older
//! daemon that omits a patch identity can still be read, but cannot safely accept line actions.

mod decide;
mod diff;
mod evidence;
mod live;
mod live_files;
mod view;

#[cfg(test)]
mod candidate_tests;
pub use diff::{
    Anchor, Highlight, NumberedLine, ParsedHunk, ParsedPatch, SplitRow, UnifiedRow, line_index,
    parse_patch, split_rows, unified_rows, word_mark,
};
pub use live::{candidate_list, candidates, live_page, selected};
pub use view::{
    AcceptanceView, ArtifactContent, ArtifactView, ArtifactsView, Body, Button, ButtonStyle,
    CandidateGroup, CandidateRow, Chip, CommentView, Composer, DecideBar, DetailView, DiffContent,
    DiffView, FileBody, FileView, Header, HunkView, LayoutChoice, LogView, MessageView, Notice,
    PageView, ReportView, StatusLine, TabView, VerdictView, effective_layout,
};

use std::collections::BTreeSet;

use crate::model::daemon::Feed;
use crate::model::effect::{Effect, Env};
use crate::model::id::Id;
use crate::model::live::{Live, Selection};

/// Review's keyboard shortcut toggles the stored split/unified preference.
pub const KEYS: &[&str] = &["s"];

/// The desktop preference; phone, terminal and narrow desktop output remain unified.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Layout {
    /// Old and new code side by side when at least 1100 pt of content are available.
    #[default]
    Split,
    /// Old/new gutters on a single stream of source lines.
    Unified,
}

/// Evidence tabs for the candidate's patches and artifact records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    /// Changed files.
    #[default]
    Diff,
    /// Rendered implementation report and acceptance list.
    Report,
    /// Candidate artifact records.
    Shot,
    /// Gate output.
    Log,
}
/// A task and the immutable coding candidate an action or draft belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateAt {
    pub task: Id,
    pub candidate: corral_protocol::detail::ReviewCandidate,
}

impl CandidateAt {
    pub(super) fn draft(&self, suffix: &str) -> String {
        format!(
            "{}:{}:{}:{}:{suffix}",
            self.task, self.candidate.run, self.candidate.base, self.candidate.head
        )
    }
}

/// A line of a served file a new comment is anchored to.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LineAt {
    pub path: String,
    /// The deleted side, numbered by the old file; otherwise the new file.
    pub old_side: bool,
    pub line: u32,
}

/// What a composer's Send submits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Compose {
    /// A new line comment on the patch's captured candidate.
    New { candidate: CandidateAt, at: LineAt },
    /// A reply to a thread at the revision the owner read.
    Reply {
        candidate: CandidateAt,
        thread: Id,
        revision: u32,
    },
    /// The request-changes note for this candidate.
    Changes(CandidateAt),
}

/// A command this page sent, kept until its outcome is dismissed or confirmed by the detail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sent {
    /// The command identity, reused for recovery; never replaced after an uncertain result.
    pub key: Id,
    /// The candidate's task.
    pub task: Id,
    pub intent: Intent,
    /// The original candidate, retained even when a failed note is restored later.
    pub candidate: Option<CandidateAt>,
}

/// What a sent command was for, with the owner's words so an unsent note can be restored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Intent {
    Approve {
        sha: String,
    },
    Changes {
        text: String,
    },
    Retry,
    Discard,
    Comment {
        at: LineAt,
        text: String,
    },
    Reply {
        candidate: CandidateAt,
        thread: Id,
        text: String,
    },
    Resolve {
        thread: Id,
    },
    Reopen {
        thread: Id,
    },
}

/// Review's own state. The selected candidate is the root's live selection; what is expanded,
/// being written or in flight is Review's.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct State {
    pub ids: crate::model::id::IdSupply,
    /// Preferred desktop diff presentation.
    pub layout: Layout,
    /// Active evidence tab; the default is Diff.
    pub tab: Tab,
    /// Served files whose patches are shown, by candidate task.
    pub expanded: BTreeSet<(Id, String)>,
    /// The line a new comment is being written on.
    pub composing: Option<(CandidateAt, LineAt)>,
    /// The candidate whose request-changes note is open.
    pub changes: Option<CandidateAt>,
    /// Artifacts whose contents are shown, by candidate task and catalog key.
    pub opened: BTreeSet<(Id, String)>,
    /// Commands sent from this page, oldest first.
    pub sent: Vec<Sent>,
}

/// Candidate interaction intents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Select an evidence tab (`rtab`).
    Tab(Tab),
    /// Set the preferred diff presentation (`rlayout`).
    Layout(Layout),
    /// Approve the shown candidate through preview and confirmation.
    Approve,
    /// Open the request-changes note for the worker.
    RequestChanges,
    /// Show a served candidate.
    Pick(Id),
    /// Show a served file's patch, fetching it once.
    Expand(String),
    Collapse(String),
    /// Open Ask about the candidate, naming a file and hunk in the draft.
    AskAbout {
        path: String,
        hunk: Option<String>,
    },
    /// Start a new comment on a served line.
    Annotate {
        candidate: CandidateAt,
        at: LineAt,
    },
    /// Close the new comment or the request-changes note without sending.
    CancelCompose,
    /// Send a composer's text.
    Submit {
        target: Compose,
        text: String,
    },
    Resolve {
        thread: Id,
        revision: u32,
    },
    Reopen {
        thread: Id,
        revision: u32,
    },
    /// Rerun the candidate's task.
    Retry,
    /// Discard the candidate, through corrald's preview and a fresh confirmation.
    Discard,
    /// Ask corrald what became of a command whose result was lost, under the same identity.
    Recover(Id),
    /// Forget a settled command's line.
    Dismiss(Id),
    /// Put an unsent note back into its composer, then forget the failed command. The
    /// renderer restores the text; the reducer drops the line.
    Restore {
        key: Id,
        draft: String,
        text: String,
    },
    /// Show an artifact's contents from corrald's catalog, fetching them once.
    OpenArtifact(String),
    CloseArtifact(String),
    /// Refetch the candidate detail, its task and every shown patch and artifact.
    Refresh,
}

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

    /// Served: `s` toggles the layout on Review itself, not under a pushed focus page.
    pub fn key_live(
        &mut self,
        key: &str,
        top: Option<&Live>,
        selection: &Selection,
        feed: &Feed,
        _env: Env,
    ) -> Option<Vec<Effect>> {
        if top.is_some_and(|live| !matches!(live, Live::Review(_))) {
            return None;
        }
        if key == "." {
            let id = match top {
                Some(Live::Review(id)) => id.clone(),
                _ => selected(selection, feed)?.id.clone(),
            };
            return Some(vec![Effect::VerbMenu(crate::model::work::Record::Review(
                id,
            ))]);
        }
        self.toggle(key)
    }

    /// Toggling on phone or terminal changes the preference, not their unified output.
    fn toggle(&mut self, key: &str) -> Option<Vec<Effect>> {
        if key != "s" {
            return None;
        }
        self.layout = match self.layout {
            Layout::Split => Layout::Unified,
            Layout::Unified => Layout::Split,
        };
        Some(Vec::new())
    }
}
