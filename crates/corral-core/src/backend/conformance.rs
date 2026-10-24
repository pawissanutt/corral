//! The generic conformance suite every backend runs (plan T-253 section 1, requirement 5).
//!
//! A backend provides a [`Fixture`] factory; [`cases`] lists every case with the operations it
//! covers, and [`suite`] runs them all. The SQLite backend reuses the same cases unchanged.

mod cases_blobs;
mod cases_ops;
mod cases_storage;
mod cases_tasks;
mod cases_views;
mod samples;

use std::sync::Arc;

use anyhow::{Result, bail};
use jiff::Timestamp;

use super::fake::{FakeBackend, FakeBlobStore};
use super::*;

/// A fresh, empty store with two handles on it, a blob store, the injected clock and fault
/// points the backend uses, and a way to prune the change feed as retention would.
pub struct Fixture {
    pub backend: Box<dyn StoreBackend>,
    pub second: Box<dyn StoreBackend>,
    pub blobs: Box<dyn BlobStore>,
    pub clock: ManualClock,
    pub faults: Arc<FaultInjector>,
    pub config: BackendConfig,
    pub prune_changes_through: PruneChanges,
    /// Replaces stored blob content in place, as on-disk corruption would.
    pub corrupt_blob: CorruptBlob,
}

pub type MakeFixture<'a> = &'a dyn Fn() -> Fixture;
pub type PruneChanges = Box<dyn Fn(ChangeSeq) -> Result<()>>;
pub type CorruptBlob = Box<dyn Fn(&BlobHash, Vec<u8>) -> Result<()>>;

/// One conformance case and the operations whose contract it checks.
pub struct Case {
    pub name: &'static str,
    pub covers: &'static [OpKind],
    pub run: fn(MakeFixture<'_>) -> Result<()>,
}

pub fn epoch() -> Timestamp {
    Timestamp::from_second(1_800_000_000).unwrap_or(Timestamp::UNIX_EPOCH)
}

/// A [`Fixture`] over [`FakeBackend`] and [`FakeBlobStore`].
pub fn fake_fixture() -> Fixture {
    let clock = ManualClock::new(epoch());
    let faults = Arc::new(FaultInjector::default());
    let config = BackendConfig::default();
    let backend = FakeBackend::new(config, Arc::new(clock.clone()), Arc::clone(&faults));
    let blobs = Arc::new(FakeBlobStore::new(Arc::new(clock.clone())));
    let pruner = backend.handle();
    let corrupter = Arc::clone(&blobs);
    Fixture {
        second: Box::new(backend.handle()),
        backend: Box::new(backend),
        blobs: Box::new(SharedBlobs(blobs)),
        clock,
        faults,
        config,
        prune_changes_through: Box::new(move |seq| {
            pruner.prune_changes_through(seq);
            Ok(())
        }),
        corrupt_blob: Box::new(move |hash, bytes| {
            corrupter.corrupt(hash, bytes);
            Ok(())
        }),
    }
}

struct SharedBlobs(Arc<FakeBlobStore>);

impl BlobStore for SharedBlobs {
    fn stage(&self, src: &mut dyn std::io::Read) -> Result<Staged> {
        self.0.stage(src)
    }
    fn put_staged(&self, staged: Staged) -> Result<()> {
        self.0.put_staged(staged)
    }
    fn get(&self, hash: &BlobHash) -> Result<VerifiedReader> {
        self.0.get(hash)
    }
    fn delete(&self, hash: &BlobHash) -> Result<()> {
        self.0.delete(hash)
    }
    fn list(&self, after: Option<&BlobHash>, limit: u32) -> Result<Vec<BlobFile>> {
        self.0.list(after, limit)
    }
}

pub fn cases() -> Vec<Case> {
    let mut all = cases_ops::cases();
    all.extend(cases_tasks::cases());
    all.extend(cases_blobs::cases());
    all.extend(cases_storage::cases());
    all.extend(cases_views::cases());
    all
}

/// Runs one named case against fixtures from `make`.
pub fn run_case(name: &str, make: MakeFixture<'_>) -> Result<()> {
    let Some(case) = cases().into_iter().find(|case| case.name == name) else {
        bail!("no conformance case named {name}");
    };
    (case.run)(make).map_err(|error| error.context(format!("conformance case {name}")))
}

/// Runs every case, reporting all failures together.
pub fn suite(make: MakeFixture<'_>) -> Result<()> {
    let failures: Vec<String> = cases()
        .into_iter()
        .filter_map(|case| {
            (case.run)(make)
                .err()
                .map(|error| format!("{}: {error:#}", case.name))
        })
        .collect();
    if failures.is_empty() {
        Ok(())
    } else {
        bail!(
            "{} conformance case(s) failed:\n{}",
            failures.len(),
            failures.join("\n")
        )
    }
}

/// Everything a view returns through every [`ReadView`] method, for whole-state comparison.
#[derive(Debug, PartialEq)]
pub struct ViewDump {
    pub seq: ChangeSeq,
    pub board: BoardDelta,
    pub changes: Vec<ChangeEntry>,
    pub open_heads: Vec<TaskHeadDoc>,
    pub heads_by_id: Vec<TaskHeadDoc>,
    pub children: Vec<Vec<TaskHeadDoc>>,
    pub point_heads: Vec<Option<TaskHeadDoc>>,
    pub headers: Vec<TaskHeader>,
    pub headers_by_update: Vec<TaskHeader>,
    pub tasks: Vec<Option<TaskDoc>>,
    pub events: Vec<StoredEvent>,
    pub conditions: Vec<ConditionRun>,
    pub records: Vec<StoredRecord>,
    pub aggregates: Aggregates,
    pub blobs: Vec<BlobRow>,
    pub storage: Vec<Vec<StorageRow>>,
}

const PAGE: u32 = 7;

pub fn all_pages<T>(
    mut read: impl FnMut(PageReq) -> Result<Page<T>, ViewError>,
) -> Result<Vec<T>, ViewError> {
    let mut items = Vec::new();
    let mut req = PageReq::first(PAGE);
    loop {
        let page = read(req)?;
        items.extend(page.items);
        match page.next {
            Some(next) => {
                req = PageReq {
                    after: Some(next),
                    limit: PAGE,
                }
            }
            None => return Ok(items),
        }
    }
}

fn feed(view: &dyn ReadView) -> Result<Vec<ChangeEntry>> {
    let mut after = ChangeSeq(0);
    let mut entries = Vec::new();
    loop {
        match view.changes(after, PAGE) {
            Ok(page) => {
                after = page.entries.last().map_or(after, |entry| entry.seq);
                entries.extend(page.entries);
                if !page.more {
                    return Ok(entries);
                }
            }
            Err(FeedError::Gap { oldest }) if entries.is_empty() => after = ChangeSeq(oldest.0 - 1),
            Err(error) => return Err(error.into()),
        }
    }
}

pub fn dump(view: &dyn ReadView) -> Result<ViewDump> {
    let headers = all_pages(|page| {
        let q = TaskQuery {
            statuses: Vec::new(),
            project: None,
            order: TaskOrder::IdAsc,
        };
        view.tasks(&q, page)
    })?;
    let ids: Vec<String> = headers.iter().map(|h| h.id.clone()).collect();
    let mut children = Vec::new();
    for id in &ids {
        children.push(all_pages(|page| {
            view.heads(&HeadQuery::Children { parent: id.clone() }, page)
        })?);
    }
    let mut point_heads = Vec::new();
    let mut tasks = Vec::new();
    for header in &headers {
        point_heads.push(view.head(header.num)?);
        tasks.push(view.task(header.num, &TaskParts::All)?);
    }
    let storage_queries = [
        StorageQuery::Scratch { states: Vec::new() },
        StorageQuery::Lifecycle { task: None },
        StorageQuery::Leases,
        StorageQuery::Claims,
        StorageQuery::Authorizations { task: None },
    ];
    let mut storage = Vec::new();
    for q in &storage_queries {
        storage.push(all_pages(|page| view.storage_rows(q, page))?);
    }
    Ok(ViewDump {
        seq: view.seq(),
        board: view.board(None)?,
        changes: feed(view)?,
        open_heads: all_pages(|page| view.heads(&HeadQuery::Open { project: None }, page))?,
        heads_by_id: all_pages(|page| view.heads(&HeadQuery::Ids(ids.clone()), page))?,
        children,
        point_heads,
        headers_by_update: all_pages(|page| {
            let q = TaskQuery {
                statuses: Vec::new(),
                project: None,
                order: TaskOrder::UpdatedDesc,
            };
            view.tasks(&q, page)
        })?,
        headers,
        tasks,
        events: all_pages(|page| view.events(&EventQuery::default(), page))?,
        conditions: all_pages(|page| view.conditions(&ConditionQuery::default(), page))?,
        records: all_pages(|page| view.records(&RecordQuery::default(), page))?,
        aggregates: view.aggregates(&AggregateName::ALL)?,
        blobs: all_pages(|page| view.blobs(&BlobQuery::default(), page))?,
        storage,
    })
}

pub fn snapshot(backend: &dyn StoreBackend) -> Result<ViewDump> {
    dump(backend.view()?.as_ref())
}

fn ensure_integrity(fx: &Fixture) -> Result<()> {
    let report = fx.backend.check()?;
    if !report.is_ok() {
        bail!("integrity check failed: {:?}", report.problems);
    }
    Ok(())
}

fn expect_conflict<T: std::fmt::Debug>(
    result: Result<T, ApplyError>,
    what: &str,
) -> Result<Conflict> {
    match result {
        Err(ApplyError::Conflict(conflict)) => Ok(conflict),
        other => bail!("{what}: expected a conflict, got {other:?}"),
    }
}

fn expect_precondition<T: std::fmt::Debug>(
    result: Result<T, ApplyError>,
    what: &str,
) -> Result<String> {
    match result {
        Err(ApplyError::Precondition(message)) => Ok(message),
        other => bail!("{what}: expected a failed precondition, got {other:?}"),
    }
}

/// Applies `op`, which must leave the store unchanged and fail as `check` requires.
fn refused(
    fx: &Fixture,
    op: Operation,
    what: &str,
    check: fn(Result<Committed, ApplyError>, &str) -> Result<()>,
) -> Result<()> {
    let before = snapshot(fx.backend.as_ref())?;
    check(fx.backend.apply(op), what)?;
    if snapshot(fx.backend.as_ref())? != before {
        bail!("{what}: a refused operation changed the store");
    }
    Ok(())
}

fn conflict(result: Result<Committed, ApplyError>, what: &str) -> Result<()> {
    expect_conflict(result, what).map(drop)
}

fn precondition(result: Result<Committed, ApplyError>, what: &str) -> Result<()> {
    expect_precondition(result, what).map(drop)
}
