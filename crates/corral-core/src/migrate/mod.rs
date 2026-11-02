//! `corral migrate` (plan T-253 sections 12 and 14): the crash-safe move of a legacy file
//! store into the database, its durable rollback and its finalize step.
//!
//! The journal `C/migration.json` records every state; each step is written tmp -> fsync ->
//! rename -> fsync before the step's effect, so a rerun after a kill at any point resumes from
//! what the journal and the directory state say (the recovery tables of section 12). The
//! fence `C/state/schema-version` is the only legacy path written before finalize. The
//! database is built in `C/db.migrating/` and published with one directory rename to `C/db/`;
//! a published rollback archives `C/db/` with one rename, and a pre-publish rollback moves its
//! candidate to `C/db.discard/` with one rename and deletes it before the fence is restored.

mod apply;
mod blockers;
mod classify;
mod durable;
mod finalize;
mod import;
mod journal;
mod locks;
mod rollback;
mod scan;
mod status;
mod workspace;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;

pub use blockers::Blocker;
pub use journal::{Candidate, Journal, JournalState, RollbackRecord, RollbackStep};
pub use scan::{Area, Classified, Manifest, ManifestEntry};
pub use status::{DirState, Status, directory_problems};

/// The database directory of a published store.
pub const DB: &str = "db";
/// The directory an import builds the candidate database in.
pub const MIGRATING: &str = "db.migrating";
/// The quarantine name only a pre-publish rollback creates.
pub const DISCARD: &str = "db.discard";
/// The journal, under the `.corral` directory.
pub const JOURNAL: &str = crate::store::db::JOURNAL;
/// The M0 manifest of the legacy tree, named by the journal's `m0_digest`.
pub const MANIFEST: &str = "migration.m0.json";
/// The fence, relative to the `.corral` directory.
pub const FENCE_PATH: &str = "state/schema-version";
/// Where published rollbacks keep the whole v2 database.
pub const ROLLBACK_DIR: &str = "rollback";
/// How long maintenance waits for the exclusive locks by default.
pub const DEFAULT_WAIT: Duration = Duration::from_secs(10);

/// A named point inside a maintenance step where a test stops the process (plan section 12,
/// barrier matrix). An error from the barrier ends the command there, as a kill would: nothing
/// after the point runs and no cleanup happens.
pub type Barrier = Arc<dyn Fn(&str) -> Result<()> + Send + Sync>;

/// Read-only proof that every pane and associated process of a task is absent. Unknown or
/// unavailable observations must return false (or an error), never proof of absence.
pub type RuntimeProbe = Arc<dyn Fn(&crate::Task) -> Result<bool> + Send + Sync>;

/// One maintenance session on the store under `<root>/.corral`.
#[derive(Clone)]
pub struct Migrate {
    corral: PathBuf,
    wait: Duration,
    barrier: Option<Barrier>,
    runtime_probe: Option<RuntimeProbe>,
}

impl std::fmt::Debug for Migrate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Migrate")
            .field("corral", &self.corral)
            .field("wait", &self.wait)
            .finish_non_exhaustive()
    }
}

impl Migrate {
    /// Maintenance of the workspace at `root` (the directory holding `.corral`).
    pub fn new(root: &Path) -> Self {
        Self {
            corral: root.join(".corral"),
            wait: DEFAULT_WAIT,
            barrier: None,
            runtime_probe: None,
        }
    }

    /// How long `--apply`, `--rollback` and `--finalize` wait for exclusive locks.
    #[must_use]
    pub fn wait(mut self, wait: Duration) -> Self {
        self.wait = wait;
        self
    }

    /// Installs a test barrier (see [`Barrier`]).
    #[must_use]
    pub fn barrier(mut self, barrier: Barrier) -> Self {
        self.barrier = Some(barrier);
        self
    }

    /// Installs the host adapter's liveness check. Without it, open work remains blocking.
    #[must_use]
    pub fn runtime_probe(mut self, probe: RuntimeProbe) -> Self {
        self.runtime_probe = Some(probe);
        self
    }

    pub fn corral_dir(&self) -> &Path {
        &self.corral
    }

    /// The journal state, step, candidate location and next command; changes nothing.
    ///
    /// # Errors
    /// Returns read errors of the journal, fence or database metadata.
    pub fn status(&self) -> Result<Status> {
        status::status(self)
    }

    /// The drain check and the classification of every legacy file; creates nothing.
    ///
    /// # Errors
    /// Returns read or parse errors of the legacy tree.
    pub fn dry_run(&self) -> Result<scan::DryRun> {
        scan::dry_run(self)
    }

    /// Runs (or resumes) the forward path A1-A7 to `published`.
    ///
    /// # Errors
    /// Refuses blockers, unclassified files, inconsistent states and a rollback in progress;
    /// returns filesystem, import and database errors.
    pub fn apply(&self) -> Result<apply::Applied> {
        apply::apply(self)
    }

    /// Rolls back (or resumes rolling back) to the legacy store.
    ///
    /// # Errors
    /// Refuses a change count other than the named one, inconsistent states and finalize.
    pub fn rollback(&self, discard: Option<u64>) -> Result<rollback::RolledBack> {
        rollback::rollback(self, discard)
    }

    /// Deletes the legacy paths of a published store (`--finalize --yes`).
    ///
    /// # Errors
    /// Refuses every state other than `published`, `finalizing` and `finalized`.
    pub fn finalize(&self) -> Result<finalize::Finalized> {
        finalize::finalize(self)
    }

    /// Stops at barrier `id`: the installed barrier, and in debug builds the process-wide
    /// `CORRAL_MIGRATE_BARRIER` hook the kill tests use.
    pub(crate) fn hit(&self, id: &str) -> Result<()> {
        if let Some(barrier) = &self.barrier {
            barrier(id)?;
        }
        env_barrier(id);
        Ok(())
    }
}

/// In debug builds, a process started with `CORRAL_MIGRATE_BARRIER=<id>` announces the barrier
/// on stderr and stops there for good, so a test can `kill -9` it at that exact point.
#[cfg(debug_assertions)]
fn env_barrier(id: &str) {
    use std::io::Write;
    if std::env::var("CORRAL_MIGRATE_BARRIER").is_ok_and(|wanted| wanted == id) {
        let mut err = std::io::stderr();
        // The announcement is the test's signal; a failed write leaves the test waiting.
        if writeln!(err, "corral-migrate-barrier {id}")
            .and_then(|()| err.flush())
            .is_ok()
        {
            loop {
                std::thread::park();
            }
        }
    }
}

#[cfg(not(debug_assertions))]
fn env_barrier(_id: &str) {}

pub use apply::Applied;
pub use finalize::Finalized;
pub use rollback::RolledBack;
pub use scan::DryRun;
