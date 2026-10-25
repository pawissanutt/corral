//! The GC barrier suite of plan T-253 section 7 on FakeBackend + FsBlobStore and on
//! SqliteBackend + FsBlobStore, plus a GC process killed between unlink and `FinishDelete`.
//! Races are staged with GC barrier points, injected clocks and real child processes; nothing
//! waits on time.

mod backend {
    pub use corral_core::backend::*;
}
mod blob_ref {
    pub use corral_core::blob_ref::*;
}
mod sqlite_support;

use std::cell::RefCell;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail, ensure};
use corral_core::backend::fake::FakeBackend;
use corral_core::backend::*;
use corral_core::blob::{
    BLOB_GRACE, FsBlobStore, Gc, GcPoint, GcReport, GcRun, ProcessIdentity, PutBlob, Putter,
    TryPut, proven_dead,
};
use corral_core::blob_ref::{BlobHash, BlobRef};
use corral_core::sqlite::SqliteBackend;
use jiff::{SignedDuration, Timestamp};
use serde_json::{Value, json};
use sqlite_support::{ChildLines, child_command, child_role, hear, options, record, tell};
use tempfile::TempDir;

const ENTRY: &str = "zz_child_process_role";
const CLOCK_ENV: &str = "CORRAL_BLOB_TEST_CLOCK";
const HOUR: Duration = Duration::from_secs(3600);
const SECOND: Duration = Duration::from_secs(1);

/// The entry child processes run; a no-op in a normal test run.
#[test]
fn zz_child_process_role() -> Result<()> {
    let Some((role, corral)) = child_role() else {
        return Ok(());
    };
    match role.as_str() {
        "linger" => {
            tell("lingering")?;
            hear().map(drop)
        }
        "gc-crash" => {
            let at: Timestamp = std::env::var(CLOCK_ENV).context("clock")?.parse()?;
            let env = Env::sqlite_at(&corral, ManualClock::new(at), None)?;
            let barrier = |point: GcPoint<'_>| -> Result<()> {
                if let GcPoint::Unlinked(hash) = point {
                    tell(&format!("unlinked {hash}"))?;
                    hear()?;
                }
                Ok(())
            };
            env.gc()?.barrier(&barrier).run().map(drop)
        }
        other => bail!("unknown child role {other}"),
    }
}

struct Env {
    backend: Box<dyn StoreBackend>,
    blobs: FsBlobStore,
    clock: ManualClock,
    corral: PathBuf,
    _dir: Option<TempDir>,
}

impl Env {
    fn fake() -> Result<Self> {
        let dir = TempDir::new()?;
        let corral = dir.path().join(".corral");
        std::fs::create_dir(&corral)?;
        let clock = ManualClock::new(Timestamp::now());
        let backend = FakeBackend::new(
            BackendConfig::default(),
            Arc::new(clock.clone()),
            Arc::new(FaultInjector::default()),
        );
        Ok(Self {
            backend: Box::new(backend),
            blobs: FsBlobStore::open(&corral)?,
            clock,
            corral,
            _dir: Some(dir),
        })
    }

    fn sqlite() -> Result<Self> {
        let dir = TempDir::new()?;
        let corral = dir.path().join(".corral");
        Self::sqlite_at(&corral, ManualClock::new(Timestamp::now()), Some(dir))
    }

    fn sqlite_at(corral: &Path, clock: ManualClock, dir: Option<TempDir>) -> Result<Self> {
        let faults = Arc::new(FaultInjector::default());
        Ok(Self {
            backend: Box::new(SqliteBackend::open(corral, options(&clock, &faults, None))?),
            blobs: FsBlobStore::open(corral)?,
            clock,
            corral: corral.to_owned(),
            _dir: dir,
        })
    }

    fn putter(&self) -> Result<Putter<'_>> {
        Putter::new(self.backend.as_ref(), &self.blobs, &self.clock)
    }

    fn gc(&self) -> Result<Gc<'_>> {
        Gc::new(self.backend.as_ref(), &self.blobs, &self.clock)
    }

    fn collect(&self) -> Result<GcReport> {
        match self.gc()?.run()? {
            GcRun::Done(report) => Ok(report),
            GcRun::ClaimHeld { owner } => bail!("the storage-gc claim is held by {owner}"),
        }
    }

    fn apply(&self, op: Operation) -> Result<Committed> {
        let kind = op.kind();
        self.backend
            .apply(op)
            .map_err(|error| anyhow!("{kind:?}: {error}"))
    }

    fn put(&self, bytes: &[u8]) -> Result<PutBlob> {
        self.putter()?
            .put(&mut &bytes[..], &mut |_| bail!("unexpectedly busy"))
    }

    fn try_put(&self, bytes: &[u8]) -> Result<TryPut> {
        let staged = self.blobs.stage(&mut &bytes[..])?;
        self.putter()?.try_put(staged)
    }

    fn reference(&self, key: &str, blob: &PutBlob) -> Result<()> {
        let doc = json!({"log": blob.blob.to_string()});
        let write = record("refs", key, None, Some(doc), self.clock.now());
        self.apply(Operation::PutRecords(PutRecords {
            writes: vec![write],
            events: Vec::new(),
            reservations: vec![blob.reservation.clone()],
        }))
        .map(drop)
    }

    fn unreference(&self, key: &str) -> Result<()> {
        let write = record("refs", key, Some(1), None, self.clock.now());
        self.apply(Operation::PutRecords(PutRecords {
            writes: vec![write],
            events: Vec::new(),
            reservations: Vec::new(),
        }))
        .map(drop)
    }

    /// A live blob whose last reference was removed now.
    fn orphan(&self, bytes: &[u8]) -> Result<BlobHash> {
        let blob = self.put(bytes)?;
        let key = blob.blob.hash().to_string();
        self.reference(&key, &blob)?;
        self.unreference(&key)?;
        Ok(blob.blob.hash().clone())
    }

    fn row(&self, hash: &BlobHash) -> Result<Option<BlobRow>> {
        let q = BlobQuery {
            hash: Some(hash.clone()),
            state: None,
        };
        let view = self.backend.view()?;
        Ok(view.blobs(&q, PageReq::first(1))?.items.into_iter().next())
    }

    fn claims(&self) -> Result<Vec<ClaimRow>> {
        let view = self.backend.view()?;
        let page = view.storage_rows(&StorageQuery::Claims, PageReq::first(100))?;
        Ok(page
            .items
            .into_iter()
            .filter_map(|row| match row {
                StorageRow::Claim(claim) => Some(claim),
                _ => None,
            })
            .collect())
    }

    fn has_file(&self, hash: &BlobHash) -> bool {
        self.blobs.path_of(hash).is_file()
    }

    fn read(&self, hash: &BlobHash) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        self.blobs.get(hash)?.read_to_end(&mut out)?;
        Ok(out)
    }

    fn ago(&self, by: Duration) -> Result<Timestamp> {
        Ok(self
            .clock
            .now()
            .checked_sub(SignedDuration::try_from(by)?)?)
    }

    /// Persists what a GC process `holder` leaves when it stops after `BeginDelete` of `hash`.
    fn hold_and_begin(&self, holder: &ProcessIdentity, hash: &BlobHash) -> Result<()> {
        self.apply(Operation::Claim(Claim {
            name: STORAGE_GC_CLAIM.to_owned(),
            owner: holder.owner(),
            acquired: self.clock.now(),
            doc: serde_json::to_value(holder)?,
            reservations: Vec::new(),
        }))?;
        self.apply(Operation::Blob(BlobOp::BeginDelete {
            hash: hash.clone(),
            gc_owner: holder.owner(),
            horizon: self.ago(BLOB_GRACE)?,
        }))
        .map(drop)
    }

    fn ensure_live_with_file(&self, hash: &BlobHash, bytes: &[u8], refs: u64) -> Result<()> {
        let row = self.row(hash)?.context("blob row")?;
        ensure!(
            row.state == BlobState::Live && row.refs == refs,
            "row {row:?}"
        );
        ensure!(self.read(hash)? == bytes, "content of {hash}");
        Ok(())
    }
}

fn spawn(role: &str, corral: &Path, env: &[(&str, String)]) -> Result<(Child, ChildLines)> {
    let mut command = child_command(ENTRY, role, corral)?;
    command
        .envs(env.iter().map(|(k, v)| (*k, v.as_str())))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    let mut child = command.spawn().context("spawn a GC test child")?;
    let lines = ChildLines::new(child.stdout.take().context("child stdout")?);
    Ok((child, lines))
}

/// A live child process that exits when its stdin closes. Its output reader lives until the
/// child is reaped, so the child never writes into a closed pipe.
struct Lingering {
    child: Child,
    _lines: ChildLines,
    identity: ProcessIdentity,
}

impl Lingering {
    fn start(corral: &Path) -> Result<Self> {
        let (child, lines) = spawn("linger", corral, &[])?;
        lines.wait_for("lingering")?;
        let identity = ProcessIdentity::of(child.id())?.context("the lingering child is alive")?;
        Ok(Self {
            child,
            _lines: lines,
            identity,
        })
    }

    fn end(mut self) -> Result<()> {
        drop(self.child.stdin.take());
        ensure!(self.child.wait()?.success(), "the lingering child failed");
        Ok(())
    }
}

/// The identity of a process that has exited and been reaped.
fn dead_process(corral: &Path) -> Result<ProcessIdentity> {
    let child = Lingering::start(corral)?;
    let identity = child.identity.clone();
    child.end()?;
    ensure!(proven_dead(&identity)?, "a reaped child is proven dead");
    Ok(identity)
}

macro_rules! barrier_suite {
    ($($case:ident),* $(,)?) => {
        mod fake_and_fs {
            $(#[test] fn $case() -> anyhow::Result<()> { super::$case(super::Env::fake()?) })*
        }
        mod sqlite_and_fs {
            $(#[test] fn $case() -> anyhow::Result<()> { super::$case(super::Env::sqlite()?) })*
        }
    };
}

barrier_suite!(
    barrier_1_a_reserve_before_begin_delete_keeps_the_blob,
    barrier_2_a_put_after_begin_delete_is_busy_then_rewrites,
    barrier_3_a_reader_keeps_content_for_the_grace_period,
    barrier_3_a_live_lease_holds_back_the_horizon,
    barrier_4_the_next_gc_completes_a_crashed_deletion,
    barrier_5_takeover_needs_proof_of_death,
    barrier_6_only_strays_older_than_the_grace_are_deleted,
    barrier_7_refs_in_records_events_and_runs_keep_blobs,
);

fn barrier_1_a_reserve_before_begin_delete_keeps_the_blob(env: Env) -> Result<()> {
    let content = b"orphan reserved again";
    let hash = env.orphan(content)?;
    env.clock.advance(BLOB_GRACE + HOUR);
    let reserved = RefCell::new(None);
    let barrier = |point: GcPoint<'_>| -> Result<()> {
        if let GcPoint::Selected(selected) = point
            && *selected == hash
        {
            match env.try_put(content)? {
                TryPut::Reserved(blob) => *reserved.borrow_mut() = Some(blob),
                TryPut::Busy(_) => bail!("busy before BeginDelete"),
            }
        }
        Ok(())
    };
    let GcRun::Done(report) = env.gc()?.barrier(&barrier).run()? else {
        bail!("claim held");
    };
    ensure!(
        report.deleted.is_empty() && report.kept.iter().any(|kept| kept.hash == hash),
        "report {report:?}"
    );
    let blob = reserved.into_inner().context("the putter reserved")?;
    ensure!(!blob.created, "the row existed");
    env.reference("late", &blob)?;
    env.ensure_live_with_file(&hash, content, 1)
}

fn barrier_2_a_put_after_begin_delete_is_busy_then_rewrites(env: Env) -> Result<()> {
    let content = b"orphan put while deleting";
    let hash = env.orphan(content)?;
    env.clock.advance(BLOB_GRACE + HOUR);
    let busy = RefCell::new(None);
    let barrier = |point: GcPoint<'_>| -> Result<()> {
        if let GcPoint::Deleting(deleting) = point
            && *deleting == hash
        {
            ensure!(
                env.row(&hash)?
                    .is_some_and(|r| r.state == BlobState::Deleting),
                "row deleting"
            );
            match env.try_put(content)? {
                TryPut::Busy(staged) => *busy.borrow_mut() = Some(staged),
                TryPut::Reserved(_) => bail!("reserved a deleting blob"),
            }
        }
        Ok(())
    };
    let GcRun::Done(report) = env.gc()?.barrier(&barrier).run()? else {
        bail!("claim held");
    };
    ensure!(report.deleted == vec![hash.clone()], "report {report:?}");
    ensure!(env.row(&hash)?.is_none() && !env.has_file(&hash), "deleted");
    let staged = busy.into_inner().context("the putter was busy")?;
    let TryPut::Reserved(blob) = env.putter()?.try_put(staged)? else {
        bail!("busy after FinishDelete");
    };
    ensure!(blob.created, "rewritten as a new row");
    env.reference("rewritten", &blob)?;
    env.ensure_live_with_file(&hash, content, 1)
}

fn barrier_3_a_reader_keeps_content_for_the_grace_period(env: Env) -> Result<()> {
    let content = b"read by a view taken at S";
    let blob = env.put(content)?;
    env.reference("doc", &blob)?;
    let held: BlobRef = {
        let view = env.backend.view()?;
        let q = RecordQuery {
            area: Some("refs".to_owned()),
            key: Some("doc".to_owned()),
        };
        let doc = view.records(&q, PageReq::first(1))?.items.remove(0).doc;
        doc["log"].as_str().context("log ref")?.parse()?
    };
    let s = env.clock.now();
    env.unreference("doc")?;
    ensure!(
        env.row(held.hash())?
            .is_some_and(|r| r.refs == 0 && r.unreferenced_since == Some(s)),
        "purged at S"
    );
    env.clock.advance(BLOB_GRACE - SECOND);
    let report = env.collect()?;
    ensure!(
        report.deleted.is_empty(),
        "deleted at S+grace-1s: {report:?}"
    );
    ensure!(env.read(held.hash())? == content, "the reader still reads");
    env.clock.advance(2 * SECOND);
    let report = env.collect()?;
    ensure!(
        report.deleted == vec![held.hash().clone()],
        "S+grace+1s: {report:?}"
    );
    ensure!(
        env.row(held.hash())?.is_none() && env.blobs.get(held.hash()).is_err(),
        "gone"
    );
    Ok(())
}

fn barrier_3_a_live_lease_holds_back_the_horizon(env: Env) -> Result<()> {
    let lease = |op| env.apply(Operation::Lease(op)).map(drop);
    lease(LeaseOp::Acquire {
        id: "backup".to_owned(),
        holder: "backup-1".to_owned(),
        expires: env
            .clock
            .now()
            .checked_add(SignedDuration::from_hours(72))?,
    })?;
    env.clock.advance(HOUR);
    let hash = env.orphan(b"unreferenced during a backup")?;
    env.clock.advance(BLOB_GRACE + HOUR);
    let report = env.collect()?;
    ensure!(
        report.deleted.is_empty() && report.horizon < Some(env.ago(BLOB_GRACE)?),
        "the lease started before the orphan: {report:?}"
    );
    lease(LeaseOp::Release {
        id: "backup".to_owned(),
        holder: "backup-1".to_owned(),
    })?;
    ensure!(
        env.collect()?.deleted == vec![hash],
        "deleted after release"
    );
    Ok(())
}

fn barrier_4_the_next_gc_completes_a_crashed_deletion(env: Env) -> Result<()> {
    let content = b"unlinked, never finished";
    let hash = env.orphan(content)?;
    env.clock.advance(BLOB_GRACE + HOUR);
    let crashed = dead_process(&env.corral)?;
    env.hold_and_begin(&crashed, &hash)?;
    env.blobs.delete(&hash)?;

    let TryPut::Busy(staged) = env.try_put(content)? else {
        bail!("a putter reserved a deleting blob");
    };
    let TryPut::Busy(staged) = env.putter()?.try_put(staged)? else {
        bail!("still deleting until the next GC");
    };
    let report = env.collect()?;
    ensure!(report.completed == vec![hash.clone()], "report {report:?}");
    ensure!(
        env.row(&hash)?.is_none() && env.claims()?.is_empty(),
        "completed and released"
    );
    let TryPut::Reserved(blob) = env.putter()?.try_put(staged)? else {
        bail!("busy after the GC completed");
    };
    ensure!(blob.created, "rewritten");
    env.reference("again", &blob)?;
    env.ensure_live_with_file(&hash, content, 1)
}

fn barrier_5_takeover_needs_proof_of_death(env: Env) -> Result<()> {
    let content = b"deleting under a live holder";
    let hash = env.orphan(content)?;
    env.clock.advance(BLOB_GRACE + HOUR);
    let child = Lingering::start(&env.corral)?;
    let holder = child.identity.clone();
    env.hold_and_begin(&holder, &hash)?;
    ensure!(!proven_dead(&holder)?, "a live holder is not proven dead");
    match env.gc()?.run()? {
        GcRun::ClaimHeld { owner } if owner == holder.owner() => {}
        other => bail!("GC took the claim from a live holder: {other:?}"),
    }

    let mut child = Some(child);
    let mut waits = 0;
    let blob = env.putter()?.put(&mut &content[..], &mut |attempt| {
        waits += 1;
        ensure!(attempt == 1, "waited again after the holder died");
        let row = env.row(&hash)?.context("row")?;
        let claims = env.claims()?;
        ensure!(
            row.state == BlobState::Deleting
                && env.has_file(&hash)
                && claims.len() == 1
                && claims[0].owner == holder.owner(),
            "taken over from a live holder: {row:?} {claims:?}"
        );
        child.take().context("the holder exits once")?.end()
    })?;
    ensure!(waits == 1, "waits {waits}");
    ensure!(blob.created, "the takeover finished the deletion first");
    ensure!(env.claims()?.is_empty(), "the putter released the claim");
    env.reference("after", &blob)?;
    env.ensure_live_with_file(&hash, content, 1)?;

    let other = env.orphan(b"a reused pid")?;
    env.clock.advance(BLOB_GRACE + HOUR);
    let reused = ProcessIdentity {
        start_time: "not-the-start-time".to_owned(),
        ..ProcessIdentity::current()?
    };
    env.hold_and_begin(&reused, &other)?;
    let report = env.collect()?;
    ensure!(
        report.completed == vec![other],
        "pid reuse is proof: {report:?}"
    );

    env.apply(Operation::Claim(Claim {
        name: STORAGE_GC_CLAIM.to_owned(),
        owner: "unknown-holder".to_owned(),
        acquired: env.clock.now(),
        doc: Value::Null,
        reservations: Vec::new(),
    }))?;
    match env.gc()?.run()? {
        GcRun::ClaimHeld { owner } if owner == "unknown-holder" => Ok(()),
        other => bail!("an unrecognized holder record was taken as proof: {other:?}"),
    }
}

fn stray(env: &Env, bytes: &[u8], modified: Timestamp) -> Result<BlobHash> {
    let staged = env.blobs.stage(&mut &bytes[..])?;
    let hash = staged.hash.clone();
    env.blobs.put_staged(staged)?;
    fs::File::options()
        .write(true)
        .open(env.blobs.path_of(&hash))?
        .set_modified(modified.into())?;
    Ok(hash)
}

fn barrier_6_only_strays_older_than_the_grace_are_deleted(env: Env) -> Result<()> {
    let young = stray(&env, b"young stray", env.ago(BLOB_GRACE - HOUR)?)?;
    let old = stray(&env, b"old stray", env.ago(BLOB_GRACE + HOUR)?)?;
    let kept = env.put(b"an old file with a row")?;
    env.reference("kept", &kept)?;
    fs::File::options()
        .write(true)
        .open(env.blobs.path_of(kept.blob.hash()))?
        .set_modified(env.ago(30 * BLOB_GRACE)?.into())?;
    let report = env.collect()?;
    ensure!(report.strays == vec![old.clone()], "report {report:?}");
    ensure!(
        env.has_file(&young) && !env.has_file(&old) && env.has_file(kept.blob.hash()),
        "only the old stray went"
    );
    ensure!(
        env.row(&young)?.is_none() && env.row(&old)?.is_none(),
        "no rows left"
    );
    env.ensure_live_with_file(kept.blob.hash(), b"an old file with a row", 1)
}

fn barrier_7_refs_in_records_events_and_runs_keep_blobs(env: Env) -> Result<()> {
    let now = env.clock.now();
    let in_record = env.put(b"held by a record document")?;
    env.apply(Operation::PutRecords(PutRecords {
        writes: vec![record(
            "t251",
            "receipt",
            None,
            Some(json!({"gate": {"log": in_record.blob.to_string()}})),
            now,
        )],
        events: Vec::new(),
        reservations: vec![in_record.reservation.clone()],
    }))?;
    let in_event = env.put(b"held by an event detail")?;
    env.apply(Operation::AppendEvents(AppendEvents {
        events: vec![NewEvent {
            ts: now,
            task: None,
            kind: "gate".to_owned(),
            actor: Some("agent".to_owned()),
            via: None,
            run: Some("R1".to_owned()),
            detail: json!({"nested": [{"log": in_event.blob.to_string()}]}),
        }],
        reservations: vec![in_event.reservation.clone()],
    }))?;
    let in_run = env.put(b"held by a run row")?;
    env.apply(Operation::CreateTask(CreateTask {
        id_prefix: "T".to_owned(),
        created: now,
        fields: TaskFields {
            title: "refs".to_owned(),
            status: "in_progress".to_owned(),
            project: Some("corral".to_owned()),
            priority: "P1".to_owned(),
            parent: None,
            updated: now,
            edges: Vec::new(),
            kind: None,
            size: None,
            tracker: false,
            design: false,
            queued: false,
            spec_rev: String::new(),
            dashboard_due: None,
        },
        doc: json!({"body": "refs"}),
        summary: json!({}),
        rows: vec![HistoryRow {
            part: TaskPart::Run,
            key: "R1".to_owned(),
            current: true,
            open: true,
            doc: json!({"prompt": in_run.blob.to_string()}),
            index: Vec::new(),
        }],
        events: Vec::new(),
        reservations: vec![in_run.reservation.clone()],
    }))?;
    env.clock.advance(30 * BLOB_GRACE);
    let report = env.collect()?;
    ensure!(
        report.deleted.is_empty() && report.kept.is_empty(),
        "report {report:?}"
    );
    for (blob, bytes) in [
        (&in_record, &b"held by a record document"[..]),
        (&in_event, b"held by an event detail"),
        (&in_run, b"held by a run row"),
    ] {
        env.ensure_live_with_file(blob.blob.hash(), bytes, 1)?;
    }
    Ok(())
}

#[cfg(target_os = "linux")]
#[test]
fn proof_of_death_needs_a_gone_pid_another_start_time_or_another_boot() -> Result<()> {
    let me = ProcessIdentity::current()?;
    ensure!(!proven_dead(&me)?, "this process is alive");
    let rebooted = ProcessIdentity {
        boot: "another-boot".to_owned(),
        ..me.clone()
    };
    ensure!(proven_dead(&rebooted)?, "another boot");
    let reused = ProcessIdentity {
        start_time: "1".to_owned(),
        ..me
    };
    ensure!(proven_dead(&reused)?, "pid reuse");
    let dir = TempDir::new()?;
    dead_process(dir.path()).map(drop)
}

#[test]
fn a_gc_process_killed_between_unlink_and_finish_delete_is_completed_by_the_next() -> Result<()> {
    let env = Env::sqlite()?;
    let content = b"killed mid-deletion";
    let hash = env.orphan(content)?;
    env.clock.advance(BLOB_GRACE + HOUR);
    let clock = env.clock.now().to_string();
    let (mut child, lines) = spawn("gc-crash", &env.corral, &[(CLOCK_ENV, clock)])?;
    let unlinked = lines.wait_for("unlinked ")?;
    ensure!(unlinked == format!("unlinked {hash}"), "{unlinked}");
    let gc = ProcessIdentity::of(child.id())?.context("the GC child is alive")?;
    child.kill()?;
    child.wait()?;

    ensure!(
        env.row(&hash)?
            .is_some_and(|r| r.state == BlobState::Deleting)
            && !env.has_file(&hash),
        "unlinked, row still deleting"
    );
    let claims = env.claims()?;
    ensure!(
        claims.len() == 1 && claims[0].owner == gc.owner(),
        "the killed GC still holds the claim: {claims:?}"
    );
    let TryPut::Busy(staged) = env.try_put(content)? else {
        bail!("a putter reserved a deleting blob");
    };
    let report = env.collect()?;
    ensure!(report.completed == vec![hash.clone()], "report {report:?}");
    let TryPut::Reserved(blob) = env.putter()?.try_put(staged)? else {
        bail!("busy after the GC completed");
    };
    env.reference("again", &blob)?;
    env.ensure_live_with_file(&hash, content, 1)
}
