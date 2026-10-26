//! Explicit, durable ownership migration for references predating saved sockets.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};

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
    /// None while a bind holds the lease.
    pub fn try_begin_session_pass(&self) -> Result<Option<StoreLock>> {
        let Some(lease) = self.try_lock_shared("session-ownership")? else {
            return Ok(None);
        };
        *guard(&self.session_pass.0) = Some(self.read_session_view()?);
        Ok(Some(lease))
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
            .try_lock("session-ownership")?
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
            bound_at: Timestamp::now(),
        };
        self.bind_session_record(&binding)
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
