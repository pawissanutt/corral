use std::path::PathBuf;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

/// Task lifecycle state, serialized snake_case in frontmatter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Idea,
    Backlog,
    Ready,
    Running,
    NeedsMe,
    Review,
    Done,
    Dropped,
}

impl Status {
    /// Board columns in order. Ideas render inside the Backlog column (filterable); Done is toggleable.
    pub const COLUMNS: [Status; 6] = [
        Status::Backlog,
        Status::Ready,
        Status::Running,
        Status::NeedsMe,
        Status::Review,
        Status::Done,
    ];

    /// Human label: "Idea", "Backlog", "Ready", "Running", "Needs me", "Review", "Done", "Dropped".
    pub fn label(self) -> &'static str {
        match self {
            Status::Idea => "Idea",
            Status::Backlog => "Backlog",
            Status::Ready => "Ready",
            Status::Running => "Running",
            Status::NeedsMe => "Needs me",
            Status::Review => "Review",
            Status::Done => "Done",
            Status::Dropped => "Dropped",
        }
    }

    /// snake_case name as written in frontmatter.
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Idea => "idea",
            Status::Backlog => "backlog",
            Status::Ready => "ready",
            Status::Running => "running",
            Status::NeedsMe => "needs_me",
            Status::Review => "review",
            Status::Done => "done",
            Status::Dropped => "dropped",
        }
    }

    /// Parses snake_case names and labels, case-insensitive; "needs-me", "needsme", "needs me" all map to NeedsMe.
    pub fn parse(s: &str) -> Option<Status> {
        let k: String = s
            .trim()
            .to_ascii_lowercase()
            .chars()
            .filter(|c| c.is_ascii_alphabetic())
            .collect();
        Some(match k.as_str() {
            "idea" | "ideas" => Status::Idea,
            "backlog" => Status::Backlog,
            "ready" => Status::Ready,
            "running" => Status::Running,
            "needsme" => Status::NeedsMe,
            "review" => Status::Review,
            "done" => Status::Done,
            "dropped" | "drop" => Status::Dropped,
            _ => return None,
        })
    }

    /// True for Running and NeedsMe: the task holds one of the global run slots.
    pub fn holds_slot(self) -> bool {
        matches!(self, Status::Running | Status::NeedsMe)
    }

    /// Column this status renders in on the board (Idea -> Backlog, Dropped -> None).
    pub fn column(self) -> Option<Status> {
        match self {
            Status::Idea => Some(Status::Backlog),
            Status::Dropped => None,
            s => Some(s),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunMode {
    #[default]
    Interactive,
    Headless,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunOutcome {
    Done,
    Abandoned,
    Superseded,
    Discarded,
}

/// Where a run lives in herdr.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HerdrRef {
    pub workspace_id: String,
    pub pane_id: String,
}

/// One agent session working on a task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Run {
    /// "R1", "R2", ...
    pub id: String,
    /// Agent key from config `[agents]` (omo, omp, claude, generic, ...).
    pub agent: String,
    #[serde(default)]
    pub mode: RunMode,
    /// Absolute worktree path.
    pub worktree: PathBuf,
    /// Branch created for the run, "corral/<task-id>" (retries: "corral/<task-id>-r<n>").
    pub branch: String,
    /// Base branch name the run started from (e.g. "main").
    pub base: String,
    /// Commit sha of `base` when the run started.
    pub base_sha: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub herdr: Option<HerdrRef>,
    pub started: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended: Option<Timestamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<RunOutcome>,
    /// Summary passed to `corral done`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// Last lines of the agent pane captured when the task entered NeedsMe.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_excerpt: Option<String>,
    /// Last herdr agent status seen by sync ("idle", "working", "blocked", "done", "unknown", "gone").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_state: Option<String>,
    /// True once sync has seen the agent "working"; idle before this is startup, not "waiting for me".
    #[serde(default)]
    pub seen_working: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    Report,
    Plan,
    Evidence,
    Screenshot,
    Diff,
    Summary,
    Log,
    Other,
}

impl ArtifactKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ArtifactKind::Report => "report",
            ArtifactKind::Plan => "plan",
            ArtifactKind::Evidence => "evidence",
            ArtifactKind::Screenshot => "screenshot",
            ArtifactKind::Diff => "diff",
            ArtifactKind::Summary => "summary",
            ArtifactKind::Log => "log",
            ArtifactKind::Other => "other",
        }
    }

    pub fn parse(s: &str) -> Option<ArtifactKind> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "report" => ArtifactKind::Report,
            "plan" => ArtifactKind::Plan,
            "evidence" => ArtifactKind::Evidence,
            "screenshot" | "image" => ArtifactKind::Screenshot,
            "diff" | "patch" => ArtifactKind::Diff,
            "summary" => ArtifactKind::Summary,
            "log" => ArtifactKind::Log,
            "other" => ArtifactKind::Other,
            _ => return None,
        })
    }

    /// Best guess from a file name: .png/.jpg/.gif/.webp -> Screenshot, .diff/.patch -> Diff,
    /// .log -> Log, names containing "plan" -> Plan, "report" or .html -> Report, else Evidence.
    pub fn guess(file_name: &str) -> ArtifactKind {
        let n = file_name.to_ascii_lowercase();
        if [".png", ".jpg", ".jpeg", ".gif", ".webp"]
            .iter()
            .any(|e| n.ends_with(e))
        {
            ArtifactKind::Screenshot
        } else if n.ends_with(".diff") || n.ends_with(".patch") {
            ArtifactKind::Diff
        } else if n.ends_with(".log") {
            ArtifactKind::Log
        } else if n.contains("plan") {
            ArtifactKind::Plan
        } else if n.contains("report") || n.ends_with(".html") {
            ArtifactKind::Report
        } else {
            ArtifactKind::Evidence
        }
    }
}

/// A snapshotted output of a run. The file lives at `<store dir>/<path>`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Artifact {
    /// Run id ("R1").
    pub run: String,
    pub kind: ArtifactKind,
    pub title: String,
    /// Snapshot path relative to the `.corral/` dir, e.g. "artifacts/T-042/R1/9f3c1a2b-report.html".
    pub path: String,
    /// Original absolute path the snapshot was copied from.
    pub source: String,
    pub sha256: String,
    pub bytes: u64,
    pub added: Timestamp,
}

/// A task file: `.corral/tasks/<id>-<slug>.md` = YAML frontmatter (this struct minus `body`/`path`) + markdown body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Task {
    /// "T-001" style, zero-padded to at least 3 digits.
    pub id: String,
    pub title: String,
    pub status: Status,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// "P0".."P3", default "P2".
    #[serde(default = "default_priority")]
    pub priority: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<String>,
    /// Preferred agent key for dispatch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// Waiting for a free slot or for dependencies; sync dispatches it when possible.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub queued: bool,
    pub created: Timestamp,
    pub updated: Timestamp,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub runs: Vec<Run>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<Artifact>,
    /// Markdown body after the frontmatter (spec, acceptance, notes).
    #[serde(skip)]
    pub body: String,
    /// Absolute path of the task file (set by the store).
    #[serde(skip)]
    pub path: PathBuf,
}

fn default_priority() -> String {
    "P2".to_string()
}

impl Task {
    /// The most recent run, if any.
    pub fn current_run(&self) -> Option<&Run> {
        self.runs.last()
    }

    pub fn current_run_mut(&mut self) -> Option<&mut Run> {
        self.runs.last_mut()
    }

    /// True for ideas (status Idea).
    pub fn is_idea(&self) -> bool {
        self.status == Status::Idea
    }
}

/// One line of `.corral/events.jsonl`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub ts: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    /// e.g. "created", "moved", "dispatched", "queued", "needs_me", "replied", "done", "artifact", "approved", "discarded".
    pub kind: String,
    #[serde(default)]
    pub detail: serde_json::Value,
}
