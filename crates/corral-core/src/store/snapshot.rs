//! Read-only snapshots (plan T-253 section 5, "One view per evaluation"): a store bound to one
//! read view, so every read of a pass observes one committed state.

use std::any::Any;
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use anyhow::Result;

use super::Store;
use super::head::PinnedView;
use crate::Task;
use crate::backend::*;

/// The view a snapshot reads from, its listing once read, and the values derived in its pass.
pub(crate) struct Frozen {
    pub(crate) view: super::head::PinnedView,
    pub(crate) listing: Mutex<Option<Arc<Vec<Task>>>>,
    derived: Derived,
}

impl std::fmt::Debug for Frozen {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Frozen")
    }
}

fn locked<T>(mutex: &Mutex<T>) -> Result<std::sync::MutexGuard<'_, T>> {
    mutex
        .lock()
        .map_err(|e| anyhow::anyhow!("locking snapshot: {e}"))
}

#[derive(Default)]
struct Derived(Mutex<HashMap<String, Arc<dyn Any + Send + Sync>>>);

impl std::fmt::Debug for Derived {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Derived")
    }
}

impl Store {
    /// A copy of this store bound to one read view taken now: heads, tasks and listings all
    /// observe that state. Build one per read-only pass and drop it after; task writes through
    /// it are refused. A snapshot of a snapshot is the same snapshot.
    pub fn read_only_snapshot(&self) -> Result<Store> {
        if self.frozen.is_some() {
            return Ok(self.clone());
        }
        self.check_schema()?;
        let view = self.view()?;
        let mut store = self.clone();
        store.frozen = Some(Arc::new(Frozen {
            view: Arc::new(Mutex::new(view)),
            listing: Mutex::default(),
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

    /// Every task: the snapshot's listing when this store is one.
    pub fn task_view(&self) -> Result<Cow<'_, [Task]>> {
        Ok(Cow::Owned(self.list_shared()?.as_ref().clone()))
    }
}

/// A raw view handed out by a snapshot: every read goes to the snapshot's own pinned view.
pub(crate) struct SharedView(pub(crate) PinnedView);

impl SharedView {
    fn view(&self) -> std::sync::MutexGuard<'_, Box<dyn ReadView + Send>> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl ReadView for SharedView {
    fn seq(&self) -> ChangeSeq {
        self.view().seq()
    }
    fn ensure_fresh(&self) -> Result<(), ViewError> {
        self.view().ensure_fresh()
    }
    fn board(&self, since: Option<ChangeSeq>) -> Result<BoardDelta, FeedError> {
        self.view().board(since)
    }
    fn changes(&self, after: ChangeSeq, limit: u32) -> Result<ChangePage, FeedError> {
        self.view().changes(after, limit)
    }
    fn head(&self, num: TaskNum) -> Result<Option<TaskHeadDoc>, ViewError> {
        self.view().head(num)
    }
    fn heads(&self, q: &HeadQuery, page: PageReq) -> Result<Page<TaskHeadDoc>, ViewError> {
        self.view().heads(q, page)
    }
    fn task(&self, num: TaskNum, parts: &TaskParts) -> Result<Option<TaskDoc>, ViewError> {
        self.view().task(num, parts)
    }
    fn rows(
        &self,
        num: TaskNum,
        q: &RowQuery,
        page: PageReq,
    ) -> Result<Page<HistoryRow>, ViewError> {
        self.view().rows(num, q, page)
    }
    fn token_owners(
        &self,
        part: TaskPart,
        token: &str,
        limit: u32,
    ) -> Result<Vec<TaskNum>, ViewError> {
        self.view().token_owners(part, token, limit)
    }
    fn tasks(&self, q: &TaskQuery, page: PageReq) -> Result<Page<TaskHeader>, ViewError> {
        self.view().tasks(q, page)
    }
    fn lesson_artifacts(&self, num: TaskNum, limit: u32) -> Result<Vec<HistoryRow>, ViewError> {
        self.view().lesson_artifacts(num, limit)
    }
    fn lesson_approvals(&self, sha: &str) -> Result<Vec<LessonApproval>, ViewError> {
        self.view().lesson_approvals(sha)
    }
    fn events(&self, q: &EventQuery, page: PageReq) -> Result<Page<StoredEvent>, ViewError> {
        self.view().events(q, page)
    }
    fn conditions(
        &self,
        q: &ConditionQuery,
        page: PageReq,
    ) -> Result<Page<ConditionRun>, ViewError> {
        self.view().conditions(q, page)
    }
    fn records(&self, q: &RecordQuery, page: PageReq) -> Result<Page<StoredRecord>, ViewError> {
        self.view().records(q, page)
    }
    fn aggregates(&self, names: &[AggregateName]) -> Result<Aggregates, ViewError> {
        self.view().aggregates(names)
    }
    fn blobs(&self, q: &BlobQuery, page: PageReq) -> Result<Page<BlobRow>, ViewError> {
        self.view().blobs(q, page)
    }
    fn storage_rows(&self, q: &StorageQuery, page: PageReq) -> Result<Page<StorageRow>, ViewError> {
        self.view().storage_rows(q, page)
    }
}
