//! Reuse decoding always, and file reads and directory listings only while settled
//! (see [`crate::file_stamp`]); everything else reads the current bytes again.
//! A read-only snapshot answers every lookup from one read of the board.

use std::any::Any;
use std::borrow::Cow;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result};

use super::Store;
use crate::Task;
use crate::file_stamp::{FileStamp, SETTLE_WINDOW};
use crate::work::{self, Work};

/// Most threads a read-only snapshot decodes task files on, and the fewest files each must
/// have before another one starts (small boards stay on the calling thread).
const READERS: usize = 8;
const PER_READER: usize = 16;

#[derive(Debug)]
pub(super) struct TaskCache {
    entries: Mutex<HashMap<PathBuf, CachedTask>>,
    listing: Mutex<Option<(FileStamp, Vec<PathBuf>)>>,
    /// Bumped whenever a cached task is decoded anew or dropped.
    generation: AtomicU64,
    snapshot: Mutex<Option<Snapshot>>,
    settle_nanos: AtomicU64,
    #[cfg(test)]
    decodes: std::sync::atomic::AtomicUsize,
}

impl Default for TaskCache {
    fn default() -> Self {
        TaskCache {
            entries: Mutex::default(),
            listing: Mutex::default(),
            generation: AtomicU64::new(0),
            snapshot: Mutex::default(),
            settle_nanos: AtomicU64::new(SETTLE_WINDOW.as_nanos() as u64),
            #[cfg(test)]
            decodes: Default::default(),
        }
    }
}

/// Every task of one listing, shared until a task file or the directory changes.
#[derive(Debug)]
struct Snapshot {
    generation: u64,
    paths: Vec<PathBuf>,
    tasks: Arc<Vec<Task>>,
}

#[derive(Debug)]
struct CachedTask {
    source: String,
    task: Arc<Task>,
    /// The stamp observed before `source` was read, kept only if the file had settled.
    settled: Option<FileStamp>,
}

fn locked<T>(mutex: &Mutex<T>) -> Result<std::sync::MutexGuard<'_, T>> {
    mutex
        .lock()
        .map_err(|e| anyhow::anyhow!("locking task cache: {e}"))
}

impl TaskCache {
    /// Reuse bytes encoded by this writer, with the identity of the file it actually wrote.
    /// Unsettled files still require byte comparison on the next read, exactly as before.
    pub(super) fn written(&self, task: &Task, source: String, file: &std::fs::File) -> Result<()> {
        crate::work::record(Work::TaskStat, 1);
        let metadata = file.metadata().context("inspecting written task")?;
        let settled = FileStamp::settled(&metadata, SystemTime::now(), self.window());
        let mut entries = locked(&self.entries)?;
        self.generation.fetch_add(1, Ordering::SeqCst);
        entries.insert(
            task.path.clone(),
            CachedTask {
                source,
                task: Arc::new(task.clone()),
                settled,
            },
        );
        Ok(())
    }

    fn window(&self) -> Duration {
        Duration::from_nanos(self.settle_nanos.load(Ordering::Relaxed))
    }

    pub(super) fn read(&self, path: &Path) -> Result<Task> {
        self.observe(path).map(|(task, _)| task.as_ref().clone())
    }

    /// The current task, and whether this call decoded it anew.
    fn observe(&self, path: &Path) -> Result<(Arc<Task>, bool)> {
        let observed = SystemTime::now();
        crate::work::record(crate::work::Work::TaskStat, 1);
        let metadata =
            std::fs::metadata(path).with_context(|| format!("reading {}", path.display()))?;
        if let Some(cached) = locked(&self.entries)?.get(path)
            && cached.settled.is_some()
            && cached.settled == FileStamp::of(&metadata)
        {
            return Ok((Arc::clone(&cached.task), false));
        }
        let source =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        crate::work::record(crate::work::Work::TaskRead, 1);
        crate::work::record(crate::work::Work::TaskBytes, source.len() as u64);
        let settled = FileStamp::settled(&metadata, observed, self.window());
        if let Some(cached) = locked(&self.entries)?.get_mut(path)
            && cached.source == source
        {
            cached.settled = settled;
            return Ok((Arc::clone(&cached.task), false));
        }
        #[cfg(test)]
        self.decodes
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        crate::work::record(crate::work::Work::TaskParse, 1);
        // Decoded unlocked so a snapshot's parallel readers decode different files at once; a
        // concurrent decode of the same file stores the same task and moves the generation too.
        let task = Arc::new(super::parse_task(&source, path)?);
        let mut entries = locked(&self.entries)?;
        self.generation.fetch_add(1, Ordering::SeqCst);
        entries.insert(
            path.to_path_buf(),
            CachedTask {
                source,
                task: Arc::clone(&task),
                settled,
            },
        );
        Ok((task, true))
    }

    /// [`Self::read`] of every path, in order.
    fn read_all(&self, paths: &[PathBuf]) -> Vec<Result<Task>> {
        self.observe_all(paths)
            .into_iter()
            .map(|observed| observed.map(|(task, _)| task.as_ref().clone()))
            .collect()
    }

    /// [`Self::observe`] of every path, in order, decoding on up to [`READERS`] threads. The
    /// readers' work counts are added to the calling thread's, so totals match a serial read.
    fn observe_all(&self, paths: &[PathBuf]) -> Vec<Result<(Arc<Task>, bool)>> {
        let readers = std::thread::available_parallelism()
            .map_or(1, usize::from)
            .min(READERS)
            .min(paths.len() / PER_READER);
        if readers <= 1 {
            return paths.iter().map(|path| self.observe(path)).collect();
        }
        let next = AtomicUsize::new(0);
        let mut results: Vec<Option<_>> = paths.iter().map(|_| None).collect();
        std::thread::scope(|scope| {
            let workers: Vec<_> = (0..readers)
                .map(|_| {
                    scope.spawn(|| {
                        let start = Work::ALL.map(work::count);
                        let mut read = Vec::new();
                        loop {
                            let index = next.fetch_add(1, Ordering::Relaxed);
                            let Some(path) = paths.get(index) else { break };
                            read.push((index, self.observe(path)));
                        }
                        let done = Work::ALL.map(work::count);
                        let counts: [u64; Work::ALL.len()] =
                            std::array::from_fn(|i| done[i] - start[i]);
                        (read, counts)
                    })
                })
                .collect();
            for worker in workers {
                // A reader panics only where a serial read would have; propagate it unchanged.
                let (read, counts) = worker
                    .join()
                    .unwrap_or_else(|panic| std::panic::resume_unwind(panic));
                for (work, n) in Work::ALL.into_iter().zip(counts) {
                    work::record(work, n);
                }
                for (index, task) in read {
                    results[index] = Some(task);
                }
            }
        });
        results
            .into_iter()
            .zip(paths)
            .map(|(read, path)| read.unwrap_or_else(|| self.observe(path)))
            .collect()
    }

    /// The tasks of `paths` sorted by id, observing every file exactly as [`Self::read`] does.
    /// When no file decoded anew and none failed since the last listing of the same paths, the
    /// previous listing is shared instead of copied; unreadable files go to `skipped`.
    pub(super) fn listing(
        &self,
        paths: &[PathBuf],
        mut skipped: impl FnMut(&Path, anyhow::Error),
    ) -> Result<Arc<Vec<Task>>> {
        // Another thread's decode during this observation could pair a stale task with a newer
        // generation, so only this call's own decodes may move the generation.
        let start = self.generation.load(Ordering::SeqCst);
        let mut decoded = 0;
        let mut observed = Vec::with_capacity(paths.len());
        let mut complete = true;
        for (path, observation) in paths.iter().zip(self.observe_all(paths)) {
            match observation {
                Ok((task, fresh)) => {
                    decoded += u64::from(fresh);
                    observed.push(task);
                }
                Err(error) => {
                    complete = false;
                    skipped(path, error);
                }
            }
        }
        let generation = self.generation.load(Ordering::SeqCst);
        let complete = complete && generation == start + decoded;
        let mut snapshot = locked(&self.snapshot)?;
        if complete
            && let Some(previous) = snapshot.as_ref()
            && previous.generation == generation
            && previous.paths == paths
        {
            return Ok(Arc::clone(&previous.tasks));
        }
        let mut tasks: Vec<Task> = observed.iter().map(|task| task.as_ref().clone()).collect();
        tasks.sort_by_key(|t| (super::id_number(&t.id), t.id.clone()));
        let tasks = Arc::new(tasks);
        *snapshot = complete.then(|| Snapshot {
            generation,
            paths: paths.to_vec(),
            tasks: Arc::clone(&tasks),
        });
        Ok(tasks)
    }

    /// The sorted task files of `dir`, listed again by `list` unless the directory has not
    /// gained, lost or renamed an entry since a settled listing.
    pub(super) fn files(
        &self,
        dir: &Path,
        list: impl FnOnce() -> Result<Vec<PathBuf>>,
    ) -> Result<Vec<PathBuf>> {
        let observed = SystemTime::now();
        let metadata = match std::fs::metadata(dir) {
            Ok(metadata) => metadata,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return list(),
            Err(e) => return Err(e).with_context(|| format!("reading {}", dir.display())),
        };
        let mut listing = locked(&self.listing)?;
        if let Some((stamp, paths)) = listing.as_ref()
            && FileStamp::of(&metadata) == Some(*stamp)
        {
            return Ok(paths.clone());
        }
        let paths = list()?;
        *listing = FileStamp::settled(&metadata, observed, self.window())
            .map(|stamp| (stamp, paths.clone()));
        Ok(paths)
    }

    /// For fixtures that cannot age a file's ctime: trust stamps of files changed earlier than
    /// `window` before an observation. Production keeps [`SETTLE_WINDOW`].
    pub(super) fn set_settle_window(&self, window: Duration) {
        self.settle_nanos
            .store(window.as_nanos() as u64, Ordering::Relaxed);
    }

    /// A listing is also the complete inventory of paths worth retaining.
    pub(super) fn retain(&self, paths: &[PathBuf]) -> Result<()> {
        let mut entries = locked(&self.entries)?;
        let before = entries.len();
        entries.retain(|path, _| paths.binary_search(path).is_ok());
        if entries.len() != before {
            self.generation.fetch_add(1, Ordering::SeqCst);
        }
        Ok(())
    }
}

/// One coherent read of every task file for a read-only pass (a policy resolve, a PM
/// workset) that would otherwise re-read the whole board on every lookup.
#[derive(Debug)]
pub(super) struct Frozen {
    /// Parsed tasks in `Store::list` order.
    tasks: Arc<Vec<Task>>,
    /// File id number -> index into `tasks`, or that file's read error; the first file wins.
    by_number: HashMap<u64, Result<usize, String>>,
    /// Values derived once per pass, by key (see [`Store::pass_derived`]).
    derived: Derived,
}

#[derive(Default)]
struct Derived(Mutex<HashMap<String, Arc<dyn Any + Send + Sync>>>);

impl std::fmt::Debug for Derived {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Derived")
    }
}

impl Frozen {
    pub(super) fn shared(&self) -> Arc<Vec<Task>> {
        Arc::clone(&self.tasks)
    }

    pub(super) fn get(&self, id: &str, number: u64) -> Result<Task> {
        match self.by_number.get(&number) {
            Some(Ok(index)) => self
                .tasks
                .get(*index)
                .cloned()
                .context("task snapshot index"),
            Some(Err(error)) => Err(anyhow::anyhow!("{error}")),
            None => anyhow::bail!("task {id} not found"),
        }
    }
}

impl Store {
    /// A copy of this store whose task lookups all come from one read of the board, taken now.
    /// Build one per read-only pass and drop it after; task writes through it are refused.
    /// A snapshot of a snapshot is the same snapshot.
    pub fn read_only_snapshot(&self) -> Result<Store> {
        if self.frozen.is_some() {
            return Ok(self.clone());
        }
        self.check_schema()?;
        crate::work::record(crate::work::Work::TaskList, 1);
        let paths = self.task_files()?;
        self.task_cache.retain(&paths)?;
        let mut tasks = Vec::new();
        let mut errors = Vec::new();
        for ((order, path), read) in paths
            .iter()
            .enumerate()
            .zip(self.task_cache.read_all(&paths))
        {
            let number = super::file_id_number(path);
            match read {
                Ok(task) => tasks.push((order, number, task)),
                Err(error) => {
                    crate::messages::report_file(
                        path,
                        format!("skipping {}: {error:#}", path.display()),
                    );
                    errors.push((order, number, format!("{error:#}")));
                }
            }
        }
        tasks.sort_by_key(|(_, _, task)| (super::id_number(&task.id), task.id.clone()));
        // `get` answers from the first file, in path order, that carries the id number.
        let mut first = HashMap::<u64, (usize, Result<usize, String>)>::new();
        let entries = tasks
            .iter()
            .enumerate()
            .map(|(index, (order, number, _))| (*order, *number, Ok(index)))
            .chain(
                errors
                    .into_iter()
                    .map(|(order, number, error)| (order, number, Err(error))),
            );
        for (order, number, entry) in entries {
            let Some(number) = number else { continue };
            if first.get(&number).is_none_or(|(seen, _)| order < *seen) {
                first.insert(number, (order, entry));
            }
        }
        let mut store = self.clone();
        store.frozen = Some(Arc::new(Frozen {
            tasks: Arc::new(tasks.into_iter().map(|(_, _, task)| task).collect()),
            by_number: first
                .into_iter()
                .map(|(number, (_, entry))| (number, entry))
                .collect(),
            derived: Derived::default(),
        }));
        Ok(store)
    }

    pub fn is_snapshot(&self) -> bool {
        self.frozen.is_some()
    }

    /// A value derived during this snapshot's read-only pass, computed at most once per `key`
    /// while the snapshot lives; a live store computes it on every call. `key` must name the
    /// derivation and every input it depends on. Errors are returned and never kept.
    pub fn pass_derived<T: Any + Send + Sync>(
        &self,
        key: &str,
        compute: impl FnOnce() -> Result<T>,
    ) -> Result<Arc<T>> {
        let Some(frozen) = &self.frozen else {
            return compute().map(Arc::new);
        };
        let known = locked(&frozen.derived.0)?.get(key).cloned();
        if let Some(value) = known.and_then(|value| value.downcast::<T>().ok()) {
            return Ok(value);
        }
        // Computed unlocked so a derivation may itself read other derived values.
        let value = Arc::new(compute()?);
        locked(&frozen.derived.0)?.insert(key.to_owned(), value.clone());
        Ok(value)
    }

    /// Every task, borrowed from the snapshot when this store is one.
    pub fn task_view(&self) -> Result<Cow<'_, [Task]>> {
        match &self.frozen {
            Some(frozen) => Ok(Cow::Borrowed(frozen.tasks.as_slice())),
            None => self.list().map(Cow::Owned),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;

    use crate::{Status, Store};

    #[test]
    fn written_cache_identity_follows_retained_file_not_replacement_path() {
        // Given our writer handle and an external replacement of its pathname.
        let dir = tempfile::tempdir().unwrap();
        let store = Store::init(dir.path()).unwrap();
        let task = store
            .create("Retained", None, Status::Backlog, "before\n")
            .unwrap();
        store.set_task_settle_window(std::time::Duration::ZERO);
        let source = crate::render_task(&task);
        let file = super::super::atomic_write_retained(&task.path, &source).unwrap();
        let mut changed = task.clone();
        changed.body = "after!\n".into();
        super::super::atomic_write(&task.path, &crate::render_task(&changed)).unwrap();
        // When the original writer seeds its cache after that replacement.
        store.task_cache.written(&task, source, &file).unwrap();
        // Then the new pathname identity cannot masquerade as the writer's bytes.
        assert_eq!(store.get(&task.id).unwrap(), changed);
    }

    #[test]
    fn attention_write_reuses_encoded_bytes_without_hiding_external_edits() {
        // Create under the normal settle policy: ID allocation lists the empty directory.
        // Trusting stamps before creation can retain that empty listing if the directory's
        // timestamps collide. Only enable stamp trust once the fixture exists.
        let dir = tempfile::tempdir().unwrap();
        let store = Store::init(dir.path()).unwrap();
        let task = store
            .create("Written", None, Status::Backlog, "before\n")
            .unwrap();
        store.set_task_settle_window(std::time::Duration::ZERO);
        store.get(&task.id).unwrap();
        let decodes = store.task_cache.decodes.load(Ordering::Relaxed);
        // When an attention transaction persists bytes it already knows.
        let saved = store
            .update_attention(&task.id, |ledger| {
                ledger.overflow_count = Some(1);
                Ok(())
            })
            .unwrap();
        // Then the reader reuses the encoding, but still observes an external replacement.
        assert_eq!(store.get(&task.id).unwrap(), saved);
        assert_eq!(store.task_cache.decodes.load(Ordering::Relaxed), decodes);
        let mut changed = saved;
        changed.body = "after!\n".into();
        super::super::atomic_write(&changed.path, &crate::render_task(&changed)).unwrap();
        assert_eq!(store.get(&task.id).unwrap(), changed);
        assert_eq!(
            store.task_cache.decodes.load(Ordering::Relaxed),
            decodes + 1
        );
    }

    #[test]
    fn attention_write_cache_stress() {
        // Each worker owns its store; exercise fixture setup and replacements under load.
        std::thread::scope(|scope| {
            for _ in 0..8 {
                scope.spawn(|| {
                    for _ in 0..25 {
                        attention_write_reuses_encoded_bytes_without_hiding_external_edits();
                    }
                });
            }
        });
    }

    #[test]
    fn attention_write_does_not_trust_unsettled_file_stamps() {
        // Given the normal settle policy and freshly encoded attention bytes.
        let dir = tempfile::tempdir().unwrap();
        let store = Store::init(dir.path()).unwrap();
        let task = store
            .create("Unsettled", None, Status::Backlog, "before\n")
            .unwrap();
        store
            .update_attention(&task.id, |ledger| {
                ledger.overflow_count = Some(1);
                Ok(())
            })
            .unwrap();
        // When an external writer changes equal-length bytes and restores mtime.
        let metadata = std::fs::metadata(&task.path).unwrap();
        let text = std::fs::read_to_string(&task.path)
            .unwrap()
            .replace("before", "after!");
        std::fs::write(&task.path, text).unwrap();
        std::fs::File::options()
            .write(true)
            .open(&task.path)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(metadata.modified().unwrap()))
            .unwrap();
        // Then the cache must compare current bytes, not trust the writer's recent stamp.
        assert_eq!(store.get(&task.id).unwrap().body, "after!\n");
    }

    #[test]
    fn unchanged_tasks_are_decoded_once_across_store_clones() {
        // Given a task warmed by one store.
        let dir = tempfile::tempdir().unwrap();
        let store = Store::init(dir.path()).unwrap();
        let task = store
            .create("Unchanged", None, Status::Backlog, "body\n")
            .unwrap();
        let expected = store.get(&task.id).unwrap();
        let decodes = store.task_cache.decodes.load(Ordering::Relaxed);
        let cloned = store.clone();

        // When UI and worker readers repeatedly list and get the same bytes.
        assert_eq!(cloned.list().unwrap(), vec![expected.clone()]);
        assert_eq!(cloned.get(&task.id).unwrap(), expected);
        assert_eq!(store.list().unwrap(), vec![expected]);

        // Then no reader repeats the YAML decode.
        assert_eq!(store.task_cache.decodes.load(Ordering::Relaxed), decodes);
    }

    #[test]
    fn same_size_same_mtime_external_change_is_visible() {
        // Given cached bytes and a writer that preserves metadata.
        let dir = tempfile::tempdir().unwrap();
        let store = Store::init(dir.path()).unwrap();
        let task = store
            .create("External", None, Status::Backlog, "before\n")
            .unwrap();
        let original = store.get(&task.id).unwrap();
        let modified = std::fs::metadata(&task.path).unwrap().modified().unwrap();
        let mut changed = original.clone();
        changed.status = Status::Running;
        changed.body = "after!\n".into();
        let text = crate::render_task(&changed);
        assert_eq!(text.len(), crate::render_task(&original).len());

        // When another process edits the file without a usable mtime/size signal.
        std::fs::write(&task.path, text).unwrap();
        std::fs::OpenOptions::new()
            .write(true)
            .open(&task.path)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(modified))
            .unwrap();

        // Then both access paths show current status and body, not cached metadata.
        assert_eq!(store.get(&task.id).unwrap(), changed);
        assert_eq!(store.clone().list().unwrap(), vec![changed]);
    }

    #[test]
    fn replacements_and_invalid_files_do_not_return_cached_tasks() {
        // Given an already decoded task.
        let dir = tempfile::tempdir().unwrap();
        let store = Store::init(dir.path()).unwrap();
        let task = store.create("Original", None, Status::Backlog, "").unwrap();
        store.get(&task.id).unwrap();
        let mut replacement = task.clone();
        replacement.status = Status::Review;
        let path = task.path.with_extension("replacement");
        std::fs::write(&path, crate::render_task(&replacement)).unwrap();

        // When a writer atomically replaces the task.
        std::fs::rename(path, &task.path).unwrap();

        // Then replacement, invalidation, deletion and repair are observed.
        assert_eq!(store.list().unwrap(), vec![replacement.clone()]);
        std::fs::write(&task.path, "---\nschema_version: 999\ntask: {}\n---\n").unwrap();
        assert!(store.get(&task.id).is_err());
        assert!(store.list().unwrap().is_empty());
        std::fs::remove_file(&task.path).unwrap();
        assert!(store.list().unwrap().is_empty());
        assert!(store.task_cache.entries.lock().unwrap().is_empty());
        assert!(store.get(&task.id).is_err());
        std::fs::write(&task.path, crate::render_task(&replacement)).unwrap();
        assert_eq!(store.get(&task.id).unwrap(), replacement);
    }

    #[test]
    fn changing_a_returned_task_does_not_mutate_the_cached_record() {
        // Given a task returned to a caller.
        let dir = tempfile::tempdir().unwrap();
        let store = Store::init(dir.path()).unwrap();
        let task = store
            .create("Independent", None, Status::Backlog, "")
            .unwrap();
        let mut returned = store.get(&task.id).unwrap();

        // When the caller modifies its value without saving it.
        returned.status = Status::Done;

        // Then the file-backed record remains authoritative.
        assert_eq!(store.get(&task.id).unwrap(), task);
    }

    #[test]
    fn a_snapshot_answers_from_its_own_read_and_refuses_writes() {
        // Given a snapshot taken before another writer changes the task.
        let dir = tempfile::tempdir().unwrap();
        let store = Store::init(dir.path()).unwrap();
        let task = store.create("Frozen", None, Status::Backlog, "").unwrap();
        let snapshot = store.read_only_snapshot().unwrap();
        let changed = store
            .update(&task.id, |task| {
                task.status = Status::Review;
                Ok(())
            })
            .unwrap();

        // When both are read after the change.
        // Then the snapshot keeps its one coherent view and the store sees the file.
        assert_eq!(snapshot.get(&task.id).unwrap(), task);
        assert_eq!(snapshot.get("1").unwrap(), task);
        assert_eq!(snapshot.list().unwrap(), vec![task.clone()]);
        assert!(snapshot.get("T-002").is_err());
        assert_eq!(store.get(&task.id).unwrap(), changed);
        // And no task write goes through a snapshot.
        assert!(snapshot.save(&changed).is_err());
        assert!(snapshot.read_only_snapshot().unwrap().is_snapshot());
    }

    #[cfg(unix)]
    #[test]
    fn settled_unchanged_files_are_not_reread_and_changes_still_are() {
        use crate::work::{Work, count};
        // Given a store that treats every file as settled, warmed by one listing.
        let dir = tempfile::tempdir().unwrap();
        let store = Store::init(dir.path()).unwrap();
        let task = store
            .create("Settled", None, Status::Backlog, "a\n")
            .unwrap();
        store.set_task_settle_window(std::time::Duration::ZERO);
        let warmed = store.list().unwrap();
        let reads = count(Work::TaskRead);

        // When the unchanged file is listed and fetched again.
        assert_eq!(store.list().unwrap(), warmed);
        assert_eq!(store.get(&task.id).unwrap(), warmed[0]);

        // Then neither lookup reads its bytes.
        assert_eq!(count(Work::TaskRead), reads);

        // When a writer replaces the file and later grows it in place.
        let mut replaced = warmed[0].clone();
        replaced.status = Status::Review;
        let staged = task.path.with_extension("staged");
        std::fs::write(&staged, crate::render_task(&replaced)).unwrap();
        std::fs::rename(&staged, &task.path).unwrap();
        assert_eq!(store.get(&task.id).unwrap(), replaced);
        let mut grown = replaced.clone();
        grown.body = "a longer body\n".into();
        std::fs::write(&task.path, crate::render_task(&grown)).unwrap();

        // Then both changes are observed.
        assert_eq!(store.list().unwrap(), vec![grown]);
    }

    #[test]
    fn a_large_snapshot_decodes_in_parallel_with_serial_order_and_counts() {
        use crate::work::{Work, count};
        // Given a board large enough for parallel readers, with one unreadable file.
        let dir = tempfile::tempdir().unwrap();
        let store = Store::init(dir.path()).unwrap();
        let tasks: Vec<_> = (0..super::READERS * super::PER_READER)
            .map(|n| {
                let body = format!("body {n}\n");
                store.create(&format!("Task {n}"), None, Status::Backlog, &body)
            })
            .collect::<anyhow::Result<_>>()
            .unwrap();
        std::fs::write(store.dir().join("tasks/T-999-bad.md"), "---\nnot: [\n---\n").unwrap();
        let serial = Store::open(dir.path()).unwrap();
        let tasks: Vec<_> = tasks.iter().map(|t| serial.get(&t.id).unwrap()).collect();
        let fresh = Store::open(dir.path()).unwrap();
        let start = Work::ALL.map(count);

        // When a fresh process takes a snapshot.
        let snapshot = fresh.read_only_snapshot().unwrap();

        // Then it holds every readable task in id order, decoded once each, and the calling
        // thread's counters include every reader's work.
        assert_eq!(snapshot.list().unwrap(), tasks);
        assert!(snapshot.get("T-999").is_err());
        let delta = |work: Work| count(work) - start[work as usize];
        assert_eq!(delta(Work::TaskStat), tasks.len() as u64 + 1);
        assert_eq!(delta(Work::TaskParse), tasks.len() as u64 + 1);
        assert_eq!(
            fresh.task_cache.decodes.load(Ordering::Relaxed),
            tasks.len() + 1
        );
        // And a later listing reuses those decodes; only the unreadable file is tried again.
        assert_eq!(fresh.list().unwrap(), tasks);
        assert_eq!(delta(Work::TaskParse), tasks.len() as u64 + 2);
    }

    #[test]
    fn pass_derived_values_are_computed_once_per_snapshot() {
        // Given a live store and a snapshot of it.
        let dir = tempfile::tempdir().unwrap();
        let store = Store::init(dir.path()).unwrap();
        let snapshot = store.read_only_snapshot().unwrap();
        let calls = std::cell::Cell::new(0);
        let derive = |s: &Store, key: &str| {
            *s.pass_derived(key, || {
                calls.set(calls.get() + 1);
                Ok(calls.get())
            })
            .unwrap()
        };

        // When each derives the same keys repeatedly.
        // Then the snapshot keeps one value per key, a failure is not kept, and the live
        // store computes every time.
        assert_eq!((derive(&snapshot, "a"), derive(&snapshot, "a")), (1, 1));
        assert_eq!(derive(&snapshot.clone(), "b"), 2);
        assert!(
            snapshot
                .pass_derived::<u8>("c", || anyhow::bail!("no"))
                .is_err()
        );
        assert_eq!(*snapshot.pass_derived("c", || Ok(7u8)).unwrap(), 7);
        assert_eq!((derive(&store, "a"), derive(&store, "a")), (3, 4));
    }

    #[test]
    fn unchanged_listings_share_one_snapshot_until_a_task_changes() {
        // Given a settled store listed once.
        let dir = tempfile::tempdir().unwrap();
        let store = Store::init(dir.path()).unwrap();
        let first = store.create("First", None, Status::Backlog, "").unwrap();
        store.set_task_settle_window(std::time::Duration::ZERO);
        let listed = store.list_shared().unwrap();

        // When nothing changes, then clones of the store share the same listing.
        assert!(std::sync::Arc::ptr_eq(
            &listed,
            &store.clone().list_shared().unwrap()
        ));
        assert_eq!(store.list().unwrap(), *listed);

        // When a task is edited (a new inode and size), then the next listing shows it.
        store
            .update(&first.id, |task| {
                task.title = "Edited".into();
                Ok(())
            })
            .unwrap();
        let edited = store.list_shared().unwrap();
        assert!(!std::sync::Arc::ptr_eq(&listed, &edited));
        assert_eq!(edited[0].title, "Edited");
    }
}
