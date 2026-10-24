//! Publication durability of the filesystem blob store (plan T-253 section 1: `put_staged`
//! fsyncs the file, renames it and fsyncs the directory). Every successful open and put syncs
//! what it relies on even when another publisher created it, and a put whose sync failed or has
//! not finished never hands out a reservation an operation could reference. Syncs are observed,
//! failed and paused through the store's sync hook; nothing waits on time.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, mpsc};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail, ensure};
use corral_core::backend::fake::FakeBackend;
use corral_core::backend::*;
use corral_core::blob::{FsBlobStore, PutBlob, Putter, SyncHook, SyncTarget, TryPut};
use corral_core::blob_ref::BlobHash;
use jiff::Timestamp;
use serde_json::json;
use tempfile::TempDir;

use SyncTarget::{Dir, File};

/// How long a test waits for a paused sync to be reached or resumed before it fails.
const TIMEOUT: Duration = Duration::from_secs(60);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

struct Pause {
    target: SyncTarget,
    path: PathBuf,
    reached: mpsc::Sender<()>,
    resume: Mutex<mpsc::Receiver<()>>,
}

/// One sync the hook let through: what was synced and whether the watched path existed then.
type Synced = (SyncTarget, PathBuf, bool);

/// A sync hook that records every sync, fails the selected ones and pauses at most one.
#[derive(Clone, Default)]
struct Syncs {
    synced: Arc<Mutex<Vec<Synced>>>,
    failing: Arc<Mutex<Vec<(SyncTarget, PathBuf)>>>,
    fail_every: Arc<AtomicBool>,
    watch: Arc<Mutex<Option<PathBuf>>>,
    pause: Arc<Mutex<Option<Arc<Pause>>>>,
}

impl Syncs {
    fn hook(&self) -> SyncHook {
        let me = self.clone();
        Arc::new(move |target, path| me.on_sync(target, path))
    }

    fn on_sync(&self, target: SyncTarget, path: &Path) -> io::Result<()> {
        let pause = {
            let mut slot = lock(&self.pause);
            let hit = slot
                .as_ref()
                .is_some_and(|p| p.target == target && p.path == path);
            if hit { slot.take() } else { None }
        };
        if let Some(pause) = pause {
            let _ = pause.reached.send(());
            lock(&pause.resume)
                .recv_timeout(TIMEOUT)
                .map_err(|_| io::Error::other("the paused sync was never resumed"))?;
        }
        let fail = self.fail_every.load(Ordering::SeqCst)
            || lock(&self.failing)
                .iter()
                .any(|(t, p)| *t == target && p == path);
        if fail {
            return Err(io::Error::other("injected EIO"));
        }
        let watched = lock(&self.watch).as_ref().is_some_and(|p| p.exists());
        lock(&self.synced).push((target, path.to_owned(), watched));
        Ok(())
    }

    fn fail(&self, target: SyncTarget, path: &Path) {
        lock(&self.failing).push((target, path.to_owned()));
    }

    fn fail_every(&self, on: bool) {
        self.fail_every.store(on, Ordering::SeqCst);
    }

    fn heal(&self) {
        lock(&self.failing).clear();
        self.fail_every(false);
    }

    fn watch(&self, path: &Path) {
        *lock(&self.watch) = Some(path.to_owned());
    }

    /// Pauses the next sync of `path` until the returned sender fires; the receiver reports
    /// that it was reached.
    fn pause_at(&self, target: SyncTarget, path: &Path) -> (mpsc::Receiver<()>, mpsc::Sender<()>) {
        let (reached, reached_rx) = mpsc::channel();
        let (resume_tx, resume) = mpsc::channel();
        *lock(&self.pause) = Some(Arc::new(Pause {
            target,
            path: path.to_owned(),
            reached,
            resume: Mutex::new(resume),
        }));
        (reached_rx, resume_tx)
    }

    fn take(&self) -> Vec<Synced> {
        std::mem::take(&mut *lock(&self.synced))
    }
}

struct Bed {
    dir: TempDir,
    root: PathBuf,
}

impl Bed {
    fn new() -> Result<Self> {
        let dir = TempDir::new()?;
        let root = dir.path().join("blobs");
        Ok(Self { dir, root })
    }

    fn store(&self, syncs: &Syncs) -> Result<FsBlobStore> {
        FsBlobStore::with_sync_hook(&self.root, Some(syncs.hook()))
    }

    fn hashes(&self) -> PathBuf {
        self.root.join("sha256")
    }

    fn staging(&self) -> PathBuf {
        self.root.join("staging")
    }

    fn staging_is_empty(&self) -> Result<bool> {
        Ok(std::fs::read_dir(self.staging())?.next().is_none())
    }
}

#[derive(Debug, Clone, Copy)]
enum Form {
    File,
    Memory,
}

fn staged(store: &FsBlobStore, form: Form, content: &[u8]) -> Result<Staged> {
    match form {
        Form::File => store.stage(&mut &content[..]),
        Form::Memory => Ok(Staged {
            hash: BlobHash::of(content),
            raw_bytes: content.len() as u64,
            stored_bytes: content.len() as u64,
            codec: "identity".to_owned(),
            data: StagedData::Memory(content.to_vec()),
        }),
    }
}

fn fan_out(store: &FsBlobStore, hash: &BlobHash) -> Result<PathBuf> {
    Ok(store
        .path_of(hash)
        .parent()
        .context("blob path has a parent")?
        .to_owned())
}

/// The syncs every publication of `dest` must end with: the hash directory (the fan-out's
/// entry), the content file and the fan-out directory (the file's entry).
fn ensure_published(bed: &Bed, synced: &[Synced], dest: &Path, first: bool) -> Result<()> {
    let fanout = dest.parent().context("blob path has a parent")?;
    let shape: Vec<(SyncTarget, bool)> = synced.iter().map(|(t, _, w)| (*t, *w)).collect();
    let file_ok = synced.get(1).is_some_and(|(_, path, _)| {
        if first {
            path.parent() == Some(bed.staging().as_path())
        } else {
            path == dest
        }
    });
    ensure!(
        synced.len() == 3
            && synced[0].1 == bed.hashes()
            && file_ok
            && synced[2].1 == fanout
            && shape == vec![(Dir, !first), (File, !first), (Dir, true)],
        "publication of {} (first: {first}) synced {synced:?}",
        dest.display()
    );
    Ok(())
}

#[test]
fn every_open_syncs_the_root_and_its_entry_even_when_they_exist() -> Result<()> {
    let bed = Bed::new()?;
    let syncs = Syncs::default();
    syncs.fail(Dir, bed.dir.path());
    ensure!(
        bed.store(&syncs).is_err(),
        "a failed sync of the root's entry fails the open"
    );
    ensure!(
        bed.hashes().is_dir() && bed.staging().is_dir(),
        "the failed open created the directories"
    );
    syncs.heal();
    syncs.take();
    bed.store(&syncs)?;
    ensure!(
        syncs.take()
            == vec![
                (Dir, bed.root.clone(), false),
                (Dir, bed.dir.path().to_owned(), false),
            ],
        "a reopen syncs the existing root and its entry"
    );

    let missing = bed.dir.path().join("missing").join("blobs");
    ensure!(
        FsBlobStore::at(&missing).is_err() && !bed.dir.path().join("missing").exists(),
        "a root whose parent is missing is refused without creating the parent"
    );
    Ok(())
}

#[test]
fn every_publication_syncs_its_file_fan_out_and_fan_out_entry_in_both_forms() -> Result<()> {
    for form in [Form::File, Form::Memory] {
        let bed = Bed::new()?;
        let syncs = Syncs::default();
        let store = bed.store(&syncs)?;
        let content = format!("{form:?} publication").into_bytes();
        let dest = store.path_of(&BlobHash::of(&content));
        syncs.watch(&dest);
        syncs.take();
        store.put_staged(staged(&store, form, &content)?)?;
        ensure_published(&bed, &syncs.take(), &dest, true)
            .with_context(|| format!("{form:?} form"))?;
        store.put_staged(staged(&store, form, &content)?)?;
        ensure_published(&bed, &syncs.take(), &dest, false)
            .with_context(|| format!("{form:?} form, duplicate"))?;
        ensure!(bed.staging_is_empty()?, "{form:?}: staging files left");
    }
    Ok(())
}

#[test]
fn a_retry_after_a_failed_fan_out_sync_syncs_it_again() -> Result<()> {
    for form in [Form::File, Form::Memory] {
        let bed = Bed::new()?;
        let syncs = Syncs::default();
        let store = bed.store(&syncs)?;
        let content = b"durability retry";
        let dest = store.path_of(&BlobHash::of(content));
        syncs.fail(Dir, &fan_out(&store, &BlobHash::of(content))?);
        ensure!(
            store.put_staged(staged(&store, form, content)?).is_err(),
            "{form:?}: a failed fan-out sync fails the publication"
        );
        ensure!(dest.is_file(), "{form:?}: the rename happened first");
        syncs.fail_every(true);
        ensure!(
            store.put_staged(staged(&store, form, content)?).is_err(),
            "{form:?}: a retry while every sync fails must not succeed"
        );
        syncs.heal();
        syncs.watch(&dest);
        syncs.take();
        store.put_staged(staged(&store, form, content)?)?;
        ensure_published(&bed, &syncs.take(), &dest, false)
            .with_context(|| format!("{form:?} form, retry"))?;
        ensure!(bed.staging_is_empty()?, "{form:?}: staging files left");
    }
    Ok(())
}

#[test]
fn a_retry_after_a_failed_hash_directory_sync_syncs_it_again() -> Result<()> {
    for form in [Form::File, Form::Memory] {
        let bed = Bed::new()?;
        let syncs = Syncs::default();
        let store = bed.store(&syncs)?;
        let content = b"durability retry";
        let dest = store.path_of(&BlobHash::of(content));
        syncs.fail(Dir, &bed.hashes());
        ensure!(
            store.put_staged(staged(&store, form, content)?).is_err(),
            "{form:?}: a failed hash-directory sync fails the publication"
        );
        ensure!(
            dest.parent().is_some_and(Path::is_dir) && !dest.exists(),
            "{form:?}: the fan-out exists, the content does not"
        );
        syncs.heal();
        syncs.watch(&dest);
        syncs.take();
        store.put_staged(staged(&store, form, content)?)?;
        ensure_published(&bed, &syncs.take(), &dest, true)
            .with_context(|| format!("{form:?} form, retry"))?;
    }
    Ok(())
}

/// Content other than `first` whose hash shares its fan-out directory.
fn same_fan_out(first: &[u8]) -> Result<Vec<u8>> {
    let prefix = BlobHash::of(first).as_str()[..2].to_owned();
    (0..100_000)
        .map(|i| format!("fan-out neighbour {i}").into_bytes())
        .find(|c| BlobHash::of(c).as_str().starts_with(&prefix))
        .context("no neighbour in the fan-out")
}

/// `a` publishes `first` on its own store and pauses at `pause`; meanwhile `b` publishes
/// `second` on another store over the same root, and must complete every sync itself.
fn while_paused(
    bed: &Bed,
    pause: impl Fn(&FsBlobStore) -> Result<(SyncTarget, PathBuf)>,
    first: &[u8],
    second: &[u8],
) -> Result<Vec<Synced>> {
    let (syncs_a, syncs_b) = (Syncs::default(), Syncs::default());
    let (store_a, store_b) = (bed.store(&syncs_a)?, bed.store(&syncs_b)?);
    let (target, path) = pause(&store_a)?;
    let (reached, resume) = syncs_a.pause_at(target, &path);
    let staged_a = store_a.stage(&mut &first[..])?;
    let staged_b = store_b.stage(&mut &second[..])?;
    syncs_b.watch(&store_b.path_of(&staged_b.hash));
    std::thread::scope(|scope| {
        let a = scope.spawn(|| store_a.put_staged(staged_a));
        let b = reached
            .recv_timeout(TIMEOUT)
            .map_err(|_| anyhow!("publisher a never reached its pause"))
            .and_then(|()| {
                syncs_b.take();
                store_b.put_staged(staged_b)?;
                Ok(syncs_b.take())
            });
        let _ = resume.send(());
        let a = a
            .join()
            .map_err(|_| anyhow!("publisher a panicked"))?
            .context("publisher a");
        let synced = b?;
        a?;
        Ok(synced)
    })
}

#[test]
fn a_publisher_finding_another_ones_unsynced_fan_out_syncs_its_entry() -> Result<()> {
    let bed = Bed::new()?;
    let first = b"first in this fan-out".to_vec();
    let second = same_fan_out(&first)?;
    let hashes = bed.hashes();
    let synced = while_paused(&bed, |_| Ok((Dir, hashes.clone())), &first, &second)?;
    let dest = FsBlobStore::at(&bed.root)?.path_of(&BlobHash::of(&second));
    ensure_published(&bed, &synced, &dest, true)
}

#[test]
fn a_publisher_finding_another_ones_unsynced_file_syncs_it_and_its_entry() -> Result<()> {
    let bed = Bed::new()?;
    let content = b"one log, two publishers".to_vec();
    let hash = BlobHash::of(&content);
    let fanout = |store: &FsBlobStore| Ok((Dir, fan_out(store, &hash)?));
    let synced = while_paused(&bed, fanout, &content, &content)?;
    let dest = FsBlobStore::at(&bed.root)?.path_of(&hash);
    ensure_published(&bed, &synced, &dest, false)
}

struct Board {
    backend: FakeBackend,
    clock: ManualClock,
}

impl Board {
    fn new() -> Self {
        let clock = ManualClock::new(Timestamp::now());
        let backend = FakeBackend::new(
            BackendConfig::default(),
            Arc::new(clock.clone()),
            Arc::new(FaultInjector::default()),
        );
        Self { backend, clock }
    }

    fn putter<'a>(&'a self, store: &'a FsBlobStore) -> Result<Putter<'a>> {
        Putter::new(&self.backend, store, &self.clock)
    }

    fn reference(
        &self,
        key: &str,
        hash: &BlobHash,
        reservations: Vec<ReservationUse>,
    ) -> Result<Committed, ApplyError> {
        self.backend.apply(Operation::PutRecords(PutRecords {
            writes: vec![RecordWrite {
                area: "logs".to_owned(),
                key: key.to_owned(),
                expected: None,
                doc: Some(json!({"log": hash.to_ref()})),
                updated: self.clock.now(),
            }],
            events: Vec::new(),
            reservations,
        }))
    }

    fn row(&self, hash: &BlobHash) -> Result<BlobRow> {
        let q = BlobQuery {
            hash: Some(hash.clone()),
            state: None,
        };
        let view = self.backend.view()?;
        view.blobs(&q, PageReq::first(1))?
            .items
            .into_iter()
            .next()
            .context("blob row")
    }
}

fn reserved(put: TryPut) -> Result<PutBlob> {
    match put {
        TryPut::Reserved(blob) => Ok(blob),
        TryPut::Busy(_) => bail!("unexpectedly busy"),
    }
}

#[test]
fn a_put_whose_sync_fails_hands_out_no_reservation_to_reference() -> Result<()> {
    let bed = Bed::new()?;
    let board = Board::new();
    let syncs = Syncs::default();
    let store = bed.store(&syncs)?;
    let content = b"evidence that must survive a crash";
    let hash = BlobHash::of(content);
    let dest = store.path_of(&hash);
    syncs.fail(Dir, &fan_out(&store, &hash)?);
    let failed = board
        .putter(&store)?
        .put(&mut &content[..], &mut |_| bail!("unexpectedly busy"));
    ensure!(failed.is_err(), "the put reports the failed sync");
    let row = board.row(&hash)?;
    ensure!(
        row.state == BlobState::Writing && row.refs == 0 && row.reservations.len() == 1,
        "nothing references the unsynced blob: {row:?}"
    );
    ensure!(
        matches!(
            board.reference("unsynced", &hash, Vec::new()),
            Err(ApplyError::Precondition(_))
        ),
        "an operation cannot reference it without a reservation"
    );

    syncs.heal();
    syncs.watch(&dest);
    syncs.take();
    let blob = board
        .putter(&store)?
        .put(&mut &content[..], &mut |_| bail!("unexpectedly busy"))?;
    ensure!(!blob.created, "the retry found the row");
    ensure_published(&bed, &syncs.take(), &dest, false)?;
    board.reference("synced", &hash, vec![blob.reservation])?;
    let row = board.row(&hash)?;
    ensure!(
        row.state == BlobState::Live && row.refs == 1,
        "live {row:?}"
    );
    Ok(())
}

#[test]
fn a_put_that_finds_another_putters_unsynced_file_syncs_it_before_its_reservation() -> Result<()> {
    let bed = Bed::new()?;
    let board = Board::new();
    let (syncs_a, syncs_b) = (Syncs::default(), Syncs::default());
    let (store_a, store_b) = (bed.store(&syncs_a)?, bed.store(&syncs_b)?);
    let content = b"one log, two putters".to_vec();
    let hash = BlobHash::of(&content);
    let dest = store_a.path_of(&hash);
    let (reached, resume) = syncs_a.pause_at(Dir, &fan_out(&store_a, &hash)?);
    let (putter_a, putter_b) = (board.putter(&store_a)?, board.putter(&store_b)?);
    let staged_a = store_a.stage(&mut &content[..])?;
    let staged_b = store_b.stage(&mut &content[..])?;
    syncs_b.watch(&dest);
    let (a, b) = std::thread::scope(|scope| {
        let a = scope.spawn(|| putter_a.try_put(staged_a).and_then(reserved));
        let b = reached
            .recv_timeout(TIMEOUT)
            .map_err(|_| anyhow!("putter a never reached its fan-out sync"))
            .and_then(|()| {
                ensure!(dest.is_file(), "putter a renamed before pausing");
                syncs_b.take();
                let blob = reserved(putter_b.try_put(staged_b)?)?;
                let synced = syncs_b.take();
                board.reference("b", &hash, vec![blob.reservation.clone()])?;
                Ok((blob, synced))
            });
        let _ = resume.send(());
        let a = a.join().map_err(|_| anyhow!("putter a panicked"));
        (a, b)
    });
    let (blob_b, synced_b) = b?;
    let blob_a = a??;
    ensure!(
        blob_a.created && !blob_b.created,
        "a created the row, b found it"
    );
    ensure_published(&bed, &synced_b, &dest, false)
        .context("b's own syncs before its reservation was handed out")?;
    board.reference("a", &hash, vec![blob_a.reservation])?;
    let row = board.row(&hash)?;
    ensure!(
        row.state == BlobState::Live && row.refs == 2,
        "live {row:?}"
    );
    Ok(())
}
