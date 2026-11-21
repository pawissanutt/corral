//! Data shared unchanged by the simple engine, CLI and wire protocol.

use std::{collections::BTreeSet, fmt, path::PathBuf};

use jiff::Timestamp;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

mod op;
pub mod pricing;

pub use op::{ConfigProblem, Next, Op, Refusal};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Idea,
    #[default]
    Backlog,
    Ready,
    Running,
    Blocked,
    Review,
    Done,
    Dropped,
}

impl Status {
    pub fn parse(value: &str) -> Result<Self, ParseStatusError> {
        [
            Self::Idea,
            Self::Backlog,
            Self::Ready,
            Self::Running,
            Self::Blocked,
            Self::Review,
            Self::Done,
            Self::Dropped,
        ]
        .into_iter()
        .find(|status| status.as_str().eq_ignore_ascii_case(value))
        .ok_or_else(|| ParseStatusError(value.to_owned()))
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Idea => "idea",
            Self::Backlog => "backlog",
            Self::Ready => "ready",
            Self::Running => "running",
            Self::Blocked => "blocked",
            Self::Review => "review",
            Self::Done => "done",
            Self::Dropped => "dropped",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseStatusError(String);

impl fmt::Display for ParseStatusError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown status {}", self.0)
    }
}

impl std::error::Error for ParseStatusError {}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Priority {
    P0,
    P1,
    #[default]
    P2,
    P3,
}

impl Priority {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::P0 => "P0",
            Self::P1 => "P1",
            Self::P2 => "P2",
            Self::P3 => "P3",
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Actor {
    #[default]
    Owner,
    Agent,
    Reviewer,
    Assistant,
    Auto,
}

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub title: String,
    pub body: String,
    pub status: Status,
    pub project: Option<String>,
    pub priority: Priority,
    pub kind: Option<String>,
    pub size: Option<String>,
    pub profile: Option<String>,
    pub base: Option<String>,
    pub parent: Option<String>,
    pub depends_on: Vec<String>,
    pub blocked: Option<Blocked>,
    pub round: u32,
    pub created: Timestamp,
    pub updated: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Blocked {
    pub reason: String,
    pub detail: Option<String>,
    pub next: Vec<Next>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Worker,
    Reviewer,
}

impl Role {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Worker => "worker",
            Self::Reviewer => "reviewer",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Done,
    Landed,
    Failed,
    Stopped,
    Superseded,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Run {
    pub id: String,
    pub task: String,
    pub role: Role,
    pub profile: String,
    pub harness: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub worktree: PathBuf,
    pub branch: String,
    pub base: String,
    pub base_sha: String,
    pub session: Option<SessionRef>,
    pub started: Timestamp,
    pub ended: Option<Timestamp>,
    pub outcome: Option<Outcome>,
    pub summary: Option<String>,
    pub head_sha: Option<String>,
    pub quiet_since: Option<Timestamp>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckKind {
    Gate,
    Review,
}

impl CheckKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Gate => "gate",
            Self::Review => "review",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckResult {
    Pass,
    Fail,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Check {
    pub task: String,
    pub kind: CheckKind,
    pub key: String,
    pub result: CheckResult,
    pub notes: Option<String>,
    pub log: Option<String>,
    pub by: Actor,
    pub at: Timestamp,
    /// The task's round when this check was recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub round: Option<u32>,
    /// Run that performed the check, if there was one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Artifact {
    pub task: String,
    pub id: u32,
    pub sha256: String,
    pub bytes: u64,
    pub kind: Option<String>,
    pub title: String,
    pub at: Timestamp,
    pub by: Actor,
    /// Project of the owning task, when supplied by the catalog.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// Run that recorded the artifact, if it was attached to a run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
}

impl Artifact {
    pub fn reference(&self) -> String {
        format!("{}:{}", self.task, self.id)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionRef {
    Herdr {
        workspace: String,
        pane: String,
    },
    Process {
        pid: u32,
        dir: PathBuf,
    },
    Pi {
        socket: PathBuf,
        inner: Box<SessionRef>,
        dir: PathBuf,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    Working,
    Blocked { excerpt: String },
    Idle,
    Gone,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    Owner,
    Auto,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewTask {
    pub title: String,
    pub body: String,
    pub status: Status,
    pub project: Option<String>,
    pub priority: Priority,
    pub kind: Option<String>,
    pub size: Option<String>,
    pub profile: Option<String>,
    pub base: Option<String>,
    pub parent: Option<String>,
    pub depends_on: Vec<String>,
}

impl Default for NewTask {
    fn default() -> Self {
        Self {
            title: String::new(),
            body: String::new(),
            status: Status::Backlog,
            project: None,
            priority: Priority::P2,
            kind: None,
            size: None,
            profile: None,
            base: None,
            parent: None,
            depends_on: Vec::new(),
        }
    }
}

/// Outer `None` leaves a field alone; `Some(None)` clears optional metadata.
/// On the wire these are represented by an omitted field and an explicit null.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskEdit {
    pub title: Option<String>,
    pub body: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    pub project: Option<Option<String>>,
    pub priority: Option<Priority>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    pub kind: Option<Option<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    pub size: Option<Option<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    pub profile: Option<Option<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    pub base: Option<Option<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    pub parent: Option<Option<String>>,
    pub depends_on: Option<Vec<String>>,
}

fn present<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    pub task: Option<String>,
    pub run: Option<String>,
    pub at: Timestamp,
    pub profile: Option<String>,
    /// Reported harness, or the harness of the exact associated run when resolved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<String>,
    pub model: String,
    #[serde(default)]
    pub input: u64,
    #[serde(default)]
    pub output: u64,
    #[serde(default)]
    pub cache_read: u64,
    #[serde(default)]
    pub cache_write: u64,
    /// Identifies which zero counters were explicitly reported. For example,
    /// `"explicit_zero_tokens":["cache_read","cache_write"]` attests to both cache zeros.
    /// Shared `Usage` serde treats positive counts as evidence on their own and
    /// unmarked zeros (including legacy stored ones) as unknown. Submission
    /// protocols can record zero evidence when the incoming field was supplied.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub explicit_zero_tokens: BTreeSet<UsageToken>,
    pub usd: Option<f64>,
    pub billing: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageToken {
    Input,
    Output,
    CacheRead,
    CacheWrite,
}

impl Usage {
    /// A count backed by evidence, including an explicitly reported zero.
    pub fn token_count(&self, token: UsageToken) -> Option<u64> {
        let count = match token {
            UsageToken::Input => self.input,
            UsageToken::Output => self.output,
            UsageToken::CacheRead => self.cache_read,
            UsageToken::CacheWrite => self.cache_write,
        };
        (count != 0 || self.explicit_zero_tokens.contains(&token)).then_some(count)
    }
}

#[derive(Debug, Default, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Applied {
    pub task: Option<Task>,
    pub run: Option<Run>,
    pub artifact: Option<Artifact>,
    pub seq: u64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    Created,
    Edited,
    Commented,
    Moved,
    Dispatched,
    Blocked,
    Resumed,
    Replied,
    Done,
    Gate,
    ReviewStarted,
    Verdict,
    SentBack,
    Ready,
    Landed,
    Forced,
    Stopped,
    Dropped,
    Reopened,
    Artifact,
    Error,
}

impl EventKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Edited => "edited",
            Self::Commented => "commented",
            Self::Moved => "moved",
            Self::Dispatched => "dispatched",
            Self::Blocked => "blocked",
            Self::Resumed => "resumed",
            Self::Replied => "replied",
            Self::Done => "done",
            Self::Gate => "gate",
            Self::ReviewStarted => "review_started",
            Self::Verdict => "verdict",
            Self::SentBack => "sent_back",
            Self::Ready => "ready",
            Self::Landed => "landed",
            Self::Forced => "forced",
            Self::Stopped => "stopped",
            Self::Dropped => "dropped",
            Self::Reopened => "reopened",
            Self::Artifact => "artifact",
            Self::Error => "error",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub seq: u64,
    pub ts: Timestamp,
    pub task: Option<String>,
    /// Current project of the task, resolved when events are read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    pub run: Option<String>,
    pub kind: EventKind,
    pub actor: Actor,
    pub detail: Value,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageBy {
    Profile,
    Model,
    Task,
    Day,
}

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageTotal {
    pub requests: u64,
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub priced_requests: u64,
    /// Sum of reported prices; `None` if no requests reported a price.
    pub usd: Option<f64>,
    /// Sum of known API-equivalent estimates, not a reported charge or subscription bill.
    /// `None` means no estimate is available; coverage is `estimated_requests` of `requests`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimate_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub estimated_requests: u64,
    /// Requests omitted from the estimate because at least one token count lacks evidence.
    /// The remaining unestimated requests have complete counts but no applicable API quote.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub estimate_missing_token_requests: u64,
    /// Sources used for the known estimates, not for the reported prices.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub estimate_sources: BTreeSet<String>,
}

fn is_zero(value: &u64) -> bool {
    *value == 0
}

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageGroup {
    /// Missing request or run group is `None`; days are local for a series, UTC otherwise.
    pub key: Option<String>,
    pub total: UsageTotal,
    /// Cohort runs with at least one report in this group.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub covered_runs: Option<u64>,
    /// Persisted runs in this group, including those without reports.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_runs: Option<u64>,
}

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageSummary {
    pub total: UsageTotal,
    /// Empty when ungrouped; otherwise ordered by key (missing first).
    pub groups: Vec<UsageGroup>,
    /// Distinct cohort runs with a report matching both IDs and, for a series, the local day.
    pub covered_runs: u64,
    pub total_runs: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageDay {
    /// Local calendar date in `YYYY-MM-DD` format.
    pub day: String,
    pub summary: UsageSummary,
}
