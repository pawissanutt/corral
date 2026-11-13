//! Documents and rows a [`super::ReadView`] returns (plan T-253 sections 1, 5, 7-9 and 11).
//!
//! Documents are backend-neutral: task heads, history rows, events and records carry their
//! payload as JSON values whose shape the facade owns; header columns are typed so backends can
//! index them.

use std::collections::BTreeMap;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::operation::OpKind;
use crate::blob_ref::BlobHash;

/// Position in the change feed: strictly increasing, one per committed operation that writes.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
pub struct ChangeSeq(pub u64);

/// Position in the event log: strictly increasing, one per stored event.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
pub struct EventSeq(pub u64);

/// Numeric task identity (`T-042` is number 42).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TaskNum(pub u64);

/// Statuses that leave the open index.
pub const CLOSED_STATUSES: [&str; 2] = ["done", "dropped"];

pub fn is_open_status(status: &str) -> bool {
    !CLOSED_STATUSES.contains(&status)
}

/// Header columns a task write sets; identity, revision and creation time are the backend's.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskFields {
    pub title: String,
    pub status: String,
    pub project: Option<String>,
    pub priority: String,
    pub parent: Option<String>,
    pub updated: Timestamp,
    /// Typed relations (`depends_on` and the like) to other task ids.
    pub edges: Vec<TaskEdge>,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub size: Option<String>,
    #[serde(default)]
    pub tracker: bool,
    #[serde(default)]
    pub design: bool,
    #[serde(default)]
    pub queued: bool,
    /// The facade's specification revision of the task.
    #[serde(default)]
    pub spec_rev: String,
    /// The unpaused dashboard schedule's next occurrence, indexed for the due page.
    #[serde(default)]
    pub dashboard_due: Option<Timestamp>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TaskEdge {
    pub kind: String,
    pub other: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskHeader {
    pub num: TaskNum,
    pub id: String,
    /// Incremented by exactly one on every committed write of the task.
    pub rev: u64,
    pub created: Timestamp,
    #[serde(flatten)]
    pub fields: TaskFields,
}

impl TaskHeader {
    pub fn is_open(&self) -> bool {
        is_open_status(&self.fields.status)
    }
}

/// A task head: header, head document (head parts and current-container projections) and the
/// history summary of its non-current rows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskHeadDoc {
    pub header: TaskHeader,
    pub doc: Value,
    pub summary: Value,
}

/// History and child row families of one task (one per table of plan section 11).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskPart {
    Run,
    PlanRun,
    ReviewRun,
    RunUsage,
    RunCheckpoint,
    RunSwitch,
    RunObligation,
    InboxEffect,
    ObserveCursor,
    Plan,
    PlanReview,
    AggregateAttempt,
    AggregateReview,
    PaneRef,
    Comment,
    AnnotationThread,
    AnnotationEntry,
    ScopeRevision,
    Recovery,
    RecoveryOutcome,
    Artifact,
    Finding,
    Prompt,
    GateAttempt,
    ReviewEvidence,
    /// A checkpoint's canonical payload, keyed by its digest and shared by every meta row
    /// that references it.
    CheckpointPayload,
    /// One corral-pi record (generation, receipt, question, turn, wake or report) of a run.
    AgentRecord,
    /// One receipt of a corral-pi PM operation targeting the task.
    PmReceipt,
    /// One superseded revision of a member gate.
    MemberGateHistory,
    /// One pane the task's records named that cleanup closes (a run's own pane, gate and
    /// reviewer panes, a plan reviewer's pane; never the PM's), kept after its record is
    /// replaced. `open` until cleanup confirms the pane closed or absent.
    CleanupPane,
    /// One payload-free derived task marker for owed attention and held-slot inputs.
    WorkIndex,
}

/// One history or child row. `current` marks the row a head carries; `open` marks a row that
/// has not ended (an unfinished run, a pending review, an open obligation).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HistoryRow {
    pub part: TaskPart,
    pub key: String,
    pub current: bool,
    pub open: bool,
    pub doc: Value,
    /// Secondary lookup tokens (`id:<request>`, `at:<time>`, ...) that a
    /// [`super::RowQuery`] can select by without decoding other rows.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub index: Vec<String>,
}

/// Which row families a [`super::ReadView::task`] read returns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskParts {
    All,
    Only(Vec<TaskPart>),
}

impl TaskParts {
    pub fn includes(&self, part: TaskPart) -> bool {
        match self {
            Self::All => true,
            Self::Only(parts) => parts.contains(&part),
        }
    }
}

/// A full or partial task: its head plus the selected rows in `(part, key)` order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskDoc {
    pub head: TaskHeadDoc,
    pub rows: Vec<HistoryRow>,
}

/// An event as an operation appends it; the backend assigns the seq.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewEvent {
    pub ts: Timestamp,
    pub task: Option<String>,
    pub kind: String,
    pub actor: Option<String>,
    pub via: Option<String>,
    pub run: Option<String>,
    pub detail: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoredEvent {
    pub seq: EventSeq,
    pub event: NewEvent,
    /// A legacy line imported verbatim that did not decode (plan section 9).
    pub malformed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CloseReason {
    Changed,
    Cleared,
}

/// One run of identical condition observations (plan section 9), durable after it closes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConditionRun {
    pub opened_seq: EventSeq,
    pub task: String,
    pub kind: String,
    pub detail_digest: String,
    pub count: u64,
    pub first_at: Timestamp,
    pub last_at: Timestamp,
    pub first_actor: Option<String>,
    pub first_via: Option<String>,
    pub last_actor: Option<String>,
    pub last_via: Option<String>,
    pub closed_seq: Option<EventSeq>,
    pub close_reason: Option<CloseReason>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoredRecord {
    pub area: String,
    pub key: String,
    pub version: u64,
    pub doc: Value,
    pub updated: Timestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlobState {
    Writing,
    Live,
    Deleting,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Reservation {
    pub token: String,
    pub owner: String,
    pub expires: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BlobRow {
    pub hash: BlobHash,
    pub raw_bytes: u64,
    pub stored_bytes: u64,
    pub codec: String,
    pub state: BlobState,
    pub refs: u64,
    /// Set by the operation that removes the last reference; cleared by one that adds one.
    pub unreferenced_since: Option<Timestamp>,
    pub created: Timestamp,
    pub reservations: Vec<Reservation>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScratchKind {
    ImplementationReview,
    DesignPlanReview,
    AggregateReview,
    Gate,
    PlannerOutput,
    ReviewWatchLog,
    Prompt,
    TmpStaging,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScratchState {
    Open,
    Closed,
    Deferred,
    Reclaiming,
    Reclaimed,
    Failed,
}

/// A registered scratch directory; `path` is relative to its class root (plan section 8).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScratchRow {
    pub id: String,
    pub kind: ScratchKind,
    pub task: Option<String>,
    pub subject: Option<String>,
    pub attempt: Option<String>,
    pub path: String,
    pub state: ScratchState,
    pub reason: Option<String>,
    pub next_check_at: Option<Timestamp>,
    pub owner: Option<String>,
    pub warned: Option<Timestamp>,
    pub evidence_bytes: u64,
    pub created: Timestamp,
    pub closed: Option<Timestamp>,
    pub reclaimed: Option<Timestamp>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleOutcome {
    Reclaimed,
    Retained,
    Deferred,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LifecycleRow {
    pub task: String,
    pub generation: String,
    pub target: String,
    pub policy_digest: String,
    pub outcome: LifecycleOutcome,
    pub reason: Option<String>,
    pub bytes: Option<u64>,
    pub next_check_at: Option<Timestamp>,
    pub at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LeaseRow {
    pub id: String,
    pub holder: String,
    pub started: Timestamp,
    pub expires: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClaimRow {
    pub name: String,
    pub owner: String,
    pub acquired: Timestamp,
    pub doc: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Disposition {
    Prepared,
    Submitting,
    Submitted,
    Uncertain,
    Refused,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuthorizationRow {
    pub id: String,
    pub task: String,
    pub disposition: Disposition,
    pub doc: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum StorageRow {
    Scratch(ScratchRow),
    Lifecycle(LifecycleRow),
    Lease(LeaseRow),
    Claim(ClaimRow),
    Authorization(AuthorizationRow),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AggregateName {
    StatusCounts,
    UsageTotals,
    /// The session ownership summary: pane references per socket (`""` is legacy socket-less).
    PaneOwners,
    /// Reviewed, unpurged Keep registrations for exactly one task.
    ArtifactReviewed(TaskNum),
}

impl AggregateName {
    /// Board-wide families. Keyed artifact counts require the caller's task number.
    pub const ALL: [AggregateName; 3] = [Self::StatusCounts, Self::UsageTotals, Self::PaneOwners];
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusCount {
    pub status: String,
    /// `""` for tasks without a project.
    pub project: String,
    pub n: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageTotal {
    pub scope: String,
    pub key: String,
    pub doc: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneOwner {
    pub socket: String,
    pub n: u64,
}

/// The requested aggregates; families not requested stay empty.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Aggregates {
    pub status_counts: Vec<StatusCount>,
    pub usage_totals: Vec<UsageTotal>,
    pub pane_owners: Vec<PaneOwner>,
    #[serde(default)]
    pub artifact_reviewed: BTreeMap<TaskNum, u64>,
}

/// What one committed operation touched (plan section 1, change feed).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ChangeScope {
    /// The head when `parts` is empty, plus each row family written.
    Task {
        num: TaskNum,
        parts: Vec<TaskPart>,
        /// Whether a container family's history (run, review child, plan, plan review) was
        /// edited: a non-current row added, rewritten or deleted. A demotion keeps the demoted
        /// container's content and is not a history edit.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        history: bool,
        /// Whether a current container row was written as non-current: a demotion, whose
        /// container keeps its content but leaves the head.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        demoted: bool,
    },
    /// Records, claims (`claims`), leases (`leases`), authorizations (`authorizations`) and
    /// the session binding (`session`).
    Record {
        area: String,
        key: String,
    },
    Events {
        task: Option<String>,
        kind: String,
    },
    Artifact {
        task: String,
    },
    Blob {
        hash: BlobHash,
    },
    Scratch {
        id: String,
    },
    Lifecycle {
        task: String,
    },
    Condition {
        task: String,
        kind: String,
    },
    Aggregate {
        name: AggregateName,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChangeEntry {
    pub seq: ChangeSeq,
    pub at: Timestamp,
    pub op: OpKind,
    pub scopes: Vec<ChangeScope>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChangePage {
    pub entries: Vec<ChangeEntry>,
    /// Whether entries after the last returned one exist at the view's seq.
    pub more: bool,
}

/// The open board, or what changed on it after `since`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BoardDelta {
    pub seq: ChangeSeq,
    pub since: Option<ChangeSeq>,
    /// Open heads (all of them, or those changed after `since`) in task-number order.
    pub heads: Vec<TaskHeadDoc>,
    /// Tasks changed after `since` that are no longer open.
    pub left: Vec<TaskNum>,
    pub status_counts: Vec<StatusCount>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntegrityReport {
    pub problems: Vec<String>,
}

impl IntegrityReport {
    pub fn is_ok(&self) -> bool {
        self.problems.is_empty()
    }
}
