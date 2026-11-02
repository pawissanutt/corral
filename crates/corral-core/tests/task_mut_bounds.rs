//! T-331 review round 1 regressions on both backends: scalar child edits read and write only
//! the edited child's row (F1); an excluded finding's row is never returned by the backend
//! (F3); a mutable head's keyed reads see its own queued edits, as the full task does (F4).
//! T-332: task-number allocation reads no page of task headers.

mod store_support;
#[path = "task_ref_support/mod.rs"]
mod task_ref_support;

use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use corral_core::backend::fake::FakeBackend;
use corral_core::backend::*;
use corral_core::backend::{
    Aggregates, BlobQuery, BlobRow, BoardDelta, ChangePage, ConditionQuery, ConditionRun,
    EventQuery, FeedError, HeadQuery, IntegrityReport, LessonApproval, Page, RecordQuery,
    StorageQuery, StorageRow, StoredEvent, StoredRecord, TaskDoc, TaskHeadDoc, TaskHeader,
    TaskQuery,
};
use corral_core::inbox::{ObserveCursor, Recipient, SessionBinding, SessionCursor};
use corral_core::work::{self, Counter, HotScope, WorkSnapshot};
use corral_core::{FindingSource, Run, Status, Store, Task, TaskMut, TaskRef, Verdict};
use serde_json::json;
use store_support::*;
use task_ref_support::{backends, norm_plan_run, norm_run};

fn measure<T>(f: impl FnOnce() -> T) -> (T, WorkSnapshot) {
    let _hot = HotScope::enter();
    let before = work::snapshot();
    let value = f();
    let delta = work::snapshot().since(&before);
    delta.check_identity().unwrap();
    (value, delta)
}

/// A current run whose first review child and first plan review carry `nested` of every
/// nested collection.
fn nested_task(store: &Store, nested: usize) -> Task {
    store.fixture_pin_now(at(0)).unwrap();
    let mut task = store
        .create("Bounds", Some("app"), Status::Running, "Spec\n")
        .unwrap();
    (task.created, task.updated) = (at(0), at(0));
    assert_eq!(task.status_since, Some(at(0)));
    let mut run = run_value("T-001", "R0001", 10, 1, false);
    run["review_runs"] = json!([
        review_child("T-001", "R0001", 1, nested, true, "approve"),
        review_child("T-001", "R0001", 2, 1, false, "approve"),
    ]);
    task.runs = vec![serde_json::from_value(run).unwrap()];
    let mut plan = plan("T-001", "P0001", 3);
    plan["reviews"][0]["inbox"] = inbox(nested, nested);
    task.plans = vec![serde_json::from_value(plan).unwrap()];
    store.fixture_put_task(&task).unwrap();
    store.get(&task.id).unwrap()
}

fn scalar_edits(store: &Store) -> (WorkSnapshot, WorkSnapshot) {
    let (child, child_work) = measure(|| {
        store.update_head("T-001", |h| {
            TaskMut::edit_review_child(h, "R0001", "R0001-review0001", |c| {
                c.summary = Some("edited".into());
                Ok(())
            })
        })
    });
    child.unwrap();
    let (review, review_work) = measure(|| {
        store.update_head("T-001", |h| {
            TaskMut::edit_plan_review(h, "P0001", "P0001-PR0001", |r| {
                r.notes = Some("edited".into());
                Ok(())
            })
        })
    });
    review.unwrap();
    (child_work, review_work)
}

#[test]
fn scalar_child_edits_cost_the_same_as_nested_history_grows() {
    let mut costs = std::collections::BTreeMap::new();
    for nested in [1, 10] {
        for (backend, _dir, store) in backends() {
            let mut expected = nested_task(&store, nested);
            let (child, review) = scalar_edits(&store);
            TaskMut::edit_review_child(&mut expected, "R0001", "R0001-review0001", |c| {
                c.summary = Some("edited".into());
                Ok(())
            })
            .unwrap();
            TaskMut::edit_plan_review(&mut expected, "P0001", "P0001-PR0001", |r| {
                r.notes = Some("edited".into());
                Ok(())
            })
            .unwrap();
            let stored = store.get("T-001").unwrap();
            expected.updated = stored.updated;
            assert_eq!(stored, expected, "{backend} nested {nested}");
            costs.insert((backend, nested), (child, review));
        }
    }
    for backend in ["sqlite", "fake"] {
        let (small, large) = (&costs[&(backend, 1)], &costs[&(backend, 10)]);
        for (what, small, large) in [
            ("child", &small.0, &large.0),
            ("plan review", &small.1, &large.1),
        ] {
            for counter in [Counter::NestedRowDecodes, Counter::HistoryRowDecodes] {
                assert_eq!(
                    small.get(counter),
                    large.get(counter),
                    "{backend} {what} {counter:?}"
                );
            }
            assert_eq!(
                small.unattributed_byte_remainder(),
                large.unattributed_byte_remainder(),
                "{backend} {what} bytes"
            );
            assert_eq!(small.full_task_hydrations(), 0);
        }
    }
}

/// What a view records for each page of task headers it returns.
const TASKS_PAGE: &str = "<tasks page>";

/// A fake backend whose views record the key of every task row they return, and each page of
/// task headers.
struct Counting {
    inner: FakeBackend,
    returned: Arc<Mutex<Vec<(TaskPart, String)>>>,
}

struct CountingView {
    inner: Box<dyn ReadView + Send>,
    returned: Arc<Mutex<Vec<(TaskPart, String)>>>,
}

impl CountingView {
    fn record(&self, rows: &[HistoryRow]) {
        let mut returned = self.returned.lock().unwrap();
        returned.extend(rows.iter().map(|r| (r.part, r.key.clone())));
    }
}

impl StoreBackend for Counting {
    fn view(&self) -> Result<Box<dyn ReadView + Send>, ViewError> {
        Ok(Box::new(CountingView {
            inner: self.inner.view()?,
            returned: Arc::clone(&self.returned),
        }))
    }
    fn view_with_max_age(
        &self,
        max_age: std::time::Duration,
    ) -> Result<Box<dyn ReadView + Send>, ViewError> {
        Ok(Box::new(CountingView {
            inner: self.inner.view_with_max_age(max_age)?,
            returned: Arc::clone(&self.returned),
        }))
    }
    fn apply(&self, op: Operation) -> Result<Committed, ApplyError> {
        self.inner.apply(op)
    }
    fn check(&self) -> anyhow::Result<IntegrityReport> {
        self.inner.check()
    }
    fn backup_database(&self, dest: &Path) -> anyhow::Result<()> {
        self.inner.backup_database(dest)
    }
}

impl ReadView for CountingView {
    fn seq(&self) -> ChangeSeq {
        self.inner.seq()
    }
    fn ensure_fresh(&self) -> Result<(), ViewError> {
        self.inner.ensure_fresh()
    }
    fn board(&self, since: Option<ChangeSeq>) -> Result<BoardDelta, FeedError> {
        self.inner.board(since)
    }
    fn changes(&self, after: ChangeSeq, limit: u32) -> Result<ChangePage, FeedError> {
        self.inner.changes(after, limit)
    }
    fn head(&self, num: TaskNum) -> Result<Option<TaskHeadDoc>, ViewError> {
        self.inner.head(num)
    }
    fn heads(&self, q: &HeadQuery, page: PageReq) -> Result<Page<TaskHeadDoc>, ViewError> {
        self.inner.heads(q, page)
    }
    fn task(&self, num: TaskNum, parts: &TaskParts) -> Result<Option<TaskDoc>, ViewError> {
        let doc = self.inner.task(num, parts)?;
        if let Some(doc) = &doc {
            self.record(&doc.rows);
        }
        Ok(doc)
    }
    fn rows(
        &self,
        num: TaskNum,
        q: &RowQuery,
        page: PageReq,
    ) -> Result<Page<HistoryRow>, ViewError> {
        let page = self.inner.rows(num, q, page)?;
        self.record(&page.items);
        Ok(page)
    }
    fn rows_except(
        &self,
        num: TaskNum,
        q: &RowQuery,
        except: &str,
        page: PageReq,
    ) -> Result<Page<HistoryRow>, ViewError> {
        let page = self.inner.rows_except(num, q, except, page)?;
        self.record(&page.items);
        Ok(page)
    }
    fn token_owners(
        &self,
        part: TaskPart,
        token: &str,
        limit: u32,
    ) -> Result<Vec<TaskNum>, ViewError> {
        self.inner.token_owners(part, token, limit)
    }
    fn lesson_artifacts(&self, num: TaskNum, limit: u32) -> Result<Vec<HistoryRow>, ViewError> {
        self.inner.lesson_artifacts(num, limit)
    }
    fn lesson_approvals(&self, sha: &str) -> Result<Vec<LessonApproval>, ViewError> {
        self.inner.lesson_approvals(sha)
    }
    fn tasks(&self, q: &TaskQuery, page: PageReq) -> Result<Page<TaskHeader>, ViewError> {
        self.returned
            .lock()
            .unwrap()
            .push((TaskPart::Comment, TASKS_PAGE.into()));
        self.inner.tasks(q, page)
    }
    fn run_worktrees(&self, current_only: bool) -> Result<Vec<RunWorktree>, ViewError> {
        self.inner.run_worktrees(current_only)
    }
    fn last_task_num(&self) -> Result<Option<TaskNum>, ViewError> {
        self.inner.last_task_num()
    }
    fn events(&self, q: &EventQuery, page: PageReq) -> Result<Page<StoredEvent>, ViewError> {
        self.inner.events(q, page)
    }
    fn conditions(
        &self,
        q: &ConditionQuery,
        page: PageReq,
    ) -> Result<Page<ConditionRun>, ViewError> {
        self.inner.conditions(q, page)
    }
    fn records(&self, q: &RecordQuery, page: PageReq) -> Result<Page<StoredRecord>, ViewError> {
        self.inner.records(q, page)
    }
    fn aggregates(&self, names: &[AggregateName]) -> Result<Aggregates, ViewError> {
        self.inner.aggregates(names)
    }
    fn blobs(&self, q: &BlobQuery, page: PageReq) -> Result<Page<BlobRow>, ViewError> {
        self.inner.blobs(q, page)
    }
    fn storage_rows(&self, q: &StorageQuery, page: PageReq) -> Result<Page<StorageRow>, ViewError> {
        self.inner.storage_rows(q, page)
    }
}

fn fake() -> FakeBackend {
    FakeBackend::new(
        BackendConfig::default(),
        Arc::new(ManualClock::new(jiff::Timestamp::now())),
        Arc::new(FaultInjector::default()),
    )
}

/// A task whose every review child and plan review carries a finding.
fn findings_task(store: &Store) -> Task {
    let mut task = store
        .create("Findings", Some("app"), Status::Running, "Spec\n")
        .unwrap();
    grow(&mut task, 2, 3, 1, 1);
    for run in &mut task.runs {
        for child in &mut run.review_runs {
            let review = child.review.as_mut().unwrap();
            review.verdict = Some(Verdict::Changes);
            review.notes = Some(format!("{} {}", child.id, "x".repeat(400)));
        }
    }
    for review in &mut task.plans.last_mut().unwrap().reviews {
        review.verdict = Some(Verdict::Changes);
        review.notes = Some(format!("{} notes", review.id));
    }
    store.fixture_put_task(&task).unwrap();
    store.get(&task.id).unwrap()
}

#[test]
fn an_excluded_finding_row_is_never_returned_by_the_backend() {
    let (_dir, base) = store_support::store();
    let returned = Arc::new(Mutex::new(Vec::new()));
    let store = base.with_backend(Box::new(Counting {
        inner: fake(),
        returned: Arc::clone(&returned),
    }));
    let full = findings_task(&store);
    let current = full.runs.last().unwrap();
    let excludes = [
        (
            FindingSource::Review,
            current.review_runs.last().unwrap().id.clone(),
        ),
        (
            FindingSource::Review,
            full.runs[0].review_runs[0].id.clone(),
        ),
        (
            FindingSource::Plan,
            full.plans[0].reviews.last().unwrap().id.clone(),
        ),
        (FindingSource::Plan, full.plans[0].reviews[0].id.clone()),
    ];
    for (source, exclude) in excludes {
        let head = store.head(&full.id).unwrap();
        let excluded_key = corral_core::task_findings(&full)
            .into_iter()
            .find(|(_, f)| f.source == source && f.attempt == exclude)
            .map(|(key, _)| key)
            .unwrap();
        returned.lock().unwrap().clear();
        let (found, delta) = measure(|| head.emitted_findings(source, Some(&exclude)).unwrap());
        assert_eq!(
            found,
            full.emitted_findings(source, Some(&exclude)).unwrap(),
            "{source:?} {exclude}"
        );
        let reads = returned.lock().unwrap().clone();
        let finding_reads: Vec<&String> = reads
            .iter()
            .filter(|(part, _)| *part == TaskPart::Finding)
            .map(|(_, key)| key)
            .collect();
        assert!(
            !finding_reads.contains(&&excluded_key),
            "{source:?}: the excluded finding {excluded_key} was read"
        );
        assert_eq!(finding_reads.len(), found.len(), "{source:?} {exclude}");
        assert_eq!(delta.emitted_rows(), found.len() as u64);
        let bytes: u64 = corral_core::task_findings(&full)
            .into_iter()
            .filter(|(_, f)| f.source == source && f.attempt != exclude)
            .map(|(_, f)| serde_json::to_vec(&f).unwrap().len() as u64)
            .sum();
        assert_eq!(delta.emitted_doc_bytes(), bytes, "{source:?} {exclude}");
    }
}

#[test]
fn a_mutable_head_never_reads_the_excluded_finding_it_also_rewrote() {
    let (_dir, base) = store_support::store();
    let returned = Arc::new(Mutex::new(Vec::new()));
    let store = base.with_backend(Box::new(Counting {
        inner: fake(),
        returned: Arc::clone(&returned),
    }));
    let full = findings_task(&store);
    let run = full.runs.last().unwrap().id.clone();
    let current = full
        .runs
        .last()
        .unwrap()
        .review_runs
        .last()
        .unwrap()
        .id
        .clone();
    let excluded_key = corral_core::task_findings(&full)
        .into_iter()
        .find(|(_, f)| f.source == FindingSource::Review && f.attempt == current)
        .map(|(key, _)| key)
        .unwrap();
    let mut expected = full.clone();
    TaskMut::edit_review_child(&mut expected, &run, &current, |c| {
        c.review.as_mut().unwrap().notes = Some("rewritten".into());
        Ok(())
    })
    .unwrap();
    let want = expected
        .emitted_findings(FindingSource::Review, Some(&current))
        .unwrap();
    store
        .update_head(&full.id, |head| {
            TaskMut::edit_review_child(head, &run, &current, |c| {
                c.review.as_mut().unwrap().notes = Some("rewritten".into());
                Ok(())
            })?;
            returned.lock().unwrap().clear();
            let (found, delta) = measure(|| {
                TaskRef::emitted_findings(head, FindingSource::Review, Some(&current)).unwrap()
            });
            assert_eq!(found, want);
            let reads = returned.lock().unwrap().clone();
            let finding_reads: Vec<&String> = reads
                .iter()
                .filter(|(part, _)| *part == TaskPart::Finding)
                .map(|(_, key)| key)
                .collect();
            assert!(
                !finding_reads.contains(&&excluded_key),
                "{excluded_key} was read"
            );
            assert_eq!(finding_reads.len(), found.len());
            assert_eq!(delta.emitted_rows(), found.len() as u64);
            Ok(())
        })
        .unwrap();
}

fn wait_record(run: &str, n: usize) -> corral_core::WaitRecord {
    serde_json::from_value(wait(run, n, false)).unwrap()
}

/// Mutations of one operation, each followed by the keyed reads that must observe it.
fn operation<M: TaskMut>(task: &mut M) -> Result<Vec<String>> {
    let mut seen = Vec::new();
    let current = task.current_run_head()?.unwrap().id.clone();
    let first_child = task
        .current_run_head()?
        .unwrap()
        .into_owned()
        .current_child
        .unwrap()
        .id;
    let plan = task.last_plan_head()?.unwrap().id.clone();
    let old_plan_run = "P0001";
    task.edit_run("R0001", |run| {
        run.summary = Some("history edit".into());
        Ok(())
    })?;
    seen.push(format!("{:?}", task.run_head("R0001")?.map(norm_run)));
    let mut child: Run =
        serde_json::from_value(review_child("T-001", &current, 900, 1, true, "changes"))?;
    child.review.as_mut().unwrap().notes = Some("pushed finding".into());
    let pushed = child.id.clone();
    task.push_review_child(&current, child)?;
    seen.push(format!(
        "{:?}",
        task.review_child(&current, &pushed)?.map(norm_run)
    ));
    seen.push(format!(
        "{:?}",
        task.review_child(&current, &first_child)?.map(norm_run)
    ));
    let ids: Vec<String> = task
        .open_review_children(&current)?
        .into_iter()
        .map(|c| c.id)
        .collect();
    seen.push(format!("{ids:?}"));
    task.edit_review_child(&current, &first_child, |c| {
        c.summary = Some("child edit".into());
        Ok(())
    })?;
    seen.push(format!(
        "{:?}",
        task.review_child(&current, &first_child)?.map(norm_run)
    ));
    seen.push(format!(
        "{:?}",
        task.emitted_findings(FindingSource::Review, None)?
    ));
    task.push_wait(&current, wait_record(&current, 9900))?;
    seen.push(format!("{:?}", task.wait(&current, "W9900")?));
    seen.push(format!("{:?}", task.open_obligations(&current)?));
    let to = Recipient::Run {
        task: "T-001".into(),
        run: current.clone(),
    };
    task.mark_effect_applied(&to, 77)?;
    seen.push(format!("{:?}", task.effect_applied(&to, 77)?));
    let binding = SessionBinding {
        session: "after".into(),
        endpoint: None,
        transcript: None,
    };
    task.set_observe_cursor(
        &to,
        SessionCursor {
            adapter: "pi".into(),
            binding: binding.clone(),
            cursor: ObserveCursor::default(),
        },
    )?;
    seen.push(format!("{:?}", task.observe_cursor(&to, "pi", &binding)?));
    let review = serde_json::from_value(plan_review(
        &format!("{plan}-PR0900"),
        "rev",
        Some("changes"),
    ))?;
    task.push_plan_review(&plan, review)?;
    seen.push(format!(
        "{:?}",
        task.plan_review(&plan, &format!("{plan}-PR0900"))?
    ));
    let pending: Vec<String> = task
        .pending_plan_reviews(&plan)?
        .into_iter()
        .map(|r| r.id)
        .collect();
    seen.push(format!("{pending:?}"));
    seen.push(format!(
        "{:?}",
        task.emitted_findings(FindingSource::Plan, None)?
    ));
    let record = serde_json::from_value(checkpoint("T-001", &current, 77, 1))?;
    task.push_checkpoint(&current, record)?;
    let metas: Vec<_> = task
        .checkpoint_metas_newest_first(&current)?
        .collect::<Result<_>>()?;
    seen.push(format!("{:?}", metas.first()));
    seen.push(format!(
        "{:?}",
        task.checkpoint_payload(&current, metas.len() as u64)?
    ));
    task.add_comment(serde_json::from_value(
        json!({"author": "owner", "at": at(5), "text": "late comment"}),
    )?)?;
    seen.push(format!("{:?}", task.emitted_comments()?));
    task.edit_plan_run(old_plan_run, |run| {
        run.rounds = 7;
        Ok(())
    })?;
    seen.push(format!(
        "{:?}",
        task.plan_run_head(old_plan_run)?.map(norm_plan_run)
    ));
    let next = coding_run("T-001", "R0900", 900, 2, 1, false);
    task.start_run(next)?;
    seen.push(format!("{:?}", task.previous_run_head()?.map(norm_run)));
    seen.push(format!("{:?}", task.run_head(&current)?.map(norm_run)));
    seen.push(format!(
        "{:?}",
        task.current_run_head()?.map(|r| r.id.clone())
    ));
    seen.push(format!(
        "{:?}",
        task.emitted_findings(FindingSource::Review, Some(&pushed))?
    ));
    let container = corral_core::agent::ContainerRef::Worker {
        task: "T-001".into(),
        run: current.clone(),
    };
    seen.push(format!("{:?}", task.agent_turn(&container, 1, 1)?));
    Ok(seen)
}

#[test]
fn a_mutable_head_reads_its_own_queued_edits_as_the_full_task_does() {
    for (backend, _dir, store) in backends() {
        let mut full = store
            .create("Reads", Some("app"), Status::Running, "Spec\n")
            .unwrap();
        grow(&mut full, 3, 3, 2, 1);
        store.fixture_put_task(&full).unwrap();
        let mut full = store.get(&full.id).unwrap();
        let expected = operation(&mut full).unwrap();
        let mut seen = None;
        store
            .update_head("T-001", |h| {
                seen = Some(operation(h)?);
                Ok(())
            })
            .unwrap();
        let seen = seen.unwrap();
        for (i, (want, got)) in expected.iter().zip(&seen).enumerate() {
            assert_eq!(want, got, "{backend} read {i}");
        }
        assert_eq!(expected.len(), seen.len());
        let stored = store.get("T-001").unwrap();
        full.updated = stored.updated;
        assert_eq!(stored, full, "{backend}: the committed task");
    }
}

#[test]
fn task_number_allocation_reads_no_header_page() {
    for n in [1_u64, 10] {
        // Given boards of n terminal tasks, one behind a view recording every header page.
        let (_dir, base) = store_support::store();
        let returned = Arc::new(Mutex::new(Vec::new()));
        let store = base.with_backend(Box::new(Counting {
            inner: fake(),
            returned: Arc::clone(&returned),
        }));
        for i in 0..n {
            base.create(&format!("Done {i}"), None, Status::Done, "")
                .unwrap();
            store
                .create(&format!("Done {i}"), None, Status::Done, "")
                .unwrap();
        }
        returned.lock().unwrap().clear();
        // When the next task number and the numbers either side of it are read.
        for store in [&base, &store] {
            let next = store.next_task_number().unwrap();
            // Then they are exact on both backends...
            assert_eq!(next, n + 1, "x{n}");
            assert!(store.task_number_taken(n).unwrap(), "x{n}");
            assert!(!store.task_number_taken(next).unwrap(), "x{n}");
        }
        // ...and the recorded view returned no page of task headers.
        let pages = returned
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, key)| key == TASKS_PAGE)
            .count();
        assert_eq!(pages, 0, "x{n}");
    }
}
