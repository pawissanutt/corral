//! Reuse decoding, never file observations: every lookup still reads the current bytes.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context, Result};

use crate::Task;

#[derive(Debug, Default)]
pub(super) struct TaskCache {
    entries: Mutex<HashMap<PathBuf, CachedTask>>,
    #[cfg(test)]
    decodes: std::sync::atomic::AtomicUsize,
}

#[derive(Debug)]
struct CachedTask {
    source: String,
    task: Task,
}

impl TaskCache {
    pub(super) fn read(&self, path: &Path) -> Result<Task> {
        let source =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        if let Some(cached) = self
            .entries
            .lock()
            .map_err(|e| anyhow::anyhow!("locking task cache: {e}"))?
            .get(path)
            && cached.source == source
        {
            return Ok(cached.task.clone());
        }
        #[cfg(test)]
        self.decodes
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let task = super::parse_task(&source, path)?;
        self.entries
            .lock()
            .map_err(|e| anyhow::anyhow!("locking task cache: {e}"))?
            .insert(
                path.to_path_buf(),
                CachedTask {
                    source,
                    task: task.clone(),
                },
            );
        Ok(task)
    }

    /// A listing is also the complete inventory of paths worth retaining.
    pub(super) fn retain(&self, paths: &[PathBuf]) -> Result<()> {
        self.entries
            .lock()
            .map_err(|e| anyhow::anyhow!("locking task cache: {e}"))?
            .retain(|path, _| paths.binary_search(path).is_ok());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;

    use crate::{Status, Store};

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
        std::fs::File::open(&task.path)
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
}
