use std::path::{Path, PathBuf};

use std::fs;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result, bail};

use crate::backend::{
    BlobQuery, BlobRow, LeaseRow, LifecycleOutcome, LifecycleRow, LifecycleWrite, Operation,
    PageReq, ScratchRow, ScratchState, StorageQuery, StorageRow,
};
use crate::config::Config;
use crate::model::{Event, Status, Task};
use crate::runtime::Executable;

mod assemble;
mod batch;
mod cleanup_panes;
pub mod codec;
mod conditions;
pub mod db;
pub(crate) mod events;
mod fixture;
mod gate_rows;
mod head;
mod head_agent;
mod head_api;
mod head_artifact;
pub(crate) use head_artifact::digest_token as artifact_digest_token;
mod head_checkpoints;
mod head_child_edit;
mod head_children;
mod head_edit;
mod head_findings;
mod head_finish;
mod head_keyed;
mod head_mut;
mod head_native;
mod head_native_inputs;
mod head_native_wakes;
mod head_obligations;
mod head_overlay;
mod head_reviews;
mod head_run;
mod head_wait;
mod heads;
mod member_gate;
pub(crate) mod migrate_rows;
mod native_counts;
mod native_work_index;
mod native_worker_index;
mod normalize;
mod plan_member;
mod plan_scope;
mod question_events;
mod read;
mod readers;
mod snapshot;
mod submission;
mod submit;
mod task_mut_task;
mod task_ref_head;
pub(crate) mod task_ref_task;
mod work_index;
mod write;
pub use cleanup_panes::CleanupPane;
pub use codec::{spec_revision, task_panes};
pub use conditions::{SLIM_TEXT_BYTES, is_condition_kind};
pub use db::{store_holders, try_exclusive_store_gate};
pub use gate_rows::{GateOutcome, ScratchReclaim};
pub use head::{CheckpointMetas, TaskHead};
pub use head_api::Board;
pub use head_mut::TaskHeadMut;
pub use heads::*;
pub use member_gate::{MemberGateAmended, MemberGateAmendment};
use plan_scope::ScopeWrite;
pub use task_ref_head::shell_of;
pub(crate) use write::apply_error;
#[cfg(test)]
#[path = "store_lock_tests.rs"]
mod lock_tests;
#[cfg(test)]
#[path = "store_version_tests.rs"]
mod version_tests;

/// The `.corral/` directory of a workspace.
///
/// Layout:
/// ```text
/// <root>/.corral/
///   corral.toml          config
///   db/corral.db         the task database (with its WAL sidecars)
///   pins.toml            ordered pinned tasks/artifacts (`[[pin]]` ref, label, follow)
///   artifacts/<task>/<run>/<sha8>-<file name>   artifact snapshots
///   prompts/<task>-<run>.md                     rendered dispatch prompts
///   state/               UI state and scratch; `state/schema-version` is the store fence (4)
///   lock/                leases (`<task>.lock`), the store gate and its holder registry
///   .gitignore           contains "artifacts/\nprompts/\nstate/\nlock/\ndb/\n"
/// ```
#[derive(Debug, Clone)]
pub struct Store {
    root: PathBuf,
    dir: PathBuf,
    pub config: Config,
    /// Full content identity, including rejected revisions, last observed by this process.
    pub(crate) config_hash: String,
    executable: Arc<Executable>,
    skew_reported: Arc<AtomicBool>,
    db: Arc<db::Db>,
    /// Set only on a `read_only_snapshot`: every read comes from one view of the store.
    frozen: Option<Arc<snapshot::Frozen>>,
    submission_barrier: Option<submission::SubmissionBarrier>,
    pub(crate) attribution: Arc<crate::attribution::AttributionContext>,
    /// The session binding read by this handle's pass; see `Store::session_view`.
    pub(crate) session_pass: crate::session_binding::HandlePass,
    pub(crate) snapshot_view: Arc<crate::session_binding::SnapshotView>,
}

/// Release the advisory lock at the operation boundary, even if a concurrent
/// fork still holds a duplicate of the open-file description.
pub struct StoreLock(fs::File);

impl StoreLock {
    pub(crate) fn new(file: fs::File) -> StoreLock {
        StoreLock(file)
    }
}

impl Drop for StoreLock {
    fn drop(&mut self) {
        if let Err(error) = self.0.unlock() {
            crate::messages::report(None, "store lock", format!("releasing store lock: {error}"));
        }
    }
}

impl Store {
    /// The time stamped on what this store writes: the wall clock, or a fixture's pinned instant.
    #[must_use]
    pub fn now(&self) -> jiff::Timestamp {
        self.attribution
            .pinned_now
            .get()
            .copied()
            .or_else(crate::clock::pinned)
            .unwrap_or_else(jiff::Timestamp::now)
    }

    /// Exclusive lock on `lock/<name>.lock`, held until the returned guard is dropped.
    pub(crate) fn lock(&self, name: &str) -> Result<StoreLock> {
        let dir = self.dir.join("lock");
        fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        let path = dir.join(format!("{name}.lock"));
        let f = fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&path)
            .with_context(|| format!("opening {}", path.display()))?;
        f.lock()
            .with_context(|| format!("locking {}", path.display()))?;
        let guard = StoreLock(f);
        if self.submission_barrier.is_some() {
            self.submission_barrier(&format!("lock:{name}"));
        }
        Ok(guard)
    }

    /// Like the per-task locks, but non-blocking: None while another process holds `lock/<name>.lock`.
    pub fn try_lock(&self, name: &str) -> Result<Option<StoreLock>> {
        self.try_lock_mode(name, false)
    }

    /// A non-blocking shared lease, excluding exclusive claims on the same name.
    pub fn try_lock_shared(&self, name: &str) -> Result<Option<StoreLock>> {
        self.try_lock_mode(name, true)
    }

    fn try_lock_mode(&self, name: &str, shared: bool) -> Result<Option<StoreLock>> {
        let dir = self.dir.join("lock");
        fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        let path = dir.join(format!("{name}.lock"));
        let f = fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&path)
            .with_context(|| format!("opening {}", path.display()))?;
        match if shared {
            f.try_lock_shared()
        } else {
            f.try_lock()
        } {
            Ok(()) => Ok(Some(StoreLock(f))),
            Err(fs::TryLockError::WouldBlock) => Ok(None),
            Err(fs::TryLockError::Error(e)) => {
                Err(e).with_context(|| format!("locking {}", path.display()))
            }
        }
    }

    /// Finds the workspace: $CORRAL_ROOT if set, else walks up from `start` to the first dir
    /// containing `.corral/corral.toml`. Error message on failure: "no .corral workspace found (run `corral init`)".
    pub fn discover(start: &Path) -> Result<Store> {
        Store::open(&Store::find_root(start)?)
    }

    /// The workspace root `discover` would open, without loading its config.
    pub fn find_root(start: &Path) -> Result<PathBuf> {
        if let Some(r) = std::env::var_os("CORRAL_ROOT").filter(|v| !v.is_empty()) {
            return Ok(PathBuf::from(r));
        }
        let restricted = [
            "CORRAL_GATE_ATTEMPT",
            "CORRAL_TEST",
            "CI",
            "RUST_TEST_THREADS",
        ]
        .iter()
        .any(|key| std::env::var_os(key).is_some());
        Self::find_ancestor(start, restricted)
    }

    fn find_ancestor(start: &Path, restricted: bool) -> Result<PathBuf> {
        let start =
            std::path::absolute(start).with_context(|| format!("resolving {}", start.display()))?;
        for dir in start.ancestors() {
            if dir.join(".corral").join("corral.toml").is_file() {
                // Use the same identity as Store::open, including Windows verbatim
                // prefixes and macOS /var aliases, before handing paths to callers.
                return dir
                    .canonicalize()
                    .with_context(|| format!("resolving workspace {}", dir.display()));
            }
            if restricted
                && dir
                    .parent()
                    .and_then(Path::file_name)
                    .is_some_and(|n| n == ".worktrees")
            {
                bail!(
                    "refusing workspace discovery outside test/gate worktree {}; pass an explicit --root",
                    dir.display()
                );
            }
        }
        bail!("no .corral workspace found (run `corral init`)")
    }

    /// Creates `<root>/.corral/` with all subdirs, the .gitignore above and `Config::default_for(root)`.
    /// Idempotent: an existing corral.toml is kept as-is (loaded, not overwritten).
    pub fn init(root: &Path) -> Result<Store> {
        Self::init_with(root, |_| Ok(()))
    }

    /// Initializes a store, configuring attribution before observing its configuration.
    /// `prepare` runs before configuration is loaded, including on idempotent init.
    pub fn init_with(root: &Path, prepare: impl FnOnce(&Store) -> Result<()>) -> Result<Store> {
        let dir = root.join(".corral");
        if root.join(".git").exists() {
            let ignore = root.join(".gitignore");
            let mut contents = if ignore.exists() {
                fs::read_to_string(&ignore)
                    .with_context(|| format!("reading {}", ignore.display()))?
            } else {
                String::new()
            };
            if !contents
                .lines()
                .any(|line| matches!(line.trim(), ".evidence/" | "/.evidence/"))
            {
                if !contents.is_empty() && !contents.ends_with('\n') {
                    contents.push('\n');
                }
                contents.push_str("/.evidence/\n");
                fs::write(&ignore, contents)
                    .with_context(|| format!("writing {}", ignore.display()))?;
            }
        }
        for sub in ["artifacts", "prompts", "state", "lock"] {
            let p = dir.join(sub);
            fs::create_dir_all(&p).with_context(|| format!("creating {}", p.display()))?;
        }
        let gi = dir.join(".gitignore");
        if !gi.exists() {
            fs::write(&gi, "artifacts/\nprompts/\nstate/\nlock/\ndb/\n")
                .with_context(|| format!("writing {}", gi.display()))?;
        }
        let toml = dir.join("corral.toml");
        if !toml.exists() {
            let mut config = Config::default_for(root);
            config.pricing = crate::pricing::starter_prices();
            atomic_write(
                &toml,
                &format!(
                    "# prices as of {}, verify\n{}{}",
                    crate::pricing::CATALOG_DATE,
                    config.to_toml(),
                    include_str!("routing-starter.toml")
                ),
            )?;
        }
        db::init(&dir)?;
        Self::open_with(root, prepare)
    }

    /// Opens an existing store at `root` (no walking).
    pub fn open(root: &Path) -> Result<Store> {
        Self::open_with(root, |_| Ok(()))
    }

    /// Configures attribution before initial configuration observation and validation.
    /// `prepare` must not rely on `store.config`, which is not loaded yet.
    pub fn open_with(root: &Path, prepare: impl FnOnce(&Store) -> Result<()>) -> Result<Store> {
        Self::open_inner(root, true, Arc::default(), prepare)
    }

    /// Reopens this store with fresh configuration while keeping its attribution
    /// default and scopes, so events the reopened store logs keep their author.
    pub fn reopen(&self) -> Result<Store> {
        Self::open_inner(&self.root, true, Arc::clone(&self.attribution), |_| Ok(()))
    }

    /// Like [`Store::reopen`], without observing configuration edits.
    pub fn reopen_unobserved(&self) -> Result<Store> {
        Self::open_inner(&self.root, false, Arc::clone(&self.attribution), |_| Ok(()))
    }

    /// Reads a store for reconciliation preflight without observing config edits.
    /// The owning sync will observe those edits only after verifying its session.
    pub fn open_unobserved(root: &Path) -> Result<Store> {
        Self::open_inner(root, false, Arc::default(), |_| Ok(()))
    }

    fn open_inner(
        root: &Path,
        observe: bool,
        attribution: Arc<crate::attribution::AttributionContext>,
        prepare: impl FnOnce(&Store) -> Result<()>,
    ) -> Result<Store> {
        // Absolute root: prompt files and worktrees are handed to shells running in other directories.
        let root = &root
            .canonicalize()
            .with_context(|| format!("resolving {}", root.display()))?;
        let dir = root.join(".corral");
        let db = db::open(&dir)?;
        let mut store = Store {
            root: root.to_path_buf(),
            dir,
            config: Config::default_for(root),
            config_hash: String::new(),
            executable: Executable::current()?,
            skew_reported: Arc::new(AtomicBool::new(false)),
            db,
            frozen: None,
            submission_barrier: None,
            attribution,
            session_pass: Default::default(),
            snapshot_view: Arc::default(),
        };
        store.check_schema()?;
        prepare(&store)?;
        if observe {
            store.load_initial_config()?;
        } else {
            store.config = store.config_layers()?.config()?;
        }
        crate::agent::pm_state::migrate(&store)?;
        Ok(store)
    }

    /// Called before every sync tick and before launching or recording new work. With open, the
    /// only schema check: `get` and `list` trust it rather than re-reading the marker. A
    /// read-only snapshot, which refuses every write, trusts the check made when it was taken.
    /// A replacement latches this process off even if the executable is restored.
    pub fn check_runtime(&self) -> Result<()> {
        // Test-only rendezvous on the real watch path after its fake herdr signals readiness.
        // Omitted from default and release builds.
        #[cfg(all(feature = "test-skew-barrier", debug_assertions, unix))]
        if self.is_snapshot()
            && let Some(dir) = std::env::var_os("CORRAL_TEST_SKEW_BARRIER")
        {
            use std::io::{Read, Write};
            let dir = Path::new(&dir);
            if dir.join("ready.sent").exists() {
                std::fs::OpenOptions::new()
                    .write(true)
                    .open(dir.join("entered"))?
                    .write_all(b"x")?;
                std::fs::File::open(dir.join("resume"))?.read_exact(&mut [0])?;
            }
        }
        let result = self.executable.check().and_then(|()| match self.frozen {
            Some(_) => Ok(()),
            None => self.check_schema(),
        });
        if result.is_err() || self.skew_reported.load(Ordering::Relaxed) {
            if !self.skew_reported.swap(true, Ordering::Relaxed) {
                let event = self.attributed_event(
                    None,
                    "version_skew",
                    serde_json::json!({
                        "pid": std::process::id(),
                        "reason": result.err().map(|e| format!("{e:#}")),
                        "message": crate::UPDATE_NOTICE,
                    }),
                )?;
                self.append_event_immediate(events::new_event(event)?)?;
            }
            bail!("{}", crate::UPDATE_NOTICE);
        }
        Ok(())
    }

    /// Whether executable/schema skew has permanently fenced this process.
    pub fn runtime_latched(&self) -> bool {
        self.skew_reported.load(Ordering::Relaxed)
    }

    /// The installed executable watched by the long-running board process.
    pub fn installed_executable(&self) -> &Path {
        self.executable.path()
    }

    fn check_schema(&self) -> Result<()> {
        crate::schema::store_version(self).map(|_| ())
    }

    /// This store with its tasks kept in `backend` instead of its database (test fixtures that
    /// run the facade over the in-memory reference backend).
    #[doc(hidden)]
    pub fn with_backend(&self, backend: Box<dyn crate::backend::StoreBackend>) -> Store {
        let mut store = self.clone();
        store.db = db::Db::unguarded(backend);
        store.frozen = None;
        store
    }

    /// Workspace root (the dir containing `.corral/`).
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The `.corral/` dir.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// `<dir>/corral.toml`.
    pub fn config_path(&self) -> PathBuf {
        self.dir.join("corral.toml")
    }

    pub fn config_layers(&self) -> Result<crate::ConfigLayers> {
        crate::ConfigLayers::load(&self.config_path())
    }

    /// Content hash last observed by config reload, including layer-only and rejected edits.
    /// Reading this identity never touches the filesystem.
    pub fn config_hash(&self) -> &str {
        &self.config_hash
    }

    /// Serializes user writes across workspaces as well as within this one.
    pub(crate) fn lock_user_config(&self) -> Result<StoreLock> {
        let path = self.config_layers()?.path(crate::ConfigLayer::User)?;
        let parent = path
            .parent()
            .context("user config has no parent directory")?;
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        let lock = path.with_extension("lock");
        let file = fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&lock)
            .with_context(|| format!("opening {}", lock.display()))?;
        file.lock()
            .with_context(|| format!("locking {}", lock.display()))?;
        Ok(StoreLock(file))
    }

    /// All tasks (including done/dropped), sorted by numeric id.
    #[track_caller]
    pub fn list(&self) -> Result<Vec<Task>> {
        Ok(self.list_shared()?.as_ref().clone())
    }

    /// [`Store::list`] without copying: an unchanged store shares one listing across calls and
    /// store clones. Prefer it where tasks are only read.
    #[track_caller]
    pub fn list_shared(&self) -> Result<Arc<Vec<Task>>> {
        crate::work::record(crate::work::Work::TaskList, 1);
        self.read_listing(std::panic::Location::caller())
    }

    /// Accepts "T-042", "t-42", "42". Error "task T-042 not found" when missing.
    #[track_caller]
    pub fn get(&self, id: &str) -> Result<Task> {
        crate::work::record(crate::work::Work::TaskGet, 1);
        let id = Self::normalize_id(id)?;
        let location = std::panic::Location::caller();
        match self.read_task(&id, location)? {
            Some(record) => Ok(record.task),
            None => bail!("task {id} not found"),
        }
    }

    /// Normalizes an id: "42" / "t-42" / "T-042" -> "T-042".
    pub fn normalize_id(id: &str) -> Result<String> {
        let t = id.trim();
        let digits = t
            .strip_prefix("T-")
            .or_else(|| t.strip_prefix("t-"))
            .unwrap_or(t);
        match digits.parse::<u64>() {
            Ok(n) if !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) => {
                Ok(format!("T-{n:03}"))
            }
            _ => bail!("invalid task id {id:?}"),
        }
    }

    /// Allocates the next id inside one `CreateTask` operation (max existing id + 1), stores
    /// the task with the given body, logs a "created" event, and returns the task.
    pub fn create(
        &self,
        title: &str,
        project: Option<&str>,
        status: Status,
        body: &str,
    ) -> Result<Task> {
        self.create_initialized(title, project, status, body, |_| Ok(()))
    }

    pub fn create_with_origin(
        &self,
        title: &str,
        project: Option<&str>,
        status: Status,
        body: &str,
        origin: crate::Origin,
    ) -> Result<Task> {
        self.create_initialized(title, project, status, body, |task| {
            task.origin = Some(origin);
            Ok(())
        })
    }

    /// Initializes all filing fields under the admission lock before publishing once.
    ///
    /// # Errors
    /// Returns initialization, locking, serialization, or persistence errors.
    pub fn create_initialized(
        &self,
        title: &str,
        project: Option<&str>,
        status: Status,
        body: &str,
        initialize: impl FnOnce(&mut Task) -> Result<()>,
    ) -> Result<Task> {
        let _board = self.lock("dispatch")?;
        let next = self.next_task_number()?;
        let mut task = batch::draft_task(next, title, project, status, body, self.now());
        let _unit = self.event_unit();
        initialize(&mut task)?;
        self.log_event(
            Some(&task.id),
            "created",
            serde_json::json!({
                "title": task.title, "status": task.status.as_str(), "origin": task.origin
            }),
        )?;
        self.insert_new(&task)
    }

    /// Stores the task as one `UpdateTask` writing only the rows whose documents changed,
    /// setting `updated` to now; a task the store lacks is stored under its own id.
    pub fn save(&self, task: &Task) -> Result<()> {
        let _board = self.lock("dispatch")?;
        self.save_inner(task).map(|_| ())
    }

    /// Reads one coherent task under its lock without rewriting its timestamp.
    pub fn read_locked<T>(&self, id: &str, f: impl FnOnce(&Task) -> Result<T>) -> Result<T> {
        let id = Self::normalize_id(id)?;
        let _lock = self.lock(&id)?;
        f(&self.get(&id)?)
    }

    /// Takes the per-task lock, re-reads the task from a fresh view, applies `f`, saves, releases the lock,
    /// and returns the saved task. Use this for every read-modify-write.
    #[track_caller]
    pub fn update<F>(&self, id: &str, f: F) -> Result<Task>
    where
        F: FnOnce(&mut Task) -> Result<()>,
    {
        let _board = self.lock("dispatch")?;
        let id = Self::normalize_id(id)?;
        let _lock = self.lock(&id)?;
        let held = self.hold(&id)?;
        let mut task = held.task();
        let _unit = self.event_unit();
        f(&mut task)?;
        self.save_held(&held, &task)
    }

    /// Like `update`, but `f` decides on the fresh copy whether to write: returning `false` leaves the
    /// task untouched and yields `None`. Use it when the change was computed from an earlier read and
    /// must be skipped if the task has moved on since (compare-and-swap under the per-task lock).
    #[track_caller]
    pub fn try_update<F>(&self, id: &str, f: F) -> Result<Option<Task>>
    where
        F: FnOnce(&mut Task) -> Result<bool>,
    {
        let _board = self.lock("dispatch")?;
        let id = Self::normalize_id(id)?;
        let _lock = self.lock(&id)?;
        let held = self.hold(&id)?;
        let mut task = held.task();
        let _unit = self.event_unit();
        if !f(&mut task)? {
            return Ok(None);
        }
        self.save_held(&held, &task).map(Some)
    }

    /// Atomically compare the board/config and mutate one task. All task and config
    /// writers share this lock; the callback must not call another write operation.
    #[track_caller]
    pub fn dispatch_update<R>(
        &self,
        id: &str,
        f: impl FnOnce(&Store, &mut Task) -> Result<R>,
    ) -> Result<(Task, R)> {
        let _board = self.lock("dispatch")?;
        let id = Self::normalize_id(id)?;
        let _task = self.lock(&id)?;
        let mut fresh = self.clone();
        fresh.reload_config_if_changed()?;
        let held = fresh.hold(&id)?;
        let mut task = held.task();
        let _unit = fresh.event_unit();
        let result = f(&fresh, &mut task)?;
        Ok((fresh.save_held(&held, &task)?, result))
    }

    /// Final short submission boundary, inside the executor's existing launch/service lock.
    ///
    /// `prepare` validates the fresh executor journal's run/attempt/candidate and all existing
    /// safety checks, then changes its task submission marker. Neither callback may acquire
    /// a board-writing lock. No worktree/pane creation or outcome wait belongs here.
    /// The task marker and durable intent are saved BEFORE the single IPC call. Policy
    /// writers stay excluded until that call returns. Malformed fresh config refuses
    /// without consulting cached authority or changing prepared task state.
    pub fn submit_operating_action<R>(
        &self,
        authorization: &crate::ActionAuthorization,
        expected: &crate::OperatingActionIdentity,
        prepare: impl FnOnce(&Store, &mut Task) -> Result<()>,
        submit: impl FnOnce() -> Result<R>,
    ) -> Result<crate::SubmissionOutcome<R>> {
        self.submit_in(authorization, expected, prepare, submit)
    }

    /// Appends one database event, or joins the active update's atomic write. A condition
    /// kind is observed as a condition run; inside an update it is applied once that update
    /// commits and discarded if it refuses.
    pub fn log_event(
        &self,
        task: Option<&str>,
        kind: &str,
        detail: serde_json::Value,
    ) -> Result<()> {
        if conditions::is_condition_kind(kind) {
            return self.observe_condition(task, kind, detail).map(drop);
        }
        let ev = self.attributed_event(task, kind, detail)?;
        self.append_database_event(ev)
    }

    /// Reads stored events, newest last; malformed legacy rows are skipped.
    pub fn events(&self) -> Result<Vec<Event>> {
        Ok(self.events_shared()?.as_ref().clone())
    }

    /// All events on one committed view, newest last. Prefer indexed pages on hot paths.
    ///
    /// # Errors
    /// Returns database view or document decoding errors.
    pub fn events_shared(&self) -> Result<Arc<Vec<Event>>> {
        self.database_events(None, &[]).map(Arc::new)
    }

    /// The owner's override `<dir>/prompts/<name>` of a built-in prompt template (`run.md`,
    /// `review.md`): configuration the owner edits, read but never written by corral.
    pub fn template_override(&self, name: &str) -> PathBuf {
        self.dir.join("prompts").join(name) // path-guard: allow (owner template configuration)
    }

    /// Intent/effect journals must be durable before a callback performs the effect.
    /// Ordinary records use `save_record` and join the callback's unit of work.
    pub(crate) fn commit_record_intent(
        &self,
        area: &str,
        key: &str,
        value: &impl serde::Serialize,
    ) -> Result<()> {
        anyhow::ensure!(!self.is_snapshot(), "a snapshot cannot commit an intent");
        self.check_runtime()?;
        let before = self.record_row(area, key)?;
        let doc = serde_json::to_value(value).context("encoding record intent")?;
        if before.as_ref().is_some_and(|row| row.doc == doc) {
            return Ok(());
        }
        self.db
            .backend
            .apply(crate::backend::Operation::PutRecords(
                crate::backend::PutRecords {
                    writes: vec![crate::backend::RecordWrite {
                        area: area.to_owned(),
                        key: key.to_owned(),
                        expected: before.map(|row| row.version),
                        doc: Some(doc),
                        updated: crate::clock::now(),
                    }],
                    events: Vec::new(),
                    reservations: Vec::new(),
                },
            ))
            .map_err(write::apply_error)
            .context("committing record intent")?;
        Ok(())
    }

    /// Validated root for rebuildable application caches (not created).
    pub fn cache_root(&self) -> Result<PathBuf> {
        Ok(self.storage_layout()?.cache_root().to_path_buf())
    }

    /// The validated storage layout (fixed class roots plus the configured cache and scratch
    /// roots, plan section 8).
    pub fn storage_layout(&self) -> Result<crate::StorageLayout> {
        crate::StorageLayout::resolve(
            &self.root,
            &self.config.storage,
            &self.config.projects,
            &crate::StorageEnv::from_process(),
        )
        .map_err(|errors| {
            anyhow::anyhow!(
                "{}",
                errors
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("; ")
            )
        })
    }

    /// The backend behind this store, for the evidence facade's own operations.
    pub(crate) fn backend(&self) -> &dyn crate::backend::StoreBackend {
        self.db.backend.as_ref()
    }

    /// Up to `limit` storage rows matching `query`, every page read on one view.
    fn storage_rows_up_to(&self, query: &StorageQuery, limit: u32) -> Result<Vec<StorageRow>> {
        let view = self.view()?;
        let mut rows = Vec::new();
        let mut req = PageReq::first(limit.min(256));
        while req.limit > 0 {
            let page = view.storage_rows(query, req)?;
            rows.extend(page.items);
            let left = limit.saturating_sub(u32::try_from(rows.len()).unwrap_or(u32::MAX));
            match page.next {
                Some(after) => {
                    req = PageReq {
                        after: Some(after),
                        limit: left.min(256),
                    }
                }
                None => break,
            }
        }
        Ok(rows)
    }

    fn lifecycle_query(&self, query: &StorageQuery, limit: u32) -> Result<Vec<LifecycleRow>> {
        Ok(self
            .storage_rows_up_to(query, limit)?
            .into_iter()
            .filter_map(|row| match row {
                StorageRow::Lifecycle(row) => Some(row),
                _ => None,
            })
            .collect())
    }

    /// The lifecycle rows of `task` (every generation and target), by key.
    pub fn lifecycle_rows(&self, task: &str) -> Result<Vec<LifecycleRow>> {
        let query = StorageQuery::Lifecycle {
            task: Some(task.to_owned()),
        };
        self.lifecycle_query(&query, u32::MAX)
    }

    /// At most `limit` deferred lifecycle rows due by `now`, earliest check first, read through
    /// the `lifecycle_due` index: a bounded batch whatever the history holds.
    pub fn lifecycle_due(&self, now: jiff::Timestamp, limit: u32) -> Result<Vec<LifecycleRow>> {
        let query = StorageQuery::LifecycleDue {
            outcome: LifecycleOutcome::Deferred,
            now,
        };
        self.lifecycle_query(&query, limit)
    }

    /// At most `limit` lifecycle rows recorded under a policy digest other than `digest`: the
    /// `lifecycle_policy` index range below it, then the range above it, each read in index
    /// order so the work is bounded by `limit`, not by the rows recorded.
    pub fn lifecycle_policy_other_than(
        &self,
        digest: &str,
        limit: u32,
    ) -> Result<Vec<LifecycleRow>> {
        let digest = digest.to_owned();
        let below = StorageQuery::LifecyclePolicyBelow {
            digest: digest.clone(),
        };
        let mut rows = self.lifecycle_query(&below, limit)?;
        let left = limit.saturating_sub(u32::try_from(rows.len()).unwrap_or(u32::MAX));
        if left > 0 {
            let above = StorageQuery::LifecyclePolicyAbove { digest };
            rows.extend(self.lifecycle_query(&above, left)?);
        }
        Ok(rows)
    }

    /// The lifecycle row `(task, generation, target)`, by key.
    pub fn lifecycle_row(
        &self,
        task: &str,
        generation: &str,
        target: &str,
    ) -> Result<Option<LifecycleRow>> {
        let query = StorageQuery::LifecycleKey {
            task: task.to_owned(),
            generation: generation.to_owned(),
            target: target.to_owned(),
        };
        Ok(self.lifecycle_query(&query, 1)?.into_iter().next())
    }

    /// Writes one lifecycle row; a stale `expected` outcome fails with a conflict.
    pub fn put_lifecycle(&self, write: LifecycleWrite) -> Result<()> {
        let key = format!("{}/{}/{}", write.task, write.generation, write.target);
        self.backend()
            .apply(Operation::Lifecycle(write))
            .map_err(|error| anyhow::anyhow!("writing lifecycle {key}: {error}"))?;
        Ok(())
    }

    /// Every blob row, in hash order, read on one view.
    pub fn blob_rows(&self) -> Result<Vec<BlobRow>> {
        all_blobs(self.view()?.as_ref(), &BlobQuery::default())
    }

    /// The scratch registrations in `states`, read from the view alone: unlike
    /// `scratch_rows_in`, nothing on disk is created or opened.
    pub fn scratch_rows_viewed(&self, states: &[ScratchState]) -> Result<Vec<ScratchRow>> {
        let query = StorageQuery::Scratch {
            states: states.to_vec(),
        };
        let rows = self.storage_rows_up_to(&query, u32::MAX)?.into_iter();
        Ok(rows
            .filter_map(|row| match row {
                StorageRow::Scratch(row) => Some(row),
                _ => None,
            })
            .collect())
    }

    /// Read one scratch registration by id without scanning historical registrations.
    pub fn scratch_row_viewed(&self, id: &str) -> Result<Option<ScratchRow>> {
        let query = StorageQuery::ScratchKey { id: id.to_owned() };
        Ok(self
            .storage_rows_up_to(&query, 1)?
            .into_iter()
            .find_map(|row| match row {
                StorageRow::Scratch(row) => Some(row),
                _ => None,
            }))
    }

    /// The storage leases (readers that hold blob GC's horizon back).
    pub fn storage_leases(&self) -> Result<Vec<LeaseRow>> {
        Ok(self
            .storage_rows_up_to(&StorageQuery::Leases, u32::MAX)?
            .into_iter()
            .filter_map(|row| match row {
                StorageRow::Lease(row) => Some(row),
                _ => None,
            })
            .collect())
    }

    /// Database integrity problems: the database half of `storage check`.
    pub fn check_database(&self) -> Result<Vec<String>> {
        Ok(self.backend().check()?.problems)
    }

    /// This workspace's `corral.toml` with every project path made absolute, for a copy of the
    /// store elsewhere that must validate as this one does.
    pub fn config_with_absolute_projects(&self) -> Result<String> {
        let text = fs::read_to_string(self.config_path()).unwrap_or_default();
        let mut config: toml_edit::DocumentMut = text.parse()?;
        let projects = config
            .get_mut("projects")
            .and_then(toml_edit::Item::as_table_like_mut);
        for (_, project) in projects.into_iter().flat_map(|p| p.iter_mut()) {
            let path = project.get("path").and_then(toml_edit::Item::as_str);
            let absolute = path.map(|p| self.root.join(p).to_string_lossy().into_owned());
            if let (Some(absolute), Some(table)) = (absolute, project.as_table_like_mut()) {
                table.insert("path", toml_edit::value(absolute));
            }
        }
        Ok(config.to_string())
    }

    /// One consistent copy of the database only, taken while writers continue, into `dest`,
    /// which must not exist; blobs are not included (plan section 7, backup).
    pub fn backup_database(&self, dest: &Path) -> Result<()> {
        self.backend().backup_database(dest)
    }

    /// Absolute path of a project's repo: config path joined to root (absolute paths kept).
    pub fn project_path(&self, project: &str) -> Result<PathBuf> {
        let p = self
            .config
            .projects
            .get(project)
            .with_context(|| format!("unknown project {project:?}"))?;
        let path = Path::new(&p.path);
        Ok(if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.root.join(path)
        })
    }
}

fn all_blobs(view: &dyn crate::backend::ReadView, query: &BlobQuery) -> Result<Vec<BlobRow>> {
    let mut rows = Vec::new();
    let mut req = PageReq::first(256);
    loop {
        let page = view.blobs(query, req)?;
        rows.extend(page.items);
        let Some(after) = page.next else {
            return Ok(rows);
        };
        req = PageReq {
            after: Some(after),
            limit: 256,
        };
    }
}

pub(crate) fn id_number(id: &str) -> u64 {
    id.trim_start_matches(['T', 't', '-'])
        .parse()
        .unwrap_or(u64::MAX)
}

pub(crate) fn atomic_write(path: &Path, content: &str) -> Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    fs::write(&tmp, content).with_context(|| format!("writing {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| format!("renaming to {}", path.display()))
}
