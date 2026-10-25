//! In-memory reference implementation of the storage contract (test support, plan section 1).
//!
//! The committed state is one `Arc<State>` behind a mutex. `apply` works on a copy whose tables
//! are shared until written (`Arc::make_mut`) and swaps it in only when every write succeeded,
//! so a failed operation leaves nothing behind; a view is a clone of the `Arc`, which gives
//! real snapshot semantics. Preconditions are validated exactly as the SQLite backend does.

mod apply;
mod read;
mod storage;
mod task_ops;

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Cursor, Read};
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};

use super::support::Clock;
use super::*;
use crate::blob_ref::BlobHash;

#[derive(Debug, Clone)]
struct TaskState {
    head: TaskHeadDoc,
    rows: BTreeMap<(TaskPart, String), HistoryRow>,
}

type Table<K, V> = Arc<BTreeMap<K, V>>;

/// The stored document that holds a set of blob references; one variant per document family,
/// with structured keys so distinct identities never share an owner.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum RefOwner {
    TaskHead(TaskNum),
    Row(TaskNum, TaskPart, String),
    Event(EventSeq),
    Record(String, String),
    Authorization(String),
    Claim(String),
    UsageTotal(String, String),
}

#[derive(Debug, Clone, Default)]
struct State {
    seq: ChangeSeq,
    feed_floor: ChangeSeq,
    feed: Arc<Vec<Arc<ChangeEntry>>>,
    events: Arc<Vec<Arc<StoredEvent>>>,
    tasks: Table<TaskNum, TaskState>,
    conditions: Table<(String, String), EventSeq>,
    condition_runs: Table<EventSeq, ConditionRun>,
    records: Table<(String, String), StoredRecord>,
    claims: Table<String, ClaimRow>,
    authorizations: Table<String, AuthorizationRow>,
    blobs: Table<BlobHash, BlobRow>,
    refs: Table<RefOwner, BTreeSet<BlobHash>>,
    scratch: Table<String, ScratchRow>,
    lifecycle: Table<(String, String, String), LifecycleRow>,
    leases: Table<String, LeaseRow>,
    status_counts: Table<(String, String), u64>,
    usage_totals: Table<(String, String), Value>,
    pane_owners: Table<String, u64>,
}

struct Shared {
    state: Mutex<Arc<State>>,
    clock: Arc<dyn Clock>,
    faults: Arc<FaultInjector>,
    config: BackendConfig,
}

/// A handle on one in-memory store; [`FakeBackend::handle`] opens another handle on it.
#[derive(Clone)]
pub struct FakeBackend {
    shared: Arc<Shared>,
}

impl FakeBackend {
    pub fn new(config: BackendConfig, clock: Arc<dyn Clock>, faults: Arc<FaultInjector>) -> Self {
        Self {
            shared: Arc::new(Shared {
                state: Mutex::new(Arc::new(State::default())),
                clock,
                faults,
                config,
            }),
        }
    }

    pub fn handle(&self) -> Self {
        self.clone()
    }

    /// Drops change entries up to and including `through`, as retention would.
    pub fn prune_changes_through(&self, through: ChangeSeq) {
        let mut committed = self.lock();
        let state = Arc::make_mut(&mut committed);
        state.feed_floor = state.feed_floor.max(through);
        let floor = state.feed_floor;
        Arc::make_mut(&mut state.feed).retain(|entry| entry.seq > floor);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Arc<State>> {
        self.shared
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn snapshot(&self) -> Arc<State> {
        Arc::clone(&self.lock())
    }
}

impl StoreBackend for FakeBackend {
    fn view(&self) -> Result<Box<dyn ReadView + Send>, ViewError> {
        Ok(Box::new(read::FakeView {
            state: self.snapshot(),
            opened: self.shared.clock.now(),
            clock: Arc::clone(&self.shared.clock),
            max_age: self.shared.config.view_max_age,
        }))
    }

    fn apply(&self, op: Operation) -> Result<Committed, ApplyError> {
        let mut committed = self.lock();
        let work = State::clone(&committed);
        let (state, result) = apply::run(work, op, self.shared.clock.now(), &self.shared.faults)?;
        *committed = Arc::new(state);
        Ok(result)
    }

    fn check(&self) -> Result<IntegrityReport> {
        Ok(check_state(&self.snapshot()))
    }

    fn backup_database(&self, dest: &Path) -> Result<()> {
        let state = self.snapshot();
        let tasks: Vec<Value> = state
            .tasks
            .values()
            .map(|task| json!({"head": task.head, "rows": task.rows.values().collect::<Vec<_>>()}))
            .collect();
        let doc = json!({
            "seq": state.seq,
            "tasks": tasks,
            "events": state.events.iter().map(|e| e.as_ref()).collect::<Vec<_>>(),
            "records": state.records.values().collect::<Vec<_>>(),
            "blobs": state.blobs.values().collect::<Vec<_>>(),
        });
        let bytes = serde_json::to_vec_pretty(&doc)?;
        std::fs::write(dest, bytes).with_context(|| format!("write backup {}", dest.display()))
    }
}

fn check_state(state: &State) -> IntegrityReport {
    let mut problems = Vec::new();
    let mut counts: BTreeMap<(String, String), u64> = BTreeMap::new();
    for task in state.tasks.values() {
        let fields = &task.head.header.fields;
        let key = (
            fields.status.clone(),
            fields.project.clone().unwrap_or_default(),
        );
        *counts.entry(key).or_default() += 1;
    }
    if counts != *state.status_counts {
        problems.push(format!(
            "status counts {:?} differ from heads {counts:?}",
            state.status_counts
        ));
    }
    let mut refs: BTreeMap<&BlobHash, u64> = BTreeMap::new();
    for hashes in state.refs.values() {
        for hash in hashes {
            *refs.entry(hash).or_default() += 1;
        }
    }
    for (hash, n) in &refs {
        match state.blobs.get(*hash) {
            None => problems.push(format!("reference to missing blob {hash}")),
            Some(row) if row.refs != *n => {
                problems.push(format!("blob {hash} counts {} refs, found {n}", row.refs))
            }
            Some(row) if row.state != BlobState::Live => {
                problems.push(format!("referenced blob {hash} is {:?}", row.state))
            }
            Some(_) => {}
        }
    }
    for row in state.blobs.values() {
        if row.refs != refs.get(&row.hash).copied().unwrap_or(0) {
            problems.push(format!("blob {} counts {} refs", row.hash, row.refs));
        }
        if row.refs == 0 && row.state == BlobState::Live && row.unreferenced_since.is_none() {
            problems.push(format!(
                "unreferenced live blob {} lacks unreferenced_since",
                row.hash
            ));
        }
    }
    for ((task, kind), opened) in state.conditions.iter() {
        match state.condition_runs.get(opened) {
            Some(run) if run.closed_seq.is_none() && run.task == *task && run.kind == *kind => {}
            _ => problems.push(format!(
                "condition {task}/{kind} points at a closed or missing run"
            )),
        }
    }
    let seqs: Vec<ChangeSeq> = state.feed.iter().map(|entry| entry.seq).collect();
    if seqs.windows(2).any(|pair| pair[0] >= pair[1]) {
        problems.push("change feed is not strictly increasing".to_owned());
    }
    IntegrityReport { problems }
}

/// In-memory content-addressed blob store with the same contract as the filesystem store.
pub struct FakeBlobStore {
    files: Mutex<BTreeMap<BlobHash, (Vec<u8>, jiff::Timestamp)>>,
    clock: Arc<dyn Clock>,
}

impl FakeBlobStore {
    pub fn new(clock: Arc<dyn Clock>) -> Self {
        Self {
            files: Mutex::new(BTreeMap::new()),
            clock,
        }
    }

    /// Replaces stored content without changing its hash, as disk corruption would.
    pub fn corrupt(&self, hash: &BlobHash, bytes: Vec<u8>) {
        let now = self.clock.now();
        self.files_lock().insert(hash.clone(), (bytes, now));
    }

    fn files_lock(
        &self,
    ) -> std::sync::MutexGuard<'_, BTreeMap<BlobHash, (Vec<u8>, jiff::Timestamp)>> {
        self.files.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl BlobStore for FakeBlobStore {
    fn stage(&self, src: &mut dyn Read) -> Result<Staged> {
        let mut bytes = Vec::new();
        src.read_to_end(&mut bytes).context("read blob source")?;
        let size = u64::try_from(bytes.len())?;
        Ok(Staged {
            hash: BlobHash::of(&bytes),
            raw_bytes: size,
            stored_bytes: size,
            codec: "identity".to_owned(),
            data: StagedData::Memory(bytes),
        })
    }

    fn put_staged(&self, staged: Staged) -> Result<()> {
        let bytes = match staged.data {
            StagedData::Memory(bytes) => bytes,
            StagedData::File(path) => {
                std::fs::read(&path).with_context(|| format!("read staged {}", path.display()))?
            }
        };
        if BlobHash::of(&bytes) != staged.hash {
            bail!("staged content does not match {}", staged.hash);
        }
        let now = self.clock.now();
        self.files_lock().entry(staged.hash).or_insert((bytes, now));
        Ok(())
    }

    fn get(&self, hash: &BlobHash) -> Result<VerifiedReader> {
        let bytes = self
            .files_lock()
            .get(hash)
            .map(|(bytes, _)| bytes.clone())
            .ok_or_else(|| anyhow!("blob {hash} not found"))?;
        Ok(VerifiedReader::new(
            Box::new(Cursor::new(bytes)),
            hash.clone(),
        ))
    }

    fn delete(&self, hash: &BlobHash) -> Result<()> {
        self.files_lock().remove(hash);
        Ok(())
    }

    fn list(&self, after: Option<&BlobHash>, limit: u32) -> Result<Vec<BlobFile>> {
        let files = self.files_lock();
        Ok(files
            .iter()
            .filter(|(hash, _)| after.is_none_or(|after| *hash > after))
            .take(usize::try_from(limit)?)
            .map(|(hash, (bytes, modified))| BlobFile {
                hash: hash.clone(),
                stored_bytes: bytes.len() as u64,
                modified: *modified,
            })
            .collect())
    }
}
