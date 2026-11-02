//! Failure-only diagnostics for the action budget. A test-only backend decorator records
//! the JSON documents returned to the facade during the measured action. These are decode
//! inputs, not a replacement for the work ledger: repeated backend reads may not all decode.
//! No document content is retained. The separate expected-walk thread is not captured.
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use anyhow::Result;
use corral_core::Store;
use corral_core::backend::*;
use corral_core::sqlite::{SqliteBackend, SqliteOptions};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub(super) struct Input {
    kind: String,
    key: String,
    bytes: usize,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub(super) struct Inputs {
    rows: Vec<(Input, u64)>,
    omitted: u64,
}

#[derive(Default)]
struct Active {
    rows: BTreeMap<Input, u64>,
    omitted: u64,
}

thread_local! {
    static ACTIVE: RefCell<Option<Active>> = const { RefCell::new(None) };
}

pub(super) struct Capture;

impl Capture {
    pub(super) fn start() -> Self {
        ACTIVE.with(|slot| {
            assert!(slot.borrow().is_none(), "nested action-budget capture");
            *slot.borrow_mut() = Some(Active::default());
        });
        Self
    }

    pub(super) fn finish(self) -> Inputs {
        ACTIVE.with(|slot| {
            let active = slot.borrow_mut().take().unwrap();
            Inputs {
                rows: active.rows.into_iter().collect(),
                omitted: active.omitted,
            }
        })
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        ACTIVE.with(|slot| *slot.borrow_mut() = None);
    }
}

impl Inputs {
    pub(super) fn print(&self, label: &str) {
        println!("DECODE INPUTS {label} (kind, key, JSON bytes, read count)");
        for (input, reads) in &self.rows {
            println!(
                "  {} {} bytes={} reads={reads}",
                input.kind, input.key, input.bytes
            );
        }
        if self.omitted != 0 {
            println!(
                "  diagnostic limit: {} additional inputs omitted",
                self.omitted
            );
        }
    }
}

fn note(kind: &str, key: String, doc: &Value) {
    ACTIVE.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(active) = slot.as_mut() else { return };
        let input = Input {
            kind: kind.into(),
            key,
            bytes: serde_json::to_vec(doc).unwrap().len(),
        };
        // Reserve half the diagnostic capacity for heads/current rows/records even when
        // the permitted FU-3 walk returns many unrelated history rows first.
        let limit = if kind.starts_with("row.") && kind.ends_with(".history") {
            10_000
        } else {
            20_000
        };
        if active.rows.len() < limit || active.rows.contains_key(&input) {
            *active.rows.entry(input).or_default() += 1;
        } else {
            active.omitted += 1;
        }
    });
}

fn head(doc: &TaskHeadDoc) {
    note("head", doc.header.id.clone(), &doc.doc);
    note("summary", doc.header.id.clone(), &doc.summary);
}

fn rows(num: TaskNum, rows: &[HistoryRow]) {
    for row in rows {
        let age = if row.current { "current" } else { "history" };
        note(
            &format!("row.{:?}.{age}", row.part),
            format!("T-{:03}/{}", num.0, row.key),
            &row.doc,
        );
    }
}

/// Preserve Store's caches/configuration; only decorate its database's returned documents.
/// Every backend operation and view-age argument is forwarded unchanged.
pub(super) fn decorate(store: &Store) -> Store {
    let inner = SqliteBackend::open(store.dir(), SqliteOptions::default()).unwrap();
    store.with_backend(Box::new(Backend(inner)))
}

struct Backend(SqliteBackend);
struct View(Box<dyn ReadView + Send>);

impl StoreBackend for Backend {
    fn view(&self) -> Result<Box<dyn ReadView + Send>, ViewError> {
        Ok(Box::new(View(self.0.view()?)))
    }
    fn view_with_max_age(&self, age: Duration) -> Result<Box<dyn ReadView + Send>, ViewError> {
        Ok(Box::new(View(self.0.view_with_max_age(age)?)))
    }
    fn apply(&self, op: Operation) -> Result<Committed, ApplyError> {
        self.0.apply(op)
    }
    fn check(&self) -> Result<IntegrityReport> {
        self.0.check()
    }
    fn backup_database(&self, dest: &Path) -> Result<()> {
        self.0.backup_database(dest)
    }
}

impl ReadView for View {
    fn seq(&self) -> ChangeSeq {
        self.0.seq()
    }
    fn ensure_fresh(&self) -> Result<(), ViewError> {
        self.0.ensure_fresh()
    }
    fn board(&self, since: Option<ChangeSeq>) -> Result<BoardDelta, FeedError> {
        let result = self.0.board(since)?;
        for doc in &result.heads {
            head(doc);
        }
        Ok(result)
    }
    fn changes(&self, after: ChangeSeq, limit: u32) -> Result<ChangePage, FeedError> {
        self.0.changes(after, limit)
    }
    fn head(&self, num: TaskNum) -> Result<Option<TaskHeadDoc>, ViewError> {
        let result = self.0.head(num)?;
        if let Some(doc) = &result {
            head(doc);
        }
        Ok(result)
    }
    fn heads(&self, q: &HeadQuery, page: PageReq) -> Result<Page<TaskHeadDoc>, ViewError> {
        let result = self.0.heads(q, page)?;
        for doc in &result.items {
            head(doc);
        }
        Ok(result)
    }
    fn task(&self, num: TaskNum, parts: &TaskParts) -> Result<Option<TaskDoc>, ViewError> {
        let result = self.0.task(num, parts)?;
        if let Some(doc) = &result {
            head(&doc.head);
            rows(num, &doc.rows);
        }
        Ok(result)
    }
    fn rows(
        &self,
        num: TaskNum,
        q: &RowQuery,
        page: PageReq,
    ) -> Result<Page<HistoryRow>, ViewError> {
        let result = self.0.rows(num, q, page)?;
        rows(num, &result.items);
        Ok(result)
    }
    fn rows_except(
        &self,
        num: TaskNum,
        q: &RowQuery,
        except: &str,
        page: PageReq,
    ) -> Result<Page<HistoryRow>, ViewError> {
        let result = self.0.rows_except(num, q, except, page)?;
        rows(num, &result.items);
        Ok(result)
    }
    fn token_owners(
        &self,
        part: TaskPart,
        token: &str,
        limit: u32,
    ) -> Result<Vec<TaskNum>, ViewError> {
        self.0.token_owners(part, token, limit)
    }
    fn lesson_artifacts(&self, num: TaskNum, limit: u32) -> Result<Vec<HistoryRow>, ViewError> {
        let result = self.0.lesson_artifacts(num, limit)?;
        rows(num, &result);
        Ok(result)
    }
    fn lesson_approvals(&self, sha: &str) -> Result<Vec<LessonApproval>, ViewError> {
        self.0.lesson_approvals(sha)
    }
    fn tasks(&self, q: &TaskQuery, page: PageReq) -> Result<Page<TaskHeader>, ViewError> {
        self.0.tasks(q, page)
    }
    fn run_worktrees(&self, current_only: bool) -> Result<Vec<RunWorktree>, ViewError> {
        self.0.run_worktrees(current_only)
    }
    fn last_task_num(&self) -> Result<Option<TaskNum>, ViewError> {
        self.0.last_task_num()
    }
    fn events(&self, q: &EventQuery, page: PageReq) -> Result<Page<StoredEvent>, ViewError> {
        let result = self.0.events(q, page)?;
        for row in &result.items {
            note("event.detail", row.seq.0.to_string(), &row.event.detail);
        }
        Ok(result)
    }
    fn conditions(
        &self,
        q: &ConditionQuery,
        page: PageReq,
    ) -> Result<Page<ConditionRun>, ViewError> {
        self.0.conditions(q, page)
    }
    fn records(&self, q: &RecordQuery, page: PageReq) -> Result<Page<StoredRecord>, ViewError> {
        let result = self.0.records(q, page)?;
        for row in &result.items {
            note("record", format!("{}/{}", row.area, row.key), &row.doc);
        }
        Ok(result)
    }
    fn aggregates(&self, names: &[AggregateName]) -> Result<Aggregates, ViewError> {
        self.0.aggregates(names)
    }
    fn blobs(&self, q: &BlobQuery, page: PageReq) -> Result<Page<BlobRow>, ViewError> {
        self.0.blobs(q, page)
    }
    fn storage_rows(&self, q: &StorageQuery, page: PageReq) -> Result<Page<StorageRow>, ViewError> {
        let result = self.0.storage_rows(q, page)?;
        for row in &result.items {
            if let StorageRow::Authorization(row) = row {
                note(
                    "authorization",
                    format!("{}/{}", row.task, row.id),
                    &row.doc,
                );
            }
        }
        Ok(result)
    }
}

#[test]
fn decorated_sqlite_reads_preserve_results_and_work_counts() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let task = store
        .create("decode input", None, corral_core::Status::Ready, "")
        .unwrap();
    let area = corral_core::records::RecordArea::<Value>::new("diagnostic-fixture");
    area.save(&store, "input", &serde_json::json!({"value": 9}))
        .unwrap();
    let read = |store: &Store| {
        let before = corral_core::work::snapshot();
        let _hot = corral_core::work::HotScope::enter();
        let head = store.head(&task.id).unwrap().to_shell_task().unwrap();
        let events = store.events_of(Some(&task.id), &["created"]).unwrap();
        let record = area.load(store, "input").unwrap();
        (
            (head, events, record),
            corral_core::work::snapshot().since(&before),
        )
    };
    let expected = read(&store);
    let wrapped = decorate(&store);
    let capture = Capture::start();
    let actual = read(&wrapped);
    let inputs = capture.finish();
    assert_eq!(
        actual, expected,
        "diagnostics must not alter results or work accounting"
    );
    for kind in ["head", "summary", "event.detail", "record"] {
        assert!(
            inputs.rows.iter().any(|(input, _)| input.kind == kind),
            "missing {kind}"
        );
    }
}

#[test]
fn capture_names_distinct_widths_and_repeated_reads_without_changing_the_ledger() {
    let before = corral_core::work::snapshot();
    let capture = Capture::start();
    note(
        "authorization",
        "example".into(),
        &serde_json::json!({"attempt":"start-9-123"}),
    );
    note(
        "authorization",
        "example".into(),
        &serde_json::json!({"attempt":"start-9-123"}),
    );
    note(
        "authorization",
        "example".into(),
        &serde_json::json!({"attempt":"start-999-123"}),
    );
    let trace = capture.finish();
    assert_eq!(trace.rows.len(), 2);
    assert_eq!(trace.rows[0].1, 2);
    assert_eq!(trace.rows[1].0.bytes - trace.rows[0].0.bytes, 2);
    assert_eq!(corral_core::work::snapshot(), before);
    let wire = serde_json::to_string(&trace).unwrap();
    assert!(!wire.contains("start-"), "only metadata may be retained");
    assert_eq!(
        serde_json::from_str::<Inputs>(&wire).unwrap().rows,
        trace.rows
    );
}
