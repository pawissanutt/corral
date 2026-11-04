//! Shared support for the SQLite backend tests: a conformance [`Fixture`] over
//! [`SqliteBackend`], the conformance suite's own operation samples, and a child-process role
//! dispatcher so tests can run a second process (or kill one) deterministically.
//!
//! `samples` is the conformance suite's `samples.rs` compiled into this test crate, so the crash
//! tests build every operation exactly as the conformance cases do. It resolves `super::Fixture`
//! here and `crate::backend` / `crate::blob_ref` through the re-export modules each test crate
//! root declares.

#![allow(dead_code)]

pub mod crash;

use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
pub use corral_core::backend::conformance::Fixture;
use corral_core::backend::conformance::epoch;
use corral_core::backend::fake::FakeBlobStore;
use corral_core::backend::*;
use corral_core::blob_ref::BlobHash;
use corral_core::sqlite::{Barrier, SqliteBackend, SqliteOptions, statfs_probe};
use tempfile::TempDir;

#[path = "../../src/backend/conformance/samples.rs"]
pub mod samples;

pub fn base_task(fx: &Fixture) -> Result<TaskNum> {
    samples::base_task(fx)
}

pub fn change(fx: &Fixture, num: TaskNum) -> Result<TaskChange> {
    samples::change(fx, num)
}

pub fn event(fx: &Fixture, task: Option<&str>, kind: &str, detail: serde_json::Value) -> NewEvent {
    samples::event(fx, task, kind, detail)
}

pub fn row(
    part: TaskPart,
    key: &str,
    current: bool,
    open: bool,
    doc: serde_json::Value,
) -> HistoryRow {
    samples::row(part, key, current, open, doc)
}

pub fn record(
    area: &str,
    key: &str,
    expected: Option<u64>,
    doc: Option<serde_json::Value>,
    at: jiff::Timestamp,
) -> RecordWrite {
    samples::record(area, key, expected, doc, at)
}

pub fn gate_attempt(fx: &Fixture, identity: &BlobHash, commands: &BlobHash) -> GateAttempt {
    samples::gate_attempt(fx, identity, commands)
}

/// Environment variables of a child process started by [`child_command`].
pub const ROLE_ENV: &str = "CORRAL_SQLITE_TEST_ROLE";
pub const DIR_ENV: &str = "CORRAL_SQLITE_TEST_DIR";
/// Prefix of every line a child writes for its parent.
pub const MARK: &str = "CORRAL-CHILD ";

pub fn options(
    clock: &ManualClock,
    faults: &Arc<FaultInjector>,
    barrier: Option<Barrier>,
) -> SqliteOptions {
    SqliteOptions {
        config: BackendConfig::default(),
        clock: Arc::new(clock.clone()),
        faults: Arc::clone(faults),
        filesystem: statfs_probe,
        barrier,
        fixture_no_sync: false,
    }
}

struct SharedBlobs(Arc<FakeBlobStore>);

impl BlobStore for SharedBlobs {
    fn stage(&self, src: &mut dyn Read) -> Result<Staged> {
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

/// A conformance fixture over a SQLite database in `<corral_dir>/db/corral.db`, starting at the
/// conformance epoch. `keep` stays alive as long as the fixture (its temporary directory).
/// Blob content uses the in-memory blob store until blob-store ships the filesystem one.
pub fn fixture_at(
    corral_dir: &Path,
    keep: Option<Arc<TempDir>>,
    barrier: Option<Barrier>,
) -> Result<Fixture> {
    let clock = ManualClock::new(epoch());
    let faults = Arc::new(FaultInjector::default());
    let backend = SqliteBackend::open(corral_dir, options(&clock, &faults, barrier))?;
    let second = backend.handle()?;
    let pruner = backend.handle()?;
    let blobs = Arc::new(FakeBlobStore::new(Arc::new(clock.clone())));
    let corrupter = Arc::clone(&blobs);
    Ok(Fixture {
        backend: Box::new(backend),
        second: Box::new(second),
        blobs: Box::new(SharedBlobs(blobs)),
        clock,
        faults,
        config: BackendConfig::default(),
        prune_changes_through: Box::new(move |seq| {
            let _keep = &keep;
            pruner.prune_changes_through(seq)
        }),
        corrupt_blob: Box::new(move |hash, bytes| {
            corrupter.corrupt(hash, bytes);
            Ok(())
        }),
    })
}

/// A fresh fixture in its own temporary directory.
pub fn sqlite_fixture() -> Fixture {
    let made = TempDir::new()
        .context("create a temporary directory")
        .and_then(|dir| {
            let dir = Arc::new(dir);
            let corral = dir.path().join(".corral");
            fixture_at(&corral, Some(dir), None)
        });
    match made {
        Ok(fixture) => fixture,
        Err(error) => panic!("open a SQLite conformance fixture: {error:#}"),
    }
}

/// The current test binary re-run as a child that executes only `entry` (a test that dispatches
/// on [`ROLE_ENV`]) with `role` in `dir`.
pub fn child_command(entry: &str, role: &str, dir: &Path) -> Result<std::process::Command> {
    let exe = std::env::current_exe().context("locate the test binary")?;
    let mut command = std::process::Command::new(exe);
    command
        .args([entry, "--exact", "--nocapture", "--test-threads", "1"])
        .env(ROLE_ENV, role)
        .env(DIR_ENV, dir);
    Ok(command)
}

/// The role and directory of this process when a parent test started it as a child.
pub fn child_role() -> Option<(String, PathBuf)> {
    let role = std::env::var(ROLE_ENV).ok()?;
    let dir = std::env::var_os(DIR_ENV)?;
    Some((role, PathBuf::from(dir)))
}

/// Writes one marked line for the parent and flushes it.
pub fn tell(line: &str) -> Result<()> {
    use std::io::Write;
    let mut out = std::io::stdout().lock();
    writeln!(out, "{MARK}{line}")?;
    out.flush()?;
    Ok(())
}

/// Blocks until the parent writes a line (or closes stdin); returns it trimmed.
pub fn hear() -> Result<String> {
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    Ok(line.trim().to_owned())
}

/// The marked part of a child's output line; libtest may print its own text before it.
pub fn marked(line: &str) -> Option<&str> {
    line.split_once(MARK).map(|(_, rest)| rest)
}

/// How long a test waits for one report from a child or a signal from another thread before
/// it fails instead of hanging.
pub const CHILD_TIMEOUT: Duration = Duration::from_secs(60);

/// A child's stdout read on its own thread, so each wait for a report is bounded.
pub struct ChildLines(mpsc::Receiver<std::io::Result<String>>);

impl ChildLines {
    pub fn new(stdout: impl Read + Send + 'static) -> Self {
        let (lines, received) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                if lines.send(line).is_err() {
                    break;
                }
            }
        });
        Self(received)
    }

    /// The next marked report starting with `want`, within [`CHILD_TIMEOUT`].
    pub fn wait_for(&self, want: &str) -> Result<String> {
        let deadline = Instant::now() + CHILD_TIMEOUT;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.0.recv_timeout(left) {
                Ok(line) => {
                    let line = line?;
                    if let Some(rest) = marked(&line)
                        && rest.starts_with(want)
                    {
                        return Ok(rest.to_owned());
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    anyhow::bail!("no {want:?} report from the child within {CHILD_TIMEOUT:?}")
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    anyhow::bail!("the child exited before reporting {want:?}")
                }
            }
        }
    }
}
