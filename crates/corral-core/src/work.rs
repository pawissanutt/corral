//! Per-thread work counters for the board work-budget benchmark (`scripts/work-budget.sh`).
//!
//! The benchmark asserts on these counts instead of wall-clock time, so it cannot flake under
//! host load. Recording is a thread-local add: helper threads and parallel tests never disturb a
//! measuring thread, and nothing is shared or locked.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;

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

/// A history-cost counter (plan T-253 section 4, "History-cost classes" and "Follow-up
/// accounting"). Each one keeps an inclusive total and one cell per bucket.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Counter {
    TaskHeadDecodes,
    HistoryRowDecodes,
    RunDecodes,
    NestedRowDecodes,
    EventDecodes,
    RecordDecodes,
    FullTaskHydrations,
    ViewsOpened,
    DecodedDocBytes,
    FileStats,
    Processes,
    TranscriptLinesAggregated,
    TranscriptBytesRead,
}

impl Counter {
    pub const ALL: [Counter; 13] = [
        Counter::TaskHeadDecodes,
        Counter::HistoryRowDecodes,
        Counter::RunDecodes,
        Counter::NestedRowDecodes,
        Counter::EventDecodes,
        Counter::RecordDecodes,
        Counter::FullTaskHydrations,
        Counter::ViewsOpened,
        Counter::DecodedDocBytes,
        Counter::FileStats,
        Counter::Processes,
        Counter::TranscriptLinesAggregated,
        Counter::TranscriptBytesRead,
    ];

    /// The decode counters: each decoded document adds one to exactly one of them.
    pub const DECODES: [Counter; 6] = [
        Counter::TaskHeadDecodes,
        Counter::HistoryRowDecodes,
        Counter::RunDecodes,
        Counter::NestedRowDecodes,
        Counter::EventDecodes,
        Counter::RecordDecodes,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Counter::TaskHeadDecodes => "task_head_decodes",
            Counter::HistoryRowDecodes => "history_row_decodes",
            Counter::RunDecodes => "run_decodes",
            Counter::NestedRowDecodes => "nested_row_decodes",
            Counter::EventDecodes => "event_decodes",
            Counter::RecordDecodes => "record_decodes",
            Counter::FullTaskHydrations => "full_task_hydrations",
            Counter::ViewsOpened => "views_opened",
            Counter::DecodedDocBytes => "decoded_doc_bytes",
            Counter::FileStats => "file_stats",
            Counter::Processes => "processes",
            Counter::TranscriptLinesAggregated => "transcript_lines_aggregated",
            Counter::TranscriptBytesRead => "transcript_bytes_read",
        }
    }
}

/// A follow-up bucket: one FU id at one call site.
pub type FollowupSite = (String, String);

/// One counter's inclusive total and its bucket cells.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Cells {
    pub total: u64,
    pub unattributed: u64,
    pub own_record: u64,
    pub emitted: u64,
    pub followup: BTreeMap<FollowupSite, u64>,
}

impl Cells {
    pub fn followup_sum(&self) -> u64 {
        self.followup.values().sum()
    }

    /// Whether the total equals the sum of the cells.
    pub fn holds(&self) -> bool {
        self.total == self.unattributed + self.own_record + self.emitted + self.followup_sum()
    }

    fn since(&self, earlier: &Cells) -> Cells {
        let mut followup = self.followup.clone();
        for (site, n) in &earlier.followup {
            let cell = followup.entry(site.clone()).or_default();
            *cell = cell.saturating_sub(*n);
        }
        followup.retain(|_, n| *n > 0);
        Cells {
            total: self.total.saturating_sub(earlier.total),
            unattributed: self.unattributed.saturating_sub(earlier.unattributed),
            own_record: self.own_record.saturating_sub(earlier.own_record),
            emitted: self.emitted.saturating_sub(earlier.emitted),
            followup,
        }
    }
}

/// What a decoded document is, so a class scope can tell whether it claims it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Subject<'a> {
    /// Anything no class scope claims by subject.
    Plain,
    /// A child row of one run of one task (`run` is the run id, `R3`).
    RunChild { task: &'a str, run: &'a str },
    /// A row an emitted reader returns verbatim.
    Emitted,
}

#[derive(Debug, Clone)]
enum Scope {
    Action { task: String, kind: String },
    OwnRecords { task: String, run: String },
    Emitted,
    Followup(FollowupSite),
}

/// One full hydration recorded inside a hot scope (debug builds only).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HydrationSite {
    pub location: &'static std::panic::Location<'static>,
    /// The innermost action scope's task and kind.
    pub action: Option<(String, String)>,
    /// The follow-up bucket that claimed it.
    pub followup: Option<FollowupSite>,
}

#[derive(Default)]
struct Ledger {
    hot: u32,
    scopes: Vec<Scope>,
    cells: BTreeMap<Counter, Cells>,
    followup_calls: BTreeMap<FollowupSite, u64>,
    sites: Vec<HydrationSite>,
}

thread_local! {
    static LEDGER: RefCell<Ledger> = RefCell::new(Ledger::default());
}

enum Bucket {
    Unattributed,
    OwnRecord,
    Emitted,
    Followup(FollowupSite),
}

impl Ledger {
    /// The innermost scope that claims an event: a follow-up claims everything, own-record
    /// scopes claim their run's child rows, emitted readers their emitted rows.
    fn bucket(&self, decode: bool, subject: Subject<'_>) -> Bucket {
        self.bucket_of(decode, false, subject)
    }

    /// `transcript`: the usage collector's transcript work, which an own-record scope claims.
    fn bucket_of(&self, decode: bool, transcript: bool, subject: Subject<'_>) -> Bucket {
        for scope in self.scopes.iter().rev() {
            match scope {
                Scope::Followup(site) => return Bucket::Followup(site.clone()),
                Scope::OwnRecords { .. } if transcript => return Bucket::OwnRecord,
                Scope::OwnRecords { task, run } if decode => {
                    if let Subject::RunChild { task: t, run: r } = subject
                        && t == task
                        && r == run
                    {
                        return Bucket::OwnRecord;
                    }
                }
                Scope::Emitted if decode && subject == Subject::Emitted => {
                    return Bucket::Emitted;
                }
                _ => {}
            }
        }
        Bucket::Unattributed
    }

    fn add(&mut self, counter: Counter, n: u64, bucket: &Bucket) {
        let cells = self.cells.entry(counter).or_default();
        cells.total += n;
        match bucket {
            Bucket::Unattributed => cells.unattributed += n,
            Bucket::OwnRecord => cells.own_record += n,
            Bucket::Emitted => cells.emitted += n,
            Bucket::Followup(site) => *cells.followup.entry(site.clone()).or_default() += n,
        }
    }

    fn action(&self) -> Option<(String, String)> {
        self.scopes.iter().rev().find_map(|scope| match scope {
            Scope::Action { task, kind } => Some((task.clone(), kind.clone())),
            _ => None,
        })
    }
}

fn with_ledger<T>(f: impl FnOnce(&mut Ledger) -> T) -> T {
    LEDGER.with(|ledger| f(&mut ledger.borrow_mut()))
}

/// Whether the current thread is inside a [`HotScope`]; nothing is counted outside one.
pub fn in_hot_scope() -> bool {
    with_ledger(|ledger| ledger.hot > 0)
}

/// Counts one decoded document of `counter` (a decode counter) with its stored bytes.
pub fn decoded(counter: Counter, bytes: u64, subject: Subject<'_>) {
    with_ledger(|ledger| {
        if ledger.hot == 0 {
            return;
        }
        let bucket = ledger.bucket(true, subject);
        ledger.add(counter, 1, &bucket);
        ledger.add(Counter::DecodedDocBytes, bytes, &bucket);
    });
}

/// Counts one full `Task` hydration; debug builds keep its call site.
#[track_caller]
pub fn hydrated() {
    hydrated_at(std::panic::Location::caller());
}

/// [`hydrated`] at a call site the store captured itself.
pub fn hydrated_at(location: &'static std::panic::Location<'static>) {
    with_ledger(|ledger| {
        if ledger.hot == 0 {
            return;
        }
        let bucket = ledger.bucket(false, Subject::Plain);
        ledger.add(Counter::FullTaskHydrations, 1, &bucket);
        if cfg!(debug_assertions) {
            let site = HydrationSite {
                location,
                action: ledger.action(),
                followup: match &bucket {
                    Bucket::Followup(site) => Some(site.clone()),
                    _ => None,
                },
            };
            ledger.sites.push(site);
        }
    });
}

/// Counts `n` events of a counter that only a follow-up scope can claim (views, file stats,
/// processes), or transcript work, which an own-record scope also claims.
pub fn counted(counter: Counter, n: u64) {
    with_ledger(|ledger| {
        if ledger.hot == 0 {
            return;
        }
        let transcript = matches!(
            counter,
            Counter::TranscriptLinesAggregated | Counter::TranscriptBytesRead
        );
        let bucket = ledger.bucket_of(false, transcript, Subject::Plain);
        ledger.add(counter, n, &bucket);
    });
}

/// Counting is on while one of these lives on the thread: a sync pass, a board frame,
/// `pm next`, `automation status` or `list`.
#[must_use = "counting stops when the scope is dropped"]
pub struct HotScope(());

impl HotScope {
    pub fn enter() -> HotScope {
        with_ledger(|ledger| ledger.hot += 1);
        HotScope(())
    }
}

impl Drop for HotScope {
    fn drop(&mut self) {
        with_ledger(|ledger| ledger.hot = ledger.hot.saturating_sub(1));
    }
}

/// Derivation preparation inside a measured pass: an evaluation prepares its derivations
/// before it enters [`HotScope`], and plan T-253 section 4 keeps that work outside the measured
/// hot path even when the evaluation runs within an enclosing measured pass, such as sync.
/// Nothing is counted until it is dropped.
#[must_use = "counting resumes when dropped"]
pub struct Preparation(u32);

impl Preparation {
    pub fn enter() -> Preparation {
        Preparation(with_ledger(|ledger| std::mem::take(&mut ledger.hot)))
    }
}

impl Drop for Preparation {
    fn drop(&mut self) {
        let hot = self.0;
        with_ledger(|ledger| ledger.hot = hot);
    }
}

/// A pushed scope, popped when dropped.
#[must_use = "the scope ends when dropped"]
pub struct ClassScope(usize);

impl Drop for ClassScope {
    fn drop(&mut self) {
        with_ledger(|ledger| ledger.scopes.truncate(self.0));
    }
}

fn push(scope: Scope) -> ClassScope {
    with_ledger(|ledger| {
        let depth = ledger.scopes.len();
        ledger.scopes.push(scope);
        ClassScope(depth)
    })
}

/// Labels the hydration sites recorded inside with the action kind and task; it attributes
/// nothing (plan section 4, "ActionScope is attribution only").
pub fn action(task: &str, kind: crate::OperatingActionKind) -> ClassScope {
    let kind = serde_json::to_value(kind)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default();
    push(Scope::Action {
        task: task.to_owned(),
        kind,
    })
}

/// Charges decodes of `run`'s child rows to the own-record cells; `label` names the call site.
pub fn own_records(label: &str, task: &str, run: &str) -> ClassScope {
    let _ = label;
    push(Scope::OwnRecords {
        task: task.to_owned(),
        run: run.to_owned(),
    })
}

/// Charges the rows an emitted reader returns to the emitted cells.
pub fn emitted() -> ClassScope {
    push(Scope::Emitted)
}

/// Charges every event inside to the follow-up bucket `(id, site)` and counts one call.
pub fn followup(id: &str, site: &str) -> ClassScope {
    let bucket = (id.to_owned(), site.to_owned());
    with_ledger(|ledger| {
        if ledger.hot > 0 {
            *ledger.followup_calls.entry(bucket.clone()).or_default() += 1;
        }
    });
    push(Scope::Followup(bucket))
}

/// The thread's counters at one moment.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkSnapshot {
    pub cells: BTreeMap<Counter, Cells>,
    pub followup_calls: BTreeMap<FollowupSite, u64>,
    pub sites: Vec<HydrationSite>,
}

/// The current thread's counters; every total equals the sum of its cells.
pub fn snapshot() -> WorkSnapshot {
    with_ledger(|ledger| WorkSnapshot {
        cells: ledger.cells.clone(),
        followup_calls: ledger.followup_calls.clone(),
        sites: ledger.sites.clone(),
    })
}

impl WorkSnapshot {
    pub fn cells(&self, counter: Counter) -> Cells {
        self.cells.get(&counter).cloned().unwrap_or_default()
    }

    /// The unattributed cell, which is what an unqualified counter name in a budget means.
    pub fn get(&self, counter: Counter) -> u64 {
        self.cells(counter).unattributed
    }

    pub fn total(&self, counter: Counter) -> u64 {
        self.cells(counter).total
    }

    /// The inclusive `full_task_hydrations`.
    pub fn full_task_hydrations(&self) -> u64 {
        self.total(Counter::FullTaskHydrations)
    }

    /// The inclusive `decoded_doc_bytes`.
    pub fn decoded_doc_bytes(&self) -> u64 {
        self.total(Counter::DecodedDocBytes)
    }

    /// `full_task_hydrations - Σ followup_hydrations`, which a hot scope requires to be 0.
    pub fn unattributed_hydrations(&self) -> u64 {
        self.get(Counter::FullTaskHydrations)
    }

    /// `decoded_doc_bytes - own_record_bytes - emitted_doc_bytes - Σ followup_doc_bytes`.
    pub fn unattributed_byte_remainder(&self) -> u64 {
        self.get(Counter::DecodedDocBytes)
    }

    fn decode_cells(&self, cell: impl Fn(&Cells) -> u64) -> u64 {
        Counter::DECODES.iter().map(|c| cell(&self.cells(*c))).sum()
    }

    pub fn own_record_rows(&self) -> u64 {
        self.decode_cells(|cells| cells.own_record)
    }

    pub fn own_record_bytes(&self) -> u64 {
        self.cells(Counter::DecodedDocBytes).own_record
    }

    pub fn emitted_rows(&self) -> u64 {
        self.decode_cells(|cells| cells.emitted)
    }

    pub fn emitted_doc_bytes(&self) -> u64 {
        self.cells(Counter::DecodedDocBytes).emitted
    }

    pub fn followup_calls(&self, id: &str, site: &str) -> u64 {
        let key = (id.to_owned(), site.to_owned());
        self.followup_calls.get(&key).copied().unwrap_or(0)
    }

    fn followup_of(&self, counter: Counter, id: &str, site: &str) -> u64 {
        let key = (id.to_owned(), site.to_owned());
        self.cells(counter).followup.get(&key).copied().unwrap_or(0)
    }

    pub fn followup_rows(&self, id: &str, site: &str) -> u64 {
        Counter::DECODES
            .iter()
            .map(|c| self.followup_of(*c, id, site))
            .sum()
    }

    pub fn followup_doc_bytes(&self, id: &str, site: &str) -> u64 {
        self.followup_of(Counter::DecodedDocBytes, id, site)
    }

    pub fn followup_hydrations(&self, id: &str, site: &str) -> u64 {
        self.followup_of(Counter::FullTaskHydrations, id, site)
    }

    pub fn followup_views(&self, id: &str, site: &str) -> u64 {
        self.followup_of(Counter::ViewsOpened, id, site)
    }

    pub fn followup_stats(&self, id: &str, site: &str) -> u64 {
        self.followup_of(Counter::FileStats, id, site)
    }

    pub fn followup_processes(&self, id: &str, site: &str) -> u64 {
        self.followup_of(Counter::Processes, id, site)
    }

    /// Fails naming every counter whose inclusive total is not the sum of its cells.
    pub fn check_identity(&self) -> Result<(), String> {
        let broken: Vec<&str> = self
            .cells
            .iter()
            .filter(|(_, cells)| !cells.holds())
            .map(|(counter, _)| counter.name())
            .collect();
        if broken.is_empty() {
            Ok(())
        } else {
            Err(format!("bucket identity broken for {}", broken.join(", ")))
        }
    }

    /// The work done between `earlier` and this snapshot.
    pub fn since(&self, earlier: &WorkSnapshot) -> WorkSnapshot {
        let mut calls = self.followup_calls.clone();
        for (site, n) in &earlier.followup_calls {
            let cell = calls.entry(site.clone()).or_default();
            *cell = cell.saturating_sub(*n);
        }
        calls.retain(|_, n| *n > 0);
        WorkSnapshot {
            cells: Counter::ALL
                .iter()
                .map(|c| (*c, self.cells(*c).since(&earlier.cells(*c))))
                .filter(|(_, cells)| *cells != Cells::default())
                .collect(),
            followup_calls: calls,
            sites: self.sites[earlier.sites.len().min(self.sites.len())..].to_vec(),
        }
    }
}
