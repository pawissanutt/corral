//! Acceptance checks of plan T-253 blob-store for the filesystem blob store: four processes
//! putting one log, streams in bounded memory (256 MiB here, the 2 GiB acceptance stream with
//! the ignored stress tests), altered and truncated content, and the conformance blob cases on
//! FakeBackend + FsBlobStore and SqliteBackend + FsBlobStore.

mod backend {
    pub use corral_core::backend::*;
}
mod blob_ref {
    pub use corral_core::blob_ref::*;
}
mod sqlite_support;

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail, ensure};
use corral_core::backend::conformance::{Fixture, run_case};
use corral_core::backend::fake::FakeBackend;
use corral_core::backend::*;
use corral_core::blob::{
    BlobProblem, CODEC, FsBlobStore, Gc, GcRun, PutBlob, Putter, TryPut, check_blobs,
};
use corral_core::blob_ref::BlobHash;
use corral_core::sqlite::SqliteBackend;
use jiff::Timestamp;
use serde_json::json;
use sqlite_support::{ChildLines, child_command, child_role, hear, options, record, tell};
use tempfile::TempDir;

const ENTRY: &str = "zz_child_process_role";
const LOG_ENV: &str = "CORRAL_BLOB_TEST_LOG";
const KEY_ENV: &str = "CORRAL_BLOB_TEST_KEY";
const SIZE_ENV: &str = "CORRAL_BLOB_TEST_SIZE";
const MIB: u64 = 1024 * 1024;
const STEP: u64 = 256 * MIB;

/// The entry child processes run; a no-op in a normal test run.
#[test]
fn zz_child_process_role() -> Result<()> {
    let Some((role, corral)) = child_role() else {
        return Ok(());
    };
    match role.as_str() {
        "put" => child_put(&corral),
        "big" => child_big(&corral),
        other => bail!("unknown child role {other}"),
    }
}

struct Store {
    backend: SqliteBackend,
    blobs: FsBlobStore,
    clock: ManualClock,
}

fn open(corral: &Path) -> Result<Store> {
    let clock = ManualClock::new(Timestamp::now());
    let faults = Arc::new(FaultInjector::default());
    Ok(Store {
        backend: SqliteBackend::open(corral, options(&clock, &faults, None))?,
        blobs: FsBlobStore::open(corral)?,
        clock,
    })
}

impl Store {
    fn putter(&self) -> Result<Putter<'_>> {
        Putter::new(&self.backend, &self.blobs, &self.clock)
    }

    fn reference(&self, key: &str, blob: &PutBlob) -> Result<()> {
        let doc = json!({"log": blob.blob.to_string()});
        let write = record("logs", key, None, Some(doc), self.clock.now());
        let op = Operation::PutRecords(PutRecords {
            writes: vec![write],
            events: Vec::new(),
            reservations: vec![blob.reservation.clone()],
        });
        self.backend
            .apply(op)
            .map(drop)
            .map_err(|error| anyhow!("reference {key}: {error}"))
    }

    fn rows(&self) -> Result<Vec<BlobRow>> {
        let view = self.backend.view()?;
        Ok(view
            .blobs(&BlobQuery::default(), PageReq::first(100))?
            .items)
    }
}

fn child_put(corral: &Path) -> Result<()> {
    let log = PathBuf::from(std::env::var_os(LOG_ENV).context("log path")?);
    let key = std::env::var(KEY_ENV).context("record key")?;
    let store = open(corral)?;
    let staged = store.blobs.stage(&mut fs::File::open(&log)?)?;
    tell(&format!("staged {}", staged.hash))?;
    ensure!(hear()? == "go", "put child: no go signal");
    let blob = match store.putter()?.try_put(staged)? {
        TryPut::Reserved(blob) => blob,
        TryPut::Busy(_) => bail!("put child: busy"),
    };
    store.reference(&key, &blob)?;
    tell(&format!("put {} {}", blob.blob, blob.created))
}

fn spawn(role: &str, corral: &Path, env: &[(&str, &str)]) -> Result<(Child, ChildLines)> {
    let mut command = child_command(ENTRY, role, corral)?;
    command
        .envs(env.iter().copied())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    let mut child = command.spawn().context("spawn a blob child")?;
    let lines = ChildLines::new(child.stdout.take().context("child stdout")?);
    Ok((child, lines))
}

fn files_under(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    for entry in fs::read_dir(dir).with_context(|| format!("list {}", dir.display()))? {
        let path = entry?.path();
        if path.is_dir() {
            found.extend(files_under(&path)?);
        } else {
            found.push(path);
        }
    }
    Ok(found)
}

fn read_all(blobs: &FsBlobStore, hash: &BlobHash) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    blobs.get(hash)?.read_to_end(&mut out)?;
    Ok(out)
}

#[test]
fn four_processes_putting_one_log_store_it_once() -> Result<()> {
    let dir = TempDir::new()?;
    let corral = dir.path().join(".corral");
    let store = open(&corral)?;
    let log = dir.path().join("gate.log");
    let mut text = String::new();
    let mut line = 0u64;
    while (text.len() as u64) < 11 * MIB {
        text.push_str(&format!("{line:08} cargo test step output: ok, 0 failed\n"));
        line += 1;
    }
    fs::write(&log, &text)?;
    let expected = BlobHash::of(text.as_bytes());

    let log_path = log.to_string_lossy().into_owned();
    let mut children = Vec::new();
    for key in ["caller-0", "caller-1", "caller-2", "caller-3"] {
        children.push(spawn(
            "put",
            &corral,
            &[(LOG_ENV, log_path.as_str()), (KEY_ENV, key)],
        )?);
    }
    for (_, lines) in &children {
        let staged = lines.wait_for("staged ")?;
        ensure!(staged == format!("staged {expected}"), "{staged}");
    }
    for (child, _) in &mut children {
        let stdin = child.stdin.as_mut().context("child stdin")?;
        writeln!(stdin, "go")?;
        stdin.flush()?;
    }
    let mut refs = Vec::new();
    let mut created = 0;
    for (_, lines) in &children {
        let put = lines.wait_for("put ")?;
        let parts: Vec<&str> = put.split(' ').collect();
        refs.push(parts.get(1).copied().unwrap_or_default().to_owned());
        created += usize::from(parts.get(2) == Some(&"true"));
    }
    for (mut child, _) in children {
        ensure!(child.wait()?.success(), "a put child failed");
    }

    let want = expected.to_ref().to_string();
    ensure!(
        refs.iter().all(|r| *r == want),
        "refs {refs:?}, want {want}"
    );
    ensure!(created == 1, "{created} callers inserted the row");
    let blob_root = corral.join("blobs");
    let files = files_under(&blob_root)?;
    let (fanout, name) = expected.as_str().split_at(2);
    let path = blob_root.join("sha256").join(fanout).join(name);
    ensure!(files == vec![path.clone()], "files {files:?}");
    let rows = store.rows()?;
    ensure!(rows.len() == 1, "rows {rows:?}");
    let row = &rows[0];
    ensure!(
        row.hash == expected
            && row.state == BlobState::Live
            && row.refs == 4
            && row.reservations.is_empty()
            && row.codec == CODEC
            && row.raw_bytes == text.len() as u64
            && row.stored_bytes == fs::metadata(&path)?.len(),
        "row {row:?}"
    );
    ensure!(
        read_all(&store.blobs, &expected)? == text.as_bytes(),
        "round trip"
    );
    ensure!(check_blobs(&store.backend, &store.blobs)?.is_ok(), "check");
    Ok(())
}

/// A stream of a repeated 1 MiB pseudo-random block, generated as it is read.
struct Synthetic {
    left: u64,
    read: u64,
    block: Vec<u8>,
    at: usize,
}

impl Synthetic {
    fn new(len: u64) -> Self {
        let mut state = 0x9e37_79b9_7f4a_7c15u64;
        let block = (0..MIB)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                state.to_le_bytes()[0]
            })
            .collect();
        Self {
            left: len,
            read: 0,
            block,
            at: 0,
        }
    }
}

/// Tells the parent each time `after` crosses a multiple of [`STEP`], so every wait for the
/// child stays bounded however long the whole stream takes.
fn progress(what: &str, before: u64, after: u64) -> io::Result<()> {
    if after / STEP > before / STEP {
        tell(&format!("progress {what} {after}")).map_err(io::Error::other)?;
    }
    Ok(())
}

#[derive(Default)]
struct ReadBack {
    written: u64,
}

impl Write for ReadBack {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let before = self.written;
        self.written += buf.len() as u64;
        progress("read", before, self.written)?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Read for Synthetic {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let left = usize::try_from(self.left).unwrap_or(usize::MAX);
        let n = buf.len().min(self.block.len() - self.at).min(left);
        buf[..n].copy_from_slice(&self.block[self.at..self.at + n]);
        self.at = (self.at + n) % self.block.len();
        self.left -= n as u64;
        let before = self.read;
        self.read += n as u64;
        progress("staged", before, self.read)?;
        Ok(n)
    }
}

fn peak_rss_kib() -> Result<u64> {
    let status = fs::read_to_string("/proc/self/status")?;
    let line = status
        .lines()
        .find(|line| line.starts_with("VmHWM:"))
        .context("no VmHWM")?;
    Ok(line
        .split_whitespace()
        .nth(1)
        .context("VmHWM value")?
        .parse()?)
}

fn child_big(corral: &Path) -> Result<()> {
    let size: u64 = std::env::var(SIZE_ENV).context("stream size")?.parse()?;
    let before = peak_rss_kib()?;
    let store = open(corral)?;
    let blob = store.putter()?.put(&mut Synthetic::new(size), &mut |_| {
        bail!("unexpectedly busy")
    })?;
    store.reference("big", &blob)?;
    let read = io::copy(
        &mut store.blobs.get(blob.blob.hash())?,
        &mut ReadBack::default(),
    )?;
    let after = peak_rss_kib()?;
    tell(&format!(
        "big {} {} {} {before} {after} {read}",
        blob.blob, blob.raw_bytes, blob.stored_bytes
    ))
}

/// A child still running when a test returns early is killed, so a failure leaves no process.
struct Reaped(Child);

impl Drop for Reaped {
    fn drop(&mut self) {
        if matches!(self.0.try_wait(), Ok(None)) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

fn put_in_bounded_memory(size: u64) -> Result<()> {
    let dir = TempDir::new()?;
    let corral = dir.path().join(".corral");
    let store = open(&corral)?;
    let size_text = size.to_string();
    let (child, lines) = spawn("big", &corral, &[(SIZE_ENV, size_text.as_str())])?;
    let mut child = Reaped(child);
    for _ in 0..2 * (size / STEP) {
        lines.wait_for("progress ")?;
    }
    let report = lines.wait_for("big ")?;
    ensure!(child.0.wait()?.success(), "the stream child failed");
    let fields: Vec<&str> = report.split(' ').collect();
    let number = |i: usize| -> Result<u64> { Ok(fields.get(i).context("report field")?.parse()?) };
    let (raw, stored, before, after, read) =
        (number(2)?, number(3)?, number(4)?, number(5)?, number(6)?);
    let hash: corral_core::blob_ref::BlobRef = fields.get(1).context("ref")?.parse()?;
    let hash = hash.hash().clone();
    ensure!(raw == size && read == size, "raw {raw}, read back {read}");
    let path = store.blobs.path_of(&hash);
    ensure!(stored == fs::metadata(&path)?.len(), "stored {stored}");
    let growth = after.saturating_sub(before);
    ensure!(
        growth < 32 * 1024,
        "peak memory grew {growth} KiB ({before} -> {after}) for a {size}-byte stream"
    );
    let rows = store.rows()?;
    ensure!(
        rows.len() == 1
            && rows[0].hash == hash
            && rows[0].raw_bytes == size
            && rows[0].stored_bytes == stored
            && rows[0].state == BlobState::Live,
        "rows {rows:?}"
    );
    println!(
        "bounded put: raw {raw} B, stored {stored} B, read back {read} B, \
         peak RSS {before} -> {after} KiB (+{growth} KiB)"
    );
    Ok(())
}

#[cfg(target_os = "linux")]
#[test]
fn a_256_mib_stream_is_put_and_read_in_bounded_memory() -> Result<()> {
    put_in_bounded_memory(STEP)
}

/// The acceptance size takes about a minute in a debug build, so it runs with the stress tests.
#[cfg(target_os = "linux")]
#[test]
#[ignore = "stress: 2 GiB stream; run with --ignored"]
fn a_2_gib_stream_is_put_and_read_in_bounded_memory() -> Result<()> {
    put_in_bounded_memory(8 * STEP)
}

#[test]
fn altered_truncated_and_missing_content_is_reported_and_never_collected() -> Result<()> {
    let dir = TempDir::new()?;
    let store = open(&dir.path().join(".corral"))?;
    let put = |key: &str, text: String| -> Result<BlobHash> {
        let blob = store
            .putter()?
            .put(&mut text.as_bytes(), &mut |_| bail!("unexpectedly busy"))?;
        store.reference(key, &blob)?;
        Ok(blob.blob.hash().clone())
    };
    let body = |tag: &str| {
        (0..20_000)
            .map(|i| format!("{tag} line {i}\n"))
            .collect::<String>()
    };
    let flipped = put("flipped", body("flipped"))?;
    let truncated = put("truncated", body("truncated"))?;
    let replaced = put("replaced", body("replaced"))?;
    let missing = put("missing", body("missing"))?;
    let intact = put("intact", body("intact"))?;

    let mut bytes = fs::read(store.blobs.path_of(&flipped))?;
    let middle = bytes.len() / 2;
    bytes[middle] ^= 0xff;
    fs::write(store.blobs.path_of(&flipped), bytes)?;
    let bytes = fs::read(store.blobs.path_of(&truncated))?;
    fs::write(store.blobs.path_of(&truncated), &bytes[..bytes.len() / 2])?;
    let other = zstd::encode_all(&b"different content"[..], 3)?;
    fs::write(store.blobs.path_of(&replaced), other)?;
    fs::remove_file(store.blobs.path_of(&missing))?;

    for hash in [&flipped, &truncated, &replaced] {
        let error = read_all(&store.blobs, hash).err().map(|e| format!("{e:#}"));
        ensure!(
            error
                .as_deref()
                .is_some_and(|e| e.contains("HashMismatch") && e.contains(hash.as_str())),
            "reading {hash}: {error:?}"
        );
    }
    let report = check_blobs(&store.backend, &store.blobs)?;
    ensure!(report.checked == 5, "checked {}", report.checked);
    let mut found: Vec<(String, BlobHash)> = report
        .problems
        .iter()
        .map(|problem| {
            let kind = match problem {
                BlobProblem::Missing { .. } => "missing",
                BlobProblem::HashMismatch { .. } => "mismatch",
            };
            (kind.to_owned(), problem.hash().clone())
        })
        .collect();
    found.sort();
    let mut want = vec![
        ("mismatch".to_owned(), flipped.clone()),
        ("mismatch".to_owned(), truncated.clone()),
        ("mismatch".to_owned(), replaced.clone()),
        ("missing".to_owned(), missing.clone()),
    ];
    want.sort();
    ensure!(found == want, "problems {:?}", report.problems);
    ensure!(
        report
            .problems
            .iter()
            .all(|p| p.to_string().contains(p.hash().as_str())),
        "every problem names its hash"
    );

    store.clock.advance(Duration::from_secs(30 * 24 * 3600));
    let gc = Gc::new(&store.backend, &store.blobs, &store.clock)?;
    let GcRun::Done(collected) = gc.run()? else {
        bail!("the claim was held");
    };
    ensure!(
        collected.deleted.is_empty() && collected.strays.is_empty(),
        "GC deleted referenced blobs: {collected:?}"
    );
    let rows = store.rows()?;
    ensure!(
        rows.len() == 5
            && rows
                .iter()
                .all(|r| r.state == BlobState::Live && r.refs == 1),
        "rows {rows:?}"
    );
    for hash in [&flipped, &truncated, &replaced, &intact] {
        ensure!(store.blobs.path_of(hash).is_file(), "{hash} was unlinked");
    }
    Ok(())
}

const FS_CASES: [&str; 3] = [
    "blob_store_contract",
    "blob_transitions",
    "documents_keep_their_blobs",
];

fn fs_fixture(
    dir: Arc<TempDir>,
    backend: Box<dyn StoreBackend>,
    second: Box<dyn StoreBackend>,
    clock: ManualClock,
    faults: Arc<FaultInjector>,
    prune: PruneChanges,
) -> Result<Fixture> {
    let corral = dir.path().join(".corral");
    fs::create_dir_all(&corral)?;
    let root = corral.join("blobs");
    let blobs = FsBlobStore::at(&root)?;
    let paths = FsBlobStore::at(&root)?;
    Ok(Fixture {
        backend,
        second,
        blobs: Box::new(blobs),
        clock,
        faults,
        config: BackendConfig::default(),
        prune_changes_through: prune,
        corrupt_blob: Box::new(move |hash, bytes| {
            let _keep = &dir;
            fs::write(paths.path_of(hash), bytes).context("corrupt a blob file")
        }),
    })
}

type PruneChanges = Box<dyn Fn(ChangeSeq) -> Result<()>>;

fn fake_fs_fixture() -> Fixture {
    let made = TempDir::new().map_err(anyhow::Error::from).and_then(|dir| {
        let clock = ManualClock::new(corral_core::backend::conformance::epoch());
        let faults = Arc::new(FaultInjector::default());
        let backend = FakeBackend::new(
            BackendConfig::default(),
            Arc::new(clock.clone()),
            Arc::clone(&faults),
        );
        let pruner = backend.handle();
        let prune: PruneChanges = Box::new(move |seq| {
            pruner.prune_changes_through(seq);
            Ok(())
        });
        let second = Box::new(backend.handle());
        fs_fixture(
            Arc::new(dir),
            Box::new(backend),
            second,
            clock,
            faults,
            prune,
        )
    });
    made.unwrap_or_else(|error| panic!("open a fake + filesystem fixture: {error:#}"))
}

fn sqlite_fs_fixture() -> Fixture {
    let made = TempDir::new().map_err(anyhow::Error::from).and_then(|dir| {
        let clock = ManualClock::new(corral_core::backend::conformance::epoch());
        let faults = Arc::new(FaultInjector::default());
        let corral = dir.path().join(".corral");
        let backend = SqliteBackend::open(&corral, options(&clock, &faults, None))?;
        let pruner = backend.handle()?;
        let prune: PruneChanges = Box::new(move |seq| pruner.prune_changes_through(seq));
        let second = Box::new(backend.handle()?);
        fs_fixture(
            Arc::new(dir),
            Box::new(backend),
            second,
            clock,
            faults,
            prune,
        )
    });
    made.unwrap_or_else(|error| panic!("open a SQLite + filesystem fixture: {error:#}"))
}

#[test]
fn conformance_blob_cases_pass_on_fake_with_the_filesystem_store() -> Result<()> {
    FS_CASES
        .iter()
        .try_for_each(|case| run_case(case, &fake_fs_fixture))
}

#[test]
fn conformance_blob_cases_pass_on_sqlite_with_the_filesystem_store() -> Result<()> {
    FS_CASES
        .iter()
        .try_for_each(|case| run_case(case, &sqlite_fs_fixture))
}

#[test]
fn listing_pages_in_hash_order_and_a_sweep_drops_only_old_staging_files() -> Result<()> {
    let dir = TempDir::new()?;
    let blobs = FsBlobStore::at(dir.path().join("blobs"))?;
    let mut hashes = Vec::new();
    for i in 0..40 {
        let staged = blobs.stage(&mut format!("content {i}").as_bytes())?;
        hashes.push(staged.hash.clone());
        blobs.put_staged(staged)?;
    }
    hashes.sort();
    let mut listed = Vec::new();
    let mut after: Option<BlobHash> = None;
    loop {
        let page = blobs.list(after.as_ref(), 7)?;
        let Some(last) = page.last() else { break };
        after = Some(last.hash.clone());
        listed.extend(page.into_iter().map(|file| file.hash));
    }
    ensure!(listed == hashes, "paged listing out of order or incomplete");
    ensure!(
        files_under(blobs.staging_dir())?.is_empty(),
        "put_staged left staging files"
    );

    let old = blobs.stage(&mut &b"abandoned before put"[..])?;
    let fresh = blobs.stage(&mut &b"still being put"[..])?;
    let (StagedData::File(old_path), StagedData::File(fresh_path)) = (old.data, fresh.data) else {
        bail!("the filesystem store stages to files");
    };
    let now = Timestamp::now();
    let day_ago = now.checked_sub(jiff::SignedDuration::from_hours(25))?;
    fs::File::options()
        .write(true)
        .open(&old_path)?
        .set_modified(day_ago.into())?;
    let cutoff = now.checked_sub(jiff::SignedDuration::from_hours(24))?;
    ensure!(blobs.sweep_staging(cutoff)? == 1, "one old staging file");
    ensure!(
        !old_path.exists() && fresh_path.exists(),
        "only the old one went"
    );
    Ok(())
}
