//! Explicit, durable ownership migration for references predating saved sockets.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};

use anyhow::{Context, Result, bail};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::{Run, Store, StoreLock, Task, store::atomic_write};

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
#[derive(Debug)]
pub struct SessionView {
    pub binding: Option<HerdrSessionBinding>,
    /// [`Store::review_launch_socket`] as this pass read it.
    pub pending: Option<Option<PathBuf>>,
    /// Ownership verdicts by shared task-list snapshot and selected herdr session.
    verdicts: Mutex<Vec<(ListKey, Option<String>, bool)>>,
}

impl SessionView {
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

/// The view of the read-only snapshot being evaluated, keyed by its frozen task list and
/// shared by every clone of one opened store.
#[derive(Debug, Default)]
pub(crate) struct SnapshotView(Mutex<Option<(ListKey, Arc<SessionView>)>>);

fn guard<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Store {
    /// A fresh read of the binding and journal, as the launch boundary requires.
    pub fn read_session_view(&self) -> Result<Arc<SessionView>> {
        Ok(Arc::new(SessionView {
            binding: self.herdr_session_binding()?,
            pending: self.review_launch_socket()?,
            verdicts: Mutex::default(),
        }))
    }

    /// This pass's view: the pass begun on this handle, else this read-only snapshot's,
    /// else a fresh read. Outside a pass every call reads afresh.
    pub fn session_view(&self) -> Result<Arc<SessionView>> {
        if let Some(view) = guard(&self.session_pass.0).as_ref() {
            return Ok(Arc::clone(view));
        }
        if !self.is_snapshot() {
            return self.read_session_view();
        }
        // A snapshot's listing is its frozen task list: no filesystem access.
        let key = Arc::downgrade(&self.list_shared()?);
        let mut slot = guard(&self.snapshot_view.0);
        if let Some((list, view)) = slot.as_ref()
            && list.ptr_eq(&key)
        {
            return Ok(Arc::clone(view));
        }
        let view = self.read_session_view()?;
        *slot = Some((key, Arc::clone(&view)));
        Ok(view)
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
        #[derive(Deserialize)]
        struct Ownership {
            #[serde(default)]
            socket: Option<PathBuf>,
        }
        let path = self.state_dir().join("review-launch.json");
        match std::fs::read(&path) {
            Ok(bytes) => {
                let ownership: Ownership = serde_json::from_slice(&bytes)
                    .with_context(|| format!("parsing {}", path.display()))?;
                Ok(Some(ownership.socket))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error).with_context(|| format!("reading {}", path.display())),
        }
    }

    /// This is durable ownership, not rebuildable `state/` data.
    pub fn herdr_session_binding(&self) -> Result<Option<HerdrSessionBinding>> {
        let path = self.dir().join("herdr-session.json");
        crate::work::record(crate::work::Work::SessionBindingRead, 1);
        match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .with_context(|| format!("parsing {}", path.display()))
                .map(Some),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error).with_context(|| format!("reading {}", path.display())),
        }
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
        if let Some(binding) = self.herdr_session_binding()? {
            if binding.socket != socket {
                bail!("store is already bound to {}", binding.socket.display());
            }
            return Ok(());
        }
        if let Some(saved) = self.review_launch_socket()? {
            check_socket(saved.as_deref(), socket)?;
        }
        for task in self.list()? {
            if let Some(saved) = task
                .launch
                .as_ref()
                .and_then(|launch| launch.socket.as_ref())
                && saved != socket
            {
                bail!("{} has a launch owned by {}", task.id, saved.display());
            }
            for run in &task.runs {
                check_run(run, socket)?;
            }
            for reference in task
                .plans
                .iter()
                .flat_map(|plan| &plan.reviews)
                .filter_map(|review| review.herdr.as_ref())
                .chain(
                    task.aggregate_attempts
                        .iter()
                        .flat_map(|attempt| &attempt.reviews)
                        .filter_map(|review| review.herdr.as_ref()),
                )
            {
                check_socket(reference.socket.as_deref(), socket)?;
            }
        }
        let binding = HerdrSessionBinding {
            socket: socket.to_path_buf(),
            launch_evidence: launch_evidence.into(),
            bound_at: Timestamp::now(),
        };
        atomic_write(
            &self.dir().join("herdr-session.json"),
            &serde_json::to_string_pretty(&binding).context("serializing herdr session binding")?,
        )
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

fn check_run(run: &Run, socket: &Path) -> Result<()> {
    if let Some(reference) = &run.herdr {
        check_socket(reference.socket.as_deref(), socket)?;
    }
    if let Some(gate) = &run.gate {
        check_socket(gate.socket.as_deref(), socket)?;
    }
    for child in &run.review_runs {
        check_run(child, socket)?;
    }
    Ok(())
}
