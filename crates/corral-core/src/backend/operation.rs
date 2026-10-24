//! Atomic logical operations (plan T-253 sections 3, 6-9 and 12).
//!
//! Every [`Operation`] commits all of its writes and one change-feed entry, or nothing. Each
//! variant documents its preconditions; a stale expected revision, version or `from` state is a
//! [`super::ApplyError::Conflict`], any other unmet precondition a
//! [`super::ApplyError::Precondition`]. Blob references are not passed in: the backend scans
//! every document it stores with [`crate::blob_ref::scan`] and keeps the reference set exact.

use std::time::Duration;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::rows::{
    Disposition, HistoryRow, LifecycleOutcome, NewEvent, ScratchKind, ScratchState, StoredRecord,
    TaskFields, TaskHeader, TaskNum, TaskPart,
};
use crate::blob_ref::{BlobHash, BlobRef};

/// Event kinds that form condition runs (plan section 9).
pub const CONDITION_KINDS: [&str; 5] = [
    "dispatch_failed",
    "queued",
    "landing_stale",
    "autopilot_error",
    "main_movement_notice",
];

/// The `storage-gc` claim every blob deletion requires (plan section 7).
pub const STORAGE_GC_CLAIM: &str = "storage-gc";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpKind {
    CreateTask,
    UpdateTask,
    AppendEvents,
    Comment,
    Done,
    Verdict,
    PlanSubmit,
    PlanVerdict,
    GateStart,
    GateLaunched,
    GateResult,
    PromoteArtifacts,
    PurgeArtifact,
    Authorization,
    Claim,
    ReleaseClaim,
    PutRecords,
    BindSession,
    ObserveCondition,
    ClearCondition,
    BlobReserve,
    BlobBeginDelete,
    BlobFinishDelete,
    BlobAdoptStray,
    ScratchRegister,
    ScratchClose,
    ScratchDefer,
    ScratchWarn,
    ScratchBeginReclaim,
    ScratchFinishReclaim,
    Lifecycle,
    Lease,
    ImportBatch,
}

impl OpKind {
    pub const ALL: [OpKind; 33] = [
        Self::CreateTask,
        Self::UpdateTask,
        Self::AppendEvents,
        Self::Comment,
        Self::Done,
        Self::Verdict,
        Self::PlanSubmit,
        Self::PlanVerdict,
        Self::GateStart,
        Self::GateLaunched,
        Self::GateResult,
        Self::PromoteArtifacts,
        Self::PurgeArtifact,
        Self::Authorization,
        Self::Claim,
        Self::ReleaseClaim,
        Self::PutRecords,
        Self::BindSession,
        Self::ObserveCondition,
        Self::ClearCondition,
        Self::BlobReserve,
        Self::BlobBeginDelete,
        Self::BlobFinishDelete,
        Self::BlobAdoptStray,
        Self::ScratchRegister,
        Self::ScratchClose,
        Self::ScratchDefer,
        Self::ScratchWarn,
        Self::ScratchBeginReclaim,
        Self::ScratchFinishReclaim,
        Self::Lifecycle,
        Self::Lease,
        Self::ImportBatch,
    ];
}

/// A reservation an operation consumes: referencing a `writing` blob requires its token, and
/// the operation turns the blob `live` and releases the reservation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReservationUse {
    pub hash: BlobHash,
    pub token: String,
}

/// A record write. `expected: None` requires the record to be absent, `Some(v)` its version
/// to be `v`; `doc: None` deletes it. The stored version becomes `expected + 1` (1 on insert).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecordWrite {
    pub area: String,
    pub key: String,
    pub expected: Option<u64>,
    pub doc: Option<Value>,
    pub updated: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RowWrite {
    Put(HistoryRow),
    Delete { part: TaskPart, key: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AggregateWrite {
    UsageTotal {
        scope: String,
        key: String,
        doc: Value,
    },
    /// Adjusts one socket's pane-reference count; a count may never go below zero.
    PaneOwner { socket: String, delta: i64 },
}

/// One write of one existing task, carried by every task-writing operation. Requires the task
/// to exist at revision `expected_rev`; commits the new header fields, head document, summary,
/// row writes, events, record writes and aggregate writes, and bumps the revision by one.
/// Status counts follow the header change without an aggregate write.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskChange {
    pub num: TaskNum,
    pub expected_rev: u64,
    pub fields: TaskFields,
    pub doc: Value,
    pub summary: Value,
    pub rows: Vec<RowWrite>,
    pub events: Vec<NewEvent>,
    pub records: Vec<RecordWrite>,
    pub aggregates: Vec<AggregateWrite>,
    pub reservations: Vec<ReservationUse>,
}

/// Head form writes only new rows or rows that are currently `current` and deletes nothing;
/// full form writes or deletes any row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UpdateForm {
    Head,
    Full,
}

/// Allocates the next task number (replacing `lock/next-id`) and the id `<prefix>-<num:03>`,
/// and commits the head, rows and events. Events without a task get the new id. Requires a
/// non-empty uppercase ASCII prefix and unique row keys.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateTask {
    pub id_prefix: String,
    pub created: Timestamp,
    pub fields: TaskFields,
    pub doc: Value,
    pub summary: Value,
    pub rows: Vec<HistoryRow>,
    pub events: Vec<NewEvent>,
    pub reservations: Vec<ReservationUse>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UpdateTask {
    pub change: TaskChange,
    pub form: UpdateForm,
}

/// Appends events; no precondition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppendEvents {
    pub events: Vec<NewEvent>,
    pub reservations: Vec<ReservationUse>,
}

/// Appends the comment row under the next comment key with the change. Requires a
/// `commented` event in the change and no comment row written by the change itself.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Comment {
    pub change: TaskChange,
    pub comment: Value,
}

/// Records a run's outcome. Requires the run row to exist, be current and open, the new status
/// to be `review` and a `done` event; the run row becomes closed with `run_doc`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Done {
    pub change: TaskChange,
    pub run_key: String,
    pub run_doc: Value,
}

/// Records a review. Requires the stored status to be `review`, the review key to be new, a
/// `verdict` event, and every scratch dir in `close_scratch` to be open (they become closed).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Verdict {
    pub change: TaskChange,
    pub review_key: String,
    pub review_doc: Value,
    pub close_scratch: Vec<String>,
}

/// Adds a plan as the current plan, demoting the previous one. Requires a new plan key and at
/// least one event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlanSubmit {
    pub change: TaskChange,
    pub plan_key: String,
    pub plan_doc: Value,
}

/// Records a plan review. Requires the plan to exist and be current, a new review key under it,
/// at least one event and open scratch dirs in `close_scratch`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlanVerdict {
    pub change: TaskChange,
    pub plan_key: String,
    pub review_key: String,
    pub review_doc: Value,
    pub close_scratch: Vec<String>,
}

/// One gate attempt row (T-251 tier and identity bytes as blob references). Attempt ids are
/// text so request-derived `ag-...` ids are kept unchanged.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GateAttempt {
    pub run: String,
    pub attempt: String,
    pub scope: String,
    pub sha: String,
    pub tier: Option<String>,
    pub identity: BlobRef,
    pub commands: BlobRef,
    pub context: Option<BlobRef>,
    pub launched: Option<Value>,
    pub result: Option<String>,
    pub failed_cmd: Option<String>,
    pub log: Option<BlobRef>,
    pub result_doc: Option<BlobRef>,
    pub started: Timestamp,
    pub ended: Option<Timestamp>,
}

impl GateAttempt {
    pub fn key(&self) -> String {
        gate_key(&self.run, &self.attempt)
    }
}

pub fn gate_key(run: &str, attempt: &str) -> String {
    format!("{run}/{attempt}")
}

/// Starts a gate attempt. Requires a new `(run, attempt)`, an unended attempt and an event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GateStart {
    pub change: TaskChange,
    pub attempt: GateAttempt,
}

/// Records the gate pane. Requires the attempt to exist, be unended and not yet launched.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GateLaunched {
    pub change: TaskChange,
    pub run: String,
    pub attempt: String,
    pub pane: Value,
}

/// Ends a gate attempt. Requires the attempt to exist and be unended, and open scratch dirs
/// in `close_scratch`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GateResult {
    pub change: TaskChange,
    pub run: String,
    pub attempt: String,
    pub result: String,
    pub failed_cmd: Option<String>,
    pub log: Option<BlobRef>,
    pub result_doc: Option<BlobRef>,
    pub context: Option<BlobRef>,
    pub ended: Timestamp,
    pub close_scratch: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArtifactRow {
    pub seq: u64,
    pub run: String,
    pub kind: String,
    pub title: String,
    pub name: String,
    pub sha256: String,
    pub bytes: u64,
    pub blob: Option<BlobRef>,
    pub tier_override: Option<String>,
    pub pinned: bool,
    pub added: Timestamp,
    pub purged_at: Option<Timestamp>,
}

pub fn artifact_key(seq: u64) -> String {
    format!("{seq:010}")
}

/// Adds artifact rows. Requires new artifact seqs and an `artifact` event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromoteArtifacts {
    pub change: TaskChange,
    pub artifacts: Vec<ArtifactRow>,
}

/// Purges an artifact's bytes, leaving a stub row without its blob. Requires the artifact to
/// exist and not be purged.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PurgeArtifact {
    pub change: TaskChange,
    pub seq: u64,
    pub purged_at: Timestamp,
}

/// Moves an authorized submission from `from` (`None`: absent) to `to`. Legal transitions:
/// absent -> Prepared, Prepared -> Submitting (the only one that may carry a task change),
/// Prepared -> Refused, Submitting -> Submitted | Uncertain. A stored disposition other than
/// `from` is a conflict.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Authorization {
    pub id: String,
    pub task: String,
    pub from: Option<Disposition>,
    pub to: Disposition,
    pub doc: Value,
    pub change: Option<(TaskChange, UpdateForm)>,
}

/// Insert-if-absent; an existing claim of that name is a conflict.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Claim {
    pub name: String,
    pub owner: String,
    pub acquired: Timestamp,
    pub doc: Value,
}

/// Deletes a claim; requires it to exist and be held by `owner`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReleaseClaim {
    pub name: String,
    pub owner: String,
}

/// Record writes with expected versions, plus events.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PutRecords {
    pub writes: Vec<RecordWrite>,
    pub events: Vec<NewEvent>,
    pub reservations: Vec<ReservationUse>,
}

/// Area and key of the session binding record.
pub const SESSION_AREA: &str = "session";
pub const SESSION_KEY: &str = "binding";

/// Writes the session binding `{socket, binding}`; requires it absent or bound to `socket`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BindSession {
    pub socket: String,
    pub binding: Value,
    pub at: Timestamp,
}

/// Observes a condition (plan section 9): an open run with the same canonical detail digest
/// counts the repeat without an event; otherwise one event opens a new run and closes the old
/// one as `changed`. Requires a kind of [`CONDITION_KINDS`]; `task` is `""` for workspace kinds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ObserveCondition {
    pub task: String,
    pub kind: String,
    pub detail: Value,
    pub at: Timestamp,
    pub actor: Option<String>,
    pub via: Option<String>,
}

/// Clears a condition: with an open run, appends `<kind>_cleared` carrying `{run, count,
/// first_at, last_at}` and closes the run as `cleared`; without one, commits no write.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClearCondition {
    pub task: String,
    pub kind: String,
    pub at: Timestamp,
    pub actor: Option<String>,
    pub via: Option<String>,
}

/// Blob row transitions (plan section 7).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum BlobOp {
    /// Absent row: inserts `writing` and returns MustWrite. `live` or `writing`: adds or extends
    /// this token's reservation and returns Exists. `deleting`: returns Busy, writing nothing.
    Reserve {
        hash: BlobHash,
        raw_bytes: u64,
        stored_bytes: u64,
        codec: String,
        token: String,
        owner: String,
        ttl: Duration,
    },
    /// `live` (or `writing` whose reservations all expired) -> `deleting`. Requires the
    /// `storage-gc` claim held by `gc_owner`, no references, no unexpired reservation and
    /// `unreferenced_since` (or `created`) before `horizon`.
    BeginDelete {
        hash: BlobHash,
        gc_owner: String,
        horizon: Timestamp,
    },
    /// `deleting` -> row removed. Requires the `storage-gc` claim held by `gc_owner`.
    FinishDelete { hash: BlobHash, gc_owner: String },
    /// Inserts a `deleting` row for a stray file. Requires the claim and no row for the hash.
    AdoptStray {
        hash: BlobHash,
        gc_owner: String,
        stored_bytes: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ReclaimResult {
    Reclaimed { bytes: u64 },
    Failed { reason: String },
}

/// Scratch directory transitions (plan section 8).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ScratchOp {
    /// Inserts an `open` dir. Requires a new id, a new path, and a relative path without `..`.
    Register {
        id: String,
        kind: ScratchKind,
        task: Option<String>,
        subject: Option<String>,
        attempt: Option<String>,
        path: String,
        at: Timestamp,
    },
    /// `open` -> `closed`.
    Close { id: String, at: Timestamp },
    /// `from` (closed or deferred) -> `deferred(reason, next_check_at)`.
    Defer {
        id: String,
        from: ScratchState,
        reason: String,
        next_check_at: Timestamp,
    },
    /// Records the one `storage_scratch_warning` of an attempt; requires it not yet warned.
    Warn {
        id: String,
        evidence_bytes: u64,
        at: Timestamp,
    },
    /// `from` (closed, or deferred whose next check is due) -> `reclaiming` owned by `owner`.
    BeginReclaim {
        id: String,
        from: ScratchState,
        owner: String,
        at: Timestamp,
    },
    /// `reclaiming` (held by `owner`) -> `reclaimed` with one `storage_reclaimed` event, or
    /// `failed` with one `storage_reclaim_failed` event.
    FinishReclaim {
        id: String,
        owner: String,
        result: ReclaimResult,
        at: Timestamp,
    },
}

/// Writes a lifecycle row keyed `(task, generation, target)`; `expected` is the stored
/// outcome the writer read (`None`: absent).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LifecycleWrite {
    pub task: String,
    pub generation: String,
    pub target: String,
    pub expected: Option<LifecycleOutcome>,
    pub outcome: LifecycleOutcome,
    pub policy_digest: String,
    pub reason: Option<String>,
    pub bytes: Option<u64>,
    pub next_check_at: Option<Timestamp>,
    pub at: Timestamp,
}

/// Storage lease transitions (readers that outlive the blob grace period).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum LeaseOp {
    /// Requires the lease absent or expired.
    Acquire {
        id: String,
        holder: String,
        expires: Timestamp,
    },
    /// Requires the lease held by `holder`.
    Renew {
        id: String,
        holder: String,
        expires: Timestamp,
    },
    /// Requires the lease held by `holder`.
    Release { id: String, holder: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImportedTask {
    pub header: TaskHeader,
    pub doc: Value,
    pub summary: Value,
    pub rows: Vec<HistoryRow>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImportedEvent {
    pub event: NewEvent,
    pub malformed: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImportedBlob {
    pub hash: BlobHash,
    pub raw_bytes: u64,
    pub stored_bytes: u64,
    pub codec: String,
}

/// Migration-only bulk insert: blobs become `live`, tasks keep their numbers, ids and
/// revisions, events get seqs in order. Requires every task number and id to be new.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImportBatch {
    pub blobs: Vec<ImportedBlob>,
    pub tasks: Vec<ImportedTask>,
    pub events: Vec<ImportedEvent>,
    pub records: Vec<StoredRecord>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Operation {
    CreateTask(CreateTask),
    UpdateTask(UpdateTask),
    AppendEvents(AppendEvents),
    Comment(Comment),
    Done(Done),
    Verdict(Verdict),
    PlanSubmit(PlanSubmit),
    PlanVerdict(PlanVerdict),
    GateStart(GateStart),
    GateLaunched(GateLaunched),
    GateResult(GateResult),
    PromoteArtifacts(PromoteArtifacts),
    PurgeArtifact(PurgeArtifact),
    Authorization(Authorization),
    Claim(Claim),
    ReleaseClaim(ReleaseClaim),
    PutRecords(PutRecords),
    BindSession(BindSession),
    ObserveCondition(ObserveCondition),
    ClearCondition(ClearCondition),
    Blob(BlobOp),
    Scratch(ScratchOp),
    Lifecycle(LifecycleWrite),
    Lease(LeaseOp),
    ImportBatch(ImportBatch),
}

impl Operation {
    pub fn kind(&self) -> OpKind {
        match self {
            Self::CreateTask(_) => OpKind::CreateTask,
            Self::UpdateTask(_) => OpKind::UpdateTask,
            Self::AppendEvents(_) => OpKind::AppendEvents,
            Self::Comment(_) => OpKind::Comment,
            Self::Done(_) => OpKind::Done,
            Self::Verdict(_) => OpKind::Verdict,
            Self::PlanSubmit(_) => OpKind::PlanSubmit,
            Self::PlanVerdict(_) => OpKind::PlanVerdict,
            Self::GateStart(_) => OpKind::GateStart,
            Self::GateLaunched(_) => OpKind::GateLaunched,
            Self::GateResult(_) => OpKind::GateResult,
            Self::PromoteArtifacts(_) => OpKind::PromoteArtifacts,
            Self::PurgeArtifact(_) => OpKind::PurgeArtifact,
            Self::Authorization(_) => OpKind::Authorization,
            Self::Claim(_) => OpKind::Claim,
            Self::ReleaseClaim(_) => OpKind::ReleaseClaim,
            Self::PutRecords(_) => OpKind::PutRecords,
            Self::BindSession(_) => OpKind::BindSession,
            Self::ObserveCondition(_) => OpKind::ObserveCondition,
            Self::ClearCondition(_) => OpKind::ClearCondition,
            Self::Blob(BlobOp::Reserve { .. }) => OpKind::BlobReserve,
            Self::Blob(BlobOp::BeginDelete { .. }) => OpKind::BlobBeginDelete,
            Self::Blob(BlobOp::FinishDelete { .. }) => OpKind::BlobFinishDelete,
            Self::Blob(BlobOp::AdoptStray { .. }) => OpKind::BlobAdoptStray,
            Self::Scratch(ScratchOp::Register { .. }) => OpKind::ScratchRegister,
            Self::Scratch(ScratchOp::Close { .. }) => OpKind::ScratchClose,
            Self::Scratch(ScratchOp::Defer { .. }) => OpKind::ScratchDefer,
            Self::Scratch(ScratchOp::Warn { .. }) => OpKind::ScratchWarn,
            Self::Scratch(ScratchOp::BeginReclaim { .. }) => OpKind::ScratchBeginReclaim,
            Self::Scratch(ScratchOp::FinishReclaim { .. }) => OpKind::ScratchFinishReclaim,
            Self::Lifecycle(_) => OpKind::Lifecycle,
            Self::Lease(_) => OpKind::Lease,
            Self::ImportBatch(_) => OpKind::ImportBatch,
        }
    }

    /// The task write this operation carries, if it writes an existing task.
    pub fn task_change(&self) -> Option<&TaskChange> {
        match self {
            Self::UpdateTask(op) => Some(&op.change),
            Self::Comment(op) => Some(&op.change),
            Self::Done(op) => Some(&op.change),
            Self::Verdict(op) => Some(&op.change),
            Self::PlanSubmit(op) => Some(&op.change),
            Self::PlanVerdict(op) => Some(&op.change),
            Self::GateStart(op) => Some(&op.change),
            Self::GateLaunched(op) => Some(&op.change),
            Self::GateResult(op) => Some(&op.change),
            Self::PromoteArtifacts(op) => Some(&op.change),
            Self::PurgeArtifact(op) => Some(&op.change),
            Self::Authorization(op) => op.change.as_ref().map(|(change, _)| change),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReserveOutcome {
    MustWrite,
    Exists,
    Busy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConditionOutcome {
    Repeated {
        opened_seq: super::EventSeq,
        count: u64,
    },
    Opened {
        opened_seq: super::EventSeq,
        closed: Option<super::EventSeq>,
    },
    Cleared {
        event: super::EventSeq,
    },
    NotOpen,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Outcome {
    Created { num: TaskNum, id: String },
    Task { num: TaskNum, rev: u64 },
    Reserved(ReserveOutcome),
    Condition(ConditionOutcome),
    Applied,
}

/// A committed operation: its change-feed position, its outcome and the seqs of the events
/// it appended, in order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Committed {
    pub seq: super::ChangeSeq,
    pub outcome: Outcome,
    pub events: Vec<super::EventSeq>,
}
