//! Explicit, durable ownership migration for references predating saved sockets.

use std::cell::Cell;
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::thread::ThreadId;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::backend::{
    AggregateName, PageReq, PaneOwner, ReadView, RecordQuery, SESSION_AREA, SESSION_KEY,
};
use crate::{Store, StoreLock, Task};

/// Launch provenance supplied by the operator, never inferred from current config.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HerdrSessionBinding {
    pub socket: PathBuf,
    pub launch_evidence: String,
    pub bound_at: Timestamp,
}

/// Identifies one shared task-list snapshot; holding it keeps the address from naming another.
type ListKey = Weak<Vec<Task>>;

/// Shared by pane-inventory readers, exclusive for launches and bindings.
const OWNERSHIP: &str = "session-ownership";
/// `lock/<WAITING>/` holds one exclusively locked file per launch waiting for `OWNERSHIP`.
/// Registering one takes no lock a reader holds; readers that find a live one queue behind it.
const WAITING: &str = "session-ownership-waiting";
const POLL: Duration = Duration::from_millis(10);

thread_local! {
    /// Depth of the `UnqueuedReads` scopes open on this thread.
    static UNQUEUED: Cell<usize> = const { Cell::new(0) };
}

/// One pass's single read of the session binding and the auxiliary-launch journal.
/// It lives only as long as that pass; nothing carries it to a later pass or process.
pub struct SessionView {
    pub binding: Option<HerdrSessionBinding>,
    /// [`Store::review_launch_socket`] as this pass read it.
    pub pending: Option<Option<PathBuf>>,
    /// Counts from the same committed view as the binding and launch journal.
    pub owners: Vec<PaneOwner>,
    source: Mutex<Box<dyn ReadView + Send>>,
    ownership: Mutex<Vec<(PathBuf, bool)>>,
    /// Ownership verdicts by shared task-list snapshot and selected herdr session.
    verdicts: Mutex<Vec<(ListKey, Option<String>, bool)>>,
}

impl std::fmt::Debug for SessionView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionView")
            .field("binding", &self.binding)
            .field("pending", &self.pending)
            .field("owners", &self.owners)
            .finish_non_exhaustive()
    }
}

impl SessionView {
    /// Checks a pass's bounded ownership summary once per selected socket.
    pub fn owns(&self, socket: &Path) -> Result<bool> {
        guard(&self.source).ensure_fresh()?;
        if let Some((_, owned)) = guard(&self.ownership).iter().find(|(s, _)| s == socket) {
            return Ok(*owned);
        }
        crate::work::record(crate::work::Work::OwnershipScan, 1);
        let legacy = self
            .binding
            .as_ref()
            .map(|binding| binding.socket.as_path());
        let owns = |saved: Option<&Path>| saved.or(legacy) == Some(socket);
        let owned = legacy.is_none_or(|bound| bound == socket)
            && self.owners.iter().all(|owner| {
                owner.n == 0 || owns((!owner.socket.is_empty()).then(|| Path::new(&owner.socket)))
            })
            && self
                .pending
                .as_ref()
                .is_none_or(|saved| owns(saved.as_deref()));
        guard(&self.ownership).push((socket.to_path_buf(), owned));
        Ok(owned)
    }

    /// The verdict for one shared task-list snapshot, computed by `scan` once per view.
    /// The held `Weak` keeps the list's allocation, so its address cannot name a later list.
    pub fn verdict(
        &self,
        tasks: &Arc<Vec<Task>>,
        session: Option<&str>,
        scan: impl FnOnce() -> Result<bool>,
    ) -> Result<bool> {
        let key = Arc::downgrade(tasks);
        let known = guard(&self.verdicts)
            .iter()
            .find(|(list, s, _)| list.ptr_eq(&key) && s.as_deref() == session)
            .map(|(_, _, owned)| *owned);
        if let Some(owned) = known {
            return Ok(owned);
        }
        let owned = scan()?;
        guard(&self.verdicts).push((key, session.map(Into::into), owned));
        Ok(owned)
    }

    /// The verdict already computed for a shared snapshot holding exactly these tasks, such
    /// as a caller's copy of that listing. Comparison is in memory; nothing is read.
    pub fn known_verdict(&self, tasks: &[Task], session: Option<&str>) -> Option<bool> {
        guard(&self.verdicts).iter().find_map(|(list, s, owned)| {
            let list = list.upgrade()?;
            (s.as_deref() == session && list.as_slice() == tasks).then_some(*owned)
        })
    }
}

/// The view of the pass begun on this store handle. A clone starts without one, so the view
/// ends with the handle that began it: sync's per-pass context, dropped after the pass.
#[derive(Debug, Default)]
pub(crate) struct HandlePass(Mutex<Option<Arc<SessionView>>>);

impl Clone for HandlePass {
    fn clone(&self) -> Self {
        Self::default()
    }
}

/// Serializes session-view derivation; the snapshot itself owns the cached value.
#[derive(Debug, Default)]
pub(crate) struct SnapshotView(Mutex<()>);

fn guard<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Store {
    /// A fresh read of the binding and journal, as the launch boundary requires.
    pub fn read_session_view(&self) -> Result<Arc<SessionView>> {
        let source = self.view()?;
        crate::work::record(crate::work::Work::SessionBindingRead, 1);
        let binding = binding_on(source.as_ref())?;
        let pending = journal_on(source.as_ref())?;
        let owners = source.aggregates(&[AggregateName::PaneOwners])?.pane_owners;
        Ok(Arc::new(SessionView {
            binding,
            pending,
            owners,
            source: Mutex::new(source),
            ownership: Mutex::default(),
            verdicts: Mutex::default(),
        }))
    }

    /// This pass's view: the pass begun on this handle, else this read-only snapshot's,
    /// else a fresh read. Outside a pass every call reads afresh.
    pub fn session_view(&self) -> Result<Arc<SessionView>> {
        if let Some(view) = guard(&self.session_pass.0).as_ref() {
            guard(&view.source).ensure_fresh()?;
            return Ok(Arc::clone(view));
        }
        if !self.is_snapshot() {
            return self.read_session_view();
        }
        let _derive = guard(&self.snapshot_view.0);
        let view = self.pass_derived("session-ownership", || self.read_session_view())?;
        guard(&view.source).ensure_fresh()?;
        Ok(Arc::clone(view.as_ref()))
    }

    /// Begins this handle's pass without the lease, reading the binding once, so a pass's
    /// earlier guards share one read. The lease taken later by `try_begin_session_pass`
    /// replaces this view with a fresh one, which is the re-check inventory needs.
    pub fn begin_session_view(&self) -> Result<()> {
        *guard(&self.session_pass.0) = Some(self.read_session_view()?);
        Ok(())
    }

    /// Takes the shared session-ownership lease and begins this handle's pass under it,
    /// reading the binding once: no bind can commit while the lease is held. The view serves
    /// the rest of the pass after the lease drops; launches still re-read at their boundary.
    /// A reader that arrives while a launch waits queues behind it and takes the lease once
    /// that launch is done; the launch's own bounded wait and claim bound the queue. None while
    /// a launch or bind holds the lease without having waited. On a thread holding the claim,
    /// the pass runs under that claim, and the lease keeps it until both are dropped.
    pub fn try_begin_session_pass(&self) -> Result<Option<SessionLease>> {
        let Some(lease) = self.session_lease()? else {
            return Ok(None);
        };
        *guard(&self.session_pass.0) = Some(self.read_session_view()?);
        Ok(Some(lease))
    }

    /// Refresh an explicitly begun pass for inventory, without installing a persistent
    /// read view on a reusable context that did not begin a pass.
    pub fn try_inventory_session_pass(&self) -> Result<Option<SessionLease>> {
        let Some(lease) = self.session_lease()? else {
            return Ok(None);
        };
        let mut pass = guard(&self.session_pass.0);
        if pass.is_some() {
            *pass = Some(self.read_session_view()?);
        }
        Ok(Some(lease))
    }

    fn session_lease(&self) -> Result<Option<SessionLease>> {
        // The claim already excludes binds and every other launch; the lease keeps it alive.
        if let Some(claim) = held_claim(self.dir()) {
            return Ok(Some(SessionLease {
                _lock: None,
                _claim: Some(claim),
            }));
        }
        let queue = UNQUEUED.with(Cell::get) == 0 && self.config.sync.launch_wait_secs > 0;
        loop {
            if !launch_waiting(self.dir())? {
                let Some(lock) = self.try_lock_shared(OWNERSHIP)? else {
                    return Ok(None);
                };
                // A launch that registered while this lease was taken still goes first.
                if !launch_waiting(self.dir())? {
                    return Ok(Some(SessionLease {
                        _lock: Some(lock),
                        _claim: None,
                    }));
                }
            }
            if !queue {
                return Ok(None);
            }
            #[cfg(test)]
            queue_probe::note(self.dir());
            std::thread::sleep(POLL);
        }
    }

    /// The exclusive session-ownership claim every launch and binding takes. A launch that
    /// cannot claim at once registers its turn, so readers arriving later queue behind it, and
    /// waits up to `sync.launch_wait_secs` for readers already holding the lease. None once the
    /// bound passes, as the no-wait claim was while a reader held it.
    pub fn claim_session_ownership(&self) -> Result<Option<SessionClaim>> {
        if let Some(claim) = self.try_claim_session_ownership()? {
            return Ok(Some(claim));
        }
        let wait = self.config.sync.launch_wait_secs;
        if wait == 0 {
            return Ok(None);
        }
        let deadline = deadline(wait);
        let turn = register_turn(self.dir())?;
        loop {
            if let Some(lock) = self.try_lock(OWNERSHIP)? {
                return Ok(Some(SessionClaim::new(self.dir(), lock, Some(turn))));
            }
            if !pause(deadline) {
                return Ok(None);
            }
        }
    }

    /// The claim only if it is free now: no wait and no turn.
    pub fn try_claim_session_ownership(&self) -> Result<Option<SessionClaim>> {
        Ok(self
            .try_lock(OWNERSHIP)?
            .map(|lock| SessionClaim::new(self.dir(), lock, None)))
    }

    /// The auxiliary-launch journal binds ownership before external pane creation.
    /// An existing journal without a socket has unknown legacy ownership.
    pub fn review_launch_socket(&self) -> Result<Option<Option<PathBuf>>> {
        Ok(self
            .record::<JournalOwnership>("review-launch", "current")?
            .map(|owner| owner.socket))
    }

    /// This is durable ownership, not rebuildable `state/` data.
    pub fn herdr_session_binding(&self) -> Result<Option<HerdrSessionBinding>> {
        crate::work::record(crate::work::Work::SessionBindingRead, 1);
        binding_on(self.view()?.as_ref())
    }

    /// Explicit migration: the caller attests that the evidence identifies the launch
    /// server of every legacy pane in this store. Existing ownership cannot be replaced.
    pub fn bind_herdr_session(&self, socket: &Path, launch_evidence: &str) -> Result<()> {
        if !socket.is_absolute() || launch_evidence.trim().is_empty() {
            bail!("binding requires an absolute socket path and nonempty launch evidence");
        }
        self.check_runtime()?;
        let _ownership = self
            .try_claim_session_ownership()?
            .context("pane inventory is being reconciled; retry binding")?;
        let _board = self.lock("dispatch")?;
        let view = self.read_session_view()?;
        if let Some(binding) = &view.binding {
            if binding.socket != socket {
                bail!("store is already bound to {}", binding.socket.display());
            }
            return Ok(());
        }
        if let Some(saved) = &view.pending {
            check_socket(saved.as_deref(), socket)?;
        }
        for owner in &view.owners {
            if owner.n != 0 && !owner.socket.is_empty() {
                check_socket(Some(Path::new(&owner.socket)), socket)?;
            }
        }
        let binding = HerdrSessionBinding {
            socket: socket.to_path_buf(),
            launch_evidence: launch_evidence.into(),
            bound_at: crate::clock::now(),
        };
        self.bind_session_record(&binding)
    }
}

/// One held claim: the thread that took it, its id, the store it claims and the claim.
type ClaimEntry = (ThreadId, u64, PathBuf, Weak<ClaimInner>);

/// Claims held now, by the thread that took them and the store they claim: that thread's
/// session passes run under its claim.
static CLAIMS: Mutex<Vec<ClaimEntry>> = Mutex::new(Vec::new());

/// An exclusive session-ownership claim. A launch that waited keeps its turn until the claim
/// is released; the lease goes first, so no reader finds it held with no turn to queue behind.
/// A pass begun under it shares it, so ownership lasts until both are dropped.
pub struct SessionClaim {
    _inner: Arc<ClaimInner>,
}

struct ClaimInner {
    id: u64,
    _lock: StoreLock,
    _turn: Option<Turn>,
}

impl SessionClaim {
    fn new(corral: &Path, lock: StoreLock, turn: Option<Turn>) -> SessionClaim {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let inner = Arc::new(ClaimInner {
            id: NEXT.fetch_add(1, Ordering::Relaxed),
            _lock: lock,
            _turn: turn,
        });
        guard(&CLAIMS).push((
            std::thread::current().id(),
            inner.id,
            corral.to_path_buf(),
            Arc::downgrade(&inner),
        ));
        SessionClaim { _inner: inner }
    }
}

impl Drop for ClaimInner {
    fn drop(&mut self) {
        guard(&CLAIMS).retain(|(_, id, _, _)| *id != self.id);
    }
}

/// This thread's live claim on `corral`. The upgraded claim leaves the registry guard before it
/// can be dropped, since dropping the last one takes that guard.
fn held_claim(corral: &Path) -> Option<Arc<ClaimInner>> {
    let thread = std::thread::current().id();
    let claims = guard(&CLAIMS);
    claims
        .iter()
        .filter(|(holder, _, dir, _)| *holder == thread && dir.as_path() == corral)
        .find_map(|(_, _, _, claim)| claim.upgrade())
}

/// The shared session-ownership lease of one pass; under this thread's own claim, a share of
/// that claim instead.
pub struct SessionLease {
    _lock: Option<StoreLock>,
    _claim: Option<Arc<ClaimInner>>,
}

/// While held, this thread's session passes never wait: they yield to a waiting launch or
/// take the lease at once. A caller holding a lock a launch needs never waits under it.
#[must_use]
pub struct UnqueuedReads(PhantomData<*const ()>);

impl UnqueuedReads {
    pub fn begin() -> UnqueuedReads {
        UNQUEUED.with(|depth| depth.set(depth.get().saturating_add(1)));
        UnqueuedReads(PhantomData)
    }
}

impl Drop for UnqueuedReads {
    fn drop(&mut self) {
        UNQUEUED.with(|depth| depth.set(depth.get().saturating_sub(1)));
    }
}

/// A launch's waiting file, exclusively locked until it is removed. A file that can be locked
/// shared is the leftover of a crashed launch.
struct Turn {
    path: PathBuf,
    _lock: StoreLock,
}

impl Drop for Turn {
    fn drop(&mut self) {
        // Removed while still locked: a reader that already opened it sees a live turn.
        if let Err(error) = std::fs::remove_file(&self.path)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            crate::messages::report(
                None,
                "store lock",
                format!("removing {}: {error}", self.path.display()),
            );
        }
    }
}

/// Locks the file under a hidden name, then renames it into place, so no reader ever finds a
/// launch's turn unlocked and mistakes it for a leftover.
fn register_turn(corral: &Path) -> Result<Turn> {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let dir = corral.join("lock").join(WAITING);
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let name = format!("{}-{seq}-{nanos}", std::process::id());
    let staged = dir.join(format!(".{name}"));
    let file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&staged)
        .with_context(|| format!("opening {}", staged.display()))?;
    file.lock()
        .with_context(|| format!("locking {}", staged.display()))?;
    let mut turn = Turn {
        path: staged,
        _lock: StoreLock::new(file),
    };
    let path = dir.join(name);
    std::fs::rename(&turn.path, &path)
        .with_context(|| format!("registering {}", path.display()))?;
    turn.path = path;
    Ok(turn)
}

/// Whether some launch's turn is live. Leftovers of crashed launches are removed.
fn launch_waiting(corral: &Path) -> Result<bool> {
    let dir = corral.join("lock").join(WAITING);
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error).with_context(|| format!("reading {}", dir.display())),
    };
    for entry in entries {
        let entry = entry.with_context(|| format!("reading {}", dir.display()))?;
        if entry.file_name().as_encoded_bytes().starts_with(b".") {
            continue;
        }
        let path = entry.path();
        let file = match std::fs::File::open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(error).with_context(|| format!("opening {}", path.display()));
            }
        };
        match file.try_lock_shared() {
            Err(std::fs::TryLockError::WouldBlock) => return Ok(true),
            Err(std::fs::TryLockError::Error(error)) => {
                return Err(error).with_context(|| format!("locking {}", path.display()));
            }
            Ok(()) => {
                // Names are never reused, so no launch can lock this file again.
                drop(file);
                if let Err(error) = std::fs::remove_file(&path)
                    && error.kind() != std::io::ErrorKind::NotFound
                {
                    crate::messages::report(
                        None,
                        "store lock",
                        format!("removing {}: {error}", path.display()),
                    );
                }
            }
        }
    }
    Ok(false)
}

/// None when the bound is beyond `Instant`'s range: wait without bound.
fn deadline(secs: u64) -> Option<Instant> {
    Instant::now().checked_add(Duration::from_secs(secs))
}

/// Sleeps one poll step, or false once the deadline has passed.
fn pause(deadline: Option<Instant>) -> bool {
    let step = deadline.map_or(POLL, |deadline| {
        deadline.saturating_duration_since(Instant::now()).min(POLL)
    });
    if step.is_zero() {
        return false;
    }
    std::thread::sleep(step);
    true
}

/// Lets a test wait, without timing, until a reader of its store has queued behind a launch.
#[cfg(test)]
pub(crate) mod queue_probe {
    use std::path::{Path, PathBuf};
    use std::sync::{Condvar, Mutex, PoisonError};
    use std::time::{Duration, Instant};

    static QUEUED: (Mutex<Vec<PathBuf>>, Condvar) = (Mutex::new(Vec::new()), Condvar::new());

    pub(crate) fn note(corral: &Path) {
        let mut queued = QUEUED.0.lock().unwrap_or_else(PoisonError::into_inner);
        if !queued.iter().any(|dir| dir == corral) {
            queued.push(corral.to_path_buf());
            QUEUED.1.notify_all();
        }
    }

    /// True once a reader of `corral` has queued, false if none has within `bound`.
    pub(crate) fn wait(corral: &Path, bound: Duration) -> bool {
        let deadline = Instant::now() + bound;
        let mut queued = QUEUED.0.lock().unwrap_or_else(PoisonError::into_inner);
        while !queued.iter().any(|dir| dir == corral) {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            queued = QUEUED
                .1
                .wait_timeout(queued, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        true
    }
}

fn check_socket(saved: Option<&Path>, socket: &Path) -> Result<()> {
    if let Some(saved) = saved
        && saved != socket
    {
        bail!(
            "saved pane belongs to {}, not {}",
            saved.display(),
            socket.display()
        );
    }
    Ok(())
}

fn record_on<T: serde::de::DeserializeOwned>(
    view: &dyn ReadView,
    area: &str,
    key: &str,
) -> Result<Option<T>> {
    let query = RecordQuery {
        area: Some(area.into()),
        key: Some(key.into()),
    };
    view.records(&query, PageReq::first(1))?
        .items
        .into_iter()
        .next()
        .map(|record| {
            crate::work::decoded(
                crate::work::Counter::RecordDecodes,
                u64::try_from(record.doc.to_string().len())
                    .context("session record byte count exceeds u64")?,
                crate::work::Subject::Plain,
            );
            serde_json::from_value(record.doc)
                .with_context(|| format!("parsing record {area}/{key}"))
        })
        .transpose()
}

fn binding_on(view: &dyn ReadView) -> Result<Option<HerdrSessionBinding>> {
    #[derive(Deserialize)]
    struct Bound {
        binding: HerdrSessionBinding,
    }
    Ok(record_on::<Bound>(view, SESSION_AREA, SESSION_KEY)?.map(|bound| bound.binding))
}

fn journal_on(view: &dyn ReadView) -> Result<Option<Option<PathBuf>>> {
    Ok(record_on::<JournalOwnership>(view, "review-launch", "current")?.map(|owner| owner.socket))
}

#[derive(Deserialize)]
struct JournalOwnership {
    #[serde(default)]
    socket: Option<PathBuf>,
}
