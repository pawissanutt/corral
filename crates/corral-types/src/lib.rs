//! Data shared unchanged by the simple engine, CLI and wire protocol.

use std::{collections::BTreeSet, fmt, path::PathBuf};

use jiff::Timestamp;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

pub mod board_plan;
pub mod facts;
pub mod lenses;
mod op;
pub mod pricing;
mod references;
mod system;
mod task_metadata;

pub use facts::*;
pub use op::{ConfigProblem, Next, Op, Refusal};
pub use references::{Reference, references_in, section_slug};
pub use system::*;
pub use task_metadata::{BUILTIN_TAGS, TASK_SIZES, TaskDifficulty, normalize_tags, validate_tag};

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
    /// Each admitted task holds one slot, including while its candidate is in the pipeline.
    pub const SLOT_HOLDERS: [Self; 3] = [Self::Running, Self::Blocked, Self::Review];

    pub fn holds_slot(self) -> bool {
        Self::SLOT_HOLDERS.contains(&self)
    }

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
    Extension,
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
    pub difficulty: Option<TaskDifficulty>,
    #[serde(default)]
    pub tags: Vec<String>,
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    Reference,
    Report,
    Evidence,
    Diff,
    Summary,
    Gate,
    Log,
    Usage,
    Tool,
    #[default]
    Legacy,
}

impl ArtifactKind {
    pub const ALL: [Self; 10] = [
        Self::Reference,
        Self::Report,
        Self::Evidence,
        Self::Diff,
        Self::Summary,
        Self::Gate,
        Self::Log,
        Self::Usage,
        Self::Tool,
        Self::Legacy,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Reference => "reference",
            Self::Report => "report",
            Self::Evidence => "evidence",
            Self::Diff => "diff",
            Self::Summary => "summary",
            Self::Gate => "gate",
            Self::Log => "log",
            Self::Usage => "usage",
            Self::Tool => "tool",
            Self::Legacy => "legacy",
        }
    }

    pub const fn group(self) -> ArtifactGroup {
        match self {
            Self::Reference => ArtifactGroup::Reference,
            Self::Report => ArtifactGroup::Deliverable,
            Self::Evidence => ArtifactGroup::Proof,
            Self::Diff | Self::Summary => ArtifactGroup::Submission,
            Self::Gate | Self::Log | Self::Usage | Self::Tool | Self::Legacy => {
                ArtifactGroup::Record
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactGroup {
    Reference,
    Deliverable,
    Proof,
    Submission,
    Record,
}

// Existing artifact documents may have null, absent or arbitrary string kinds.
// Keep this compatibility local to artifact rows; operations use the strict enum.
fn deserialize_artifact_kind<'de, D: Deserializer<'de>>(de: D) -> Result<ArtifactKind, D::Error> {
    let name = Option::<String>::deserialize(de)?;
    Ok(ArtifactKind::ALL
        .into_iter()
        .find(|kind| name.as_deref() == Some(kind.as_str()))
        .unwrap_or_default())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Artifact {
    /// Owner ID: a task ID or the name of the project library.
    pub task: String,
    pub id: u32,
    pub sha256: String,
    pub bytes: u64,
    #[serde(default, deserialize_with = "deserialize_artifact_kind")]
    pub kind: ArtifactKind,
    pub title: String,
    pub at: Timestamp,
    pub by: Actor,
    /// Owning project for a library artifact, or the task's project when supplied by the catalog.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// Run that recorded the artifact, if it was attached to a run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
    /// The prior artifact reference explicitly replaced by this row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replaces: Option<String>,
    /// This row's direct successor, supplied by the catalog and never stored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replaced_by: Option<String>,
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
    pub difficulty: Option<TaskDifficulty>,
    #[serde(default)]
    pub tags: Vec<String>,
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
            difficulty: None,
            tags: Vec::new(),
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
    pub difficulty: Option<Option<TaskDifficulty>>,
    /// Replace the exact tag list; an empty list clears it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
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
    /// Sum of known input counts; interpret alongside `input_requests`.
    pub input: u64,
    /// Sum of known output counts; interpret alongside `output_requests`.
    pub output: u64,
    /// Sum of known cache-read counts; interpret alongside `cache_read_requests`.
    pub cache_read: u64,
    /// Sum of known cache-write counts; interpret alongside `cache_write_requests`.
    pub cache_write: u64,
    /// Requests with an evidenced input count, including explicit zero.
    /// `None` means coverage is unavailable (e.g. on older wire responses).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_requests: Option<u64>,
    /// Requests with an evidenced output count, using the same coverage convention.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_requests: Option<u64>,
    /// Requests with an evidenced cache-read count, using the same coverage convention.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_requests: Option<u64>,
    /// Requests with an evidenced cache-write count, using the same coverage convention.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_requests: Option<u64>,
    pub priced_requests: u64,
    /// Sum of reported prices; `None` if no requests reported a price.
    pub usd: Option<f64>,
    /// Whether estimate coverage metadata was calculated. False means unknown,
    /// including responses from servers predating estimate metadata.
    #[serde(default, skip_serializing_if = "is_false")]
    pub estimate_metadata_available: bool,
    /// Sum of known API-equivalent estimates, not a reported charge or subscription bill.
    /// `None` means no estimate is available. Coverage is `estimated_requests` of
    /// `requests` only when `estimate_metadata_available` is true; otherwise counts may be partial.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimate_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub estimated_requests: u64,
    /// Requests omitted from the estimate because at least one token count lacks evidence.
    /// When metadata is available, remaining unestimated requests have complete counts but no applicable API quote.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub estimate_missing_token_requests: u64,
    /// Sources used for the known estimates, not for the reported prices.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub estimate_sources: BTreeSet<String>,
}

fn is_false(value: &bool) -> bool {
    !*value
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

#[cfg(test)]
mod usage_tests {
    use super::UsageTotal;
    use serde_json::{json, to_value};

    #[test]
    fn usage_total_distinguishes_legacy_coverage_from_zero_coverage() {
        let legacy = json!({
            "requests": 2,
            "input": 10,
            "output": 0,
            "cache_read": 0,
            "cache_write": 0,
            "priced_requests": 0,
            "usd": null,
        });
        let old: UsageTotal = serde_json::from_value(legacy.clone()).unwrap();
        assert_eq!(
            [
                old.input_requests,
                old.output_requests,
                old.cache_read_requests,
                old.cache_write_requests,
            ],
            [None; 4]
        );
        assert!(!old.estimate_metadata_available);
        assert_eq!(old.estimated_requests, 0);
        assert_eq!(old.estimate_missing_token_requests, 0);
        assert_eq!(to_value(&old).unwrap(), legacy);

        let current = UsageTotal {
            estimate_metadata_available: true,
            input_requests: Some(2),
            output_requests: Some(0),
            cache_read_requests: Some(1),
            cache_write_requests: Some(0),
            ..old
        };
        let wire = to_value(&current).unwrap();
        assert_eq!(wire["estimate_metadata_available"], json!(true));
        assert!(wire.get("estimated_requests").is_none());
        assert_eq!(wire["input_requests"], json!(2));
        assert_eq!(wire["output_requests"], json!(0));
        assert_eq!(wire["cache_read_requests"], json!(1));
        assert_eq!(wire["cache_write_requests"], json!(0));
        assert_eq!(serde_json::from_value::<UsageTotal>(wire).unwrap(), current);
    }
}
