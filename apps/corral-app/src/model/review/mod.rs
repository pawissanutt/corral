//! Pure model of a task's candidate on its page: the stored candidate (the run's summary, its
//! diff and the gate and review checks) and the engine operations that decide it.
//!
//! The task's `Review` and `Task` details, its expanded file patches and its artifacts are read from the client's cache.
//! Every decision is one engine operation (`Effect::Act`): the engine decides whether it runs,
//! and the app never predicts a refusal. Request changes needs words, so it opens the shared
//! verb composer through the work state (`Action::Verb`, routed by the screen).
//! `loop_page_filtered` is a candidate's page: the Brief-first candidate with
//! its evidence tabs and decide bar.

mod brief;
mod brief_side;
mod decide;
mod diff;
mod evidence;
mod live;
mod live_files;
mod loop_page;
mod outline;
mod readiness;
mod rounds;
mod view;

#[cfg(test)]
mod candidate_tests;

pub use brief::{Brief, ChangedFile, checks};
pub use brief_side::{AskedFor, BriefSide, ReviewFact};
pub use diff::{
    Highlight, NumberedLine, ParsedHunk, ParsedPatch, SplitRow, UnifiedRow, parse_patch,
    split_rows, unified_rows, word_mark,
};
pub(crate) use live::actor;
pub use live::selected_filtered;
pub use loop_page::{LoopCandidate, LoopContent, LoopPage, loop_page_filtered};
pub use readiness::{ReadyLink, ReviewQuote};
pub use rounds::{ReviewerLine, Round, Rounds};
pub use view::{
    ArtifactContent, ArtifactView, ArtifactsView, Body, Button, ButtonStyle, DecideBar,
    DiffContent, DiffView, FileBody, FileView, HunkView, LayoutChoice, LogView, Notice, ReportView,
    StatusLine, TabView, Target, effective_layout,
};

use std::collections::{BTreeMap, BTreeSet};

use crate::model::effect::Effect;
use crate::model::id::Id;
use crate::model::work::Verb;

/// The decide bar's Approve (`btn-approve`).
pub const APPROVE: &str = "btn-approve";

/// The desktop preference; phone, terminal and narrow desktop output remain unified.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Layout {
    /// Old and new code side by side when at least 1100 pt of content are available.
    #[default]
    Split,
    /// Old/new gutters on a single stream of source lines.
    Unified,
}

/// Brief first, followed by candidate patches and the recorded evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    /// Changed files.
    Diff,
    /// The worker run's summary, recorded at `done`.
    Report,
    /// The task's artifact records.
    Shot,
    /// The gate check's log artifact.
    Log,
    /// Readiness, changed files and the task's request.
    #[default]
    Brief,
}

/// An engine operation this page submitted, kept until dismissed so its outcome stays visible.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sent {
    /// The request id, the key of its status in the feed.
    pub key: Id,
    /// The candidate's task.
    pub task: Id,
    /// What was sent: "Approve and land T-42".
    pub title: String,
    /// An unforced `approve`: once refused, the bar offers Land anyway.
    pub approve: bool,
}

/// Review's own state. The selected candidate is the root's live selection; what is expanded,
/// opened or in flight is Review's.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct State {
    pub ids: crate::model::id::IdSupply,
    /// Preferred desktop diff presentation.
    pub layout: Layout,
    /// Active evidence tab; the default is Brief.
    pub tab: Tab,
    /// Served files whose patches are shown, by candidate task.
    pub expanded: BTreeSet<(Id, String)>,
    /// Artifacts whose contents are shown, by candidate task and artifact reference.
    pub opened: BTreeSet<(Id, String)>,
    /// Operations sent from this page, oldest first.
    pub sent: Vec<Sent>,
    /// The branch typed into a candidate's Onto field, by candidate; absent means its base.
    pub onto: BTreeMap<Id, String>,
}

/// Candidate interaction intents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Select an evidence tab (`rtab`).
    Tab(Tab),
    /// Set the preferred diff presentation (`rlayout`).
    Layout(Layout),
    /// `approve`: land the candidate onto the Onto field's branch.
    Approve,
    /// `approve --force`: land past a red gate, onto the same branch.
    LandAnyway,
    /// The Onto field's words changed.
    Onto(String),
    /// `gate`: run the gate on the task's HEAD.
    Gate,
    /// `review`: start the reviewer.
    StartReview,
    /// A shared Actions verb on the shown task (Request changes opens its composer); the
    /// screen runs it through the work state, where the shared composer lives.
    Verb(Verb),
    /// Show a served file's patch, fetching it once.
    Expand(String),
    Collapse(String),
    /// Open Ask about the candidate, naming a file and hunk in the draft.
    AskAbout {
        path: String,
        hunk: Option<String>,
    },
    /// Open the shared Ask composer with the whole candidate as context.
    AskCandidate,
    /// Open the candidate run's agent session.
    OpenSession,
    /// Open the task's live reviewer session.
    OpenReviewer,
    /// Forget an operation's status line.
    Dismiss(Id),
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
}
