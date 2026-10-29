//! Per-thread work counters for the board work-budget benchmark (`scripts/work-budget.sh`).
//!
//! The benchmark asserts on these counts instead of wall-clock time, so it cannot flake under
//! host load. Recording is a thread-local add: helper threads and parallel tests never disturb a
//! measuring thread, and nothing is shared or locked.

use std::cell::Cell;

/// One kind of store work that can grow with workspace history.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Work {
    /// A read of the config layer files (workspace `corral.toml` plus the user layer).
    ConfigRead,
    /// A schema parse of the config layers.
    ConfigParse,
    /// A task directory listing (`Store::list`).
    TaskList,
    /// A task file's metadata checked to decide whether its cached decode is current.
    TaskStat,
    /// A task file read.
    TaskRead,
    /// Bytes of task files read.
    TaskBytes,
    /// A task frontmatter decode.
    TaskParse,
    /// An open of the event log for reading.
    EventRead,
    /// Bytes of the event log read.
    EventBytes,
    /// An event line decode.
    EventParse,
    /// A read of a harness session transcript.
    TranscriptRead,
    /// A task visited deriving board state (column order, Done history, slot usage).
    BoardVisit,
    /// An unfinished task classified for attention, independent of UI board-index construction.
    AttentionVisit,
    /// A read of the store schema marker (`state/schema-version`).
    SchemaRead,
    /// A read of the durable herdr session binding (with its review-launch journal).
    SessionBindingRead,
    /// An evaluation of herdr session ownership over a task list.
    OwnershipScan,
    /// A content-refresh fingerprint pass over the task listing.
    FingerprintPass,
    /// A stat of an artifact snapshot file by the search-index refresh.
    ArtifactStat,
    /// A resource-escalation reconcile pass (it runs on every sync tick).
    ResourcePass,
    /// A size walk of one run's worktree and target directory.
    WorktreeWalk,
    /// A project's lesson signals derived from its history (event tail, artifacts, gate logs).
    LessonSignalScan,
    /// A project repository's identity resolved from the filesystem (paths, `.git` marker).
    ProjectIdentity,
    InboxRecordRead,
    InboxMessageRead,
    InboxEnvelopeRead,
    InboxList,
    InboxHeaderWrite,
    InboxIndexRead,
    /// A full resolution of the effective operating policy (grant, members and refusals).
    PolicyResolve,
}

impl Work {
    pub const ALL: [Work; 29] = [
        Work::ConfigRead,
        Work::ConfigParse,
        Work::TaskList,
        Work::TaskStat,
        Work::TaskRead,
        Work::TaskBytes,
        Work::TaskParse,
        Work::EventRead,
        Work::EventBytes,
        Work::EventParse,
        Work::TranscriptRead,
        Work::BoardVisit,
        Work::AttentionVisit,
        Work::SchemaRead,
        Work::SessionBindingRead,
        Work::OwnershipScan,
        Work::FingerprintPass,
        Work::ArtifactStat,
        Work::ResourcePass,
        Work::WorktreeWalk,
        Work::LessonSignalScan,
        Work::ProjectIdentity,
        Work::InboxRecordRead,
        Work::InboxMessageRead,
        Work::InboxEnvelopeRead,
        Work::InboxList,
        Work::InboxHeaderWrite,
        Work::InboxIndexRead,
        Work::PolicyResolve,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Work::ConfigRead => "config_reads",
            Work::ConfigParse => "config_parses",
            Work::TaskList => "task_lists",
            Work::TaskStat => "task_stats",
            Work::TaskRead => "task_reads",
            Work::TaskBytes => "task_bytes",
            Work::TaskParse => "task_parses",
            Work::EventRead => "event_reads",
            Work::EventBytes => "event_bytes",
            Work::EventParse => "event_parses",
            Work::TranscriptRead => "transcript_reads",
            Work::BoardVisit => "board_visits",
            Work::AttentionVisit => "attention_visits",
            Work::SchemaRead => "schema_reads",
            Work::SessionBindingRead => "session_binding_reads",
            Work::OwnershipScan => "ownership_scans",
            Work::FingerprintPass => "fingerprint_passes",
            Work::ArtifactStat => "artifact_stats",
            Work::ResourcePass => "resource_passes",
            Work::WorktreeWalk => "worktree_walks",
            Work::LessonSignalScan => "lesson_signal_scans",
            Work::ProjectIdentity => "project_identities",
            Work::InboxRecordRead => "inbox_record_reads",
            Work::InboxMessageRead => "inbox_message_reads",
            Work::InboxEnvelopeRead => "inbox_envelope_reads",
            Work::InboxList => "inbox_lists",
            Work::InboxHeaderWrite => "inbox_header_writes",
            Work::InboxIndexRead => "inbox_index_reads",
            Work::PolicyResolve => "policy_resolves",
        }
    }
}

thread_local! {
    static COUNTS: [Cell<u64>; Work::ALL.len()] = const { [const { Cell::new(0) }; Work::ALL.len()] };
}

/// Adds `n` units of `work` on the current thread.
pub fn record(work: Work, n: u64) {
    COUNTS.with(|counts| {
        let cell = &counts[work as usize];
        cell.set(cell.get().saturating_add(n));
    });
}

/// The current thread's running total for `work`.
pub fn count(work: Work) -> u64 {
    COUNTS.with(|counts| counts[work as usize].get())
}
