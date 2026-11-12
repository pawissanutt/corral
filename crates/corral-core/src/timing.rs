//! Process-wide wall-time counters. Recording never performs filesystem I/O.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

static STORE_DIR: OnceLock<PathBuf> = OnceLock::new();
static SPANS: Mutex<BTreeMap<&'static str, (u64, u64)>> = Mutex::new(BTreeMap::new());

/// Remembers the first resolved store, without reopening it at process exit.
pub fn register_store(dir: &Path) {
    STORE_DIR.get_or_init(|| dir.to_path_buf());
}

pub fn store_dir() -> Option<&'static Path> {
    STORE_DIR.get().map(PathBuf::as_path)
}

#[derive(Default)]
pub struct Counter {
    count: AtomicU64,
    nanos: AtomicU64,
}

impl Counter {
    pub const fn new() -> Self {
        Self {
            count: AtomicU64::new(0),
            nanos: AtomicU64::new(0),
        }
    }

    pub fn start(&'static self) -> Timer {
        Timer {
            counter: self,
            started: Instant::now(),
        }
    }

    pub fn total(&self) -> Total {
        Total {
            count: self.count.load(Ordering::Relaxed),
            nanos: self.nanos.load(Ordering::Relaxed),
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Total {
    pub count: u64,
    pub nanos: u64,
}

pub struct Timer {
    counter: &'static Counter,
    started: Instant,
}

impl Drop for Timer {
    fn drop(&mut self) {
        self.counter.count.fetch_add(1, Ordering::Relaxed);
        self.counter
            .nanos
            .fetch_add(nanos(self.started), Ordering::Relaxed);
    }
}

fn nanos(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX)
}

pub static LOCK_WAIT: Counter = Counter::new();
pub static SQLITE_BUSY: Counter = Counter::new();
pub static SQLITE_APPLY: Counter = Counter::new();
static LOCK_TRY_FAILED: AtomicU64 = AtomicU64::new(0);

/// Nonblocking acquisitions count as attempts, but contribute no wait time.
pub fn record_lock_try(failed: bool) {
    LOCK_WAIT.count.fetch_add(1, Ordering::Relaxed);
    if failed {
        LOCK_TRY_FAILED.fetch_add(1, Ordering::Relaxed);
    }
}

pub fn lock_try_failed() -> u64 {
    LOCK_TRY_FAILED.load(Ordering::Relaxed)
}

/// A coarse, statically named wall-time span; nested spans accumulate independently.
pub fn span(name: &'static str) -> Span {
    Span {
        name,
        started: Instant::now(),
    }
}

pub struct Span {
    name: &'static str,
    started: Instant,
}

impl Drop for Span {
    fn drop(&mut self) {
        if let Ok(mut spans) = SPANS.lock() {
            let (count, elapsed) = spans.entry(self.name).or_default();
            *count = count.saturating_add(1);
            *elapsed = elapsed.saturating_add(nanos(self.started));
        }
    }
}

pub fn spans() -> BTreeMap<&'static str, (u64, u64)> {
    SPANS.lock().map(|spans| spans.clone()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    #[test]
    fn blocking_store_lock_records_wait_time() {
        let dir = tempfile::tempdir().unwrap();
        let store = crate::Store::init(dir.path()).unwrap();
        let held = store.lock("timing").unwrap();
        let before = super::LOCK_WAIT.total();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let contender = store.clone();
        let waiter = std::thread::spawn(move || {
            assert!(contender.try_lock("timing").unwrap().is_none());
            ready_tx.send(()).unwrap();
            contender.lock("timing").unwrap()
        });
        ready_rx.recv().unwrap();
        drop(held);
        drop(waiter.join().unwrap());
        let after = super::LOCK_WAIT.total();
        assert!(after.count > before.count);
        assert!(after.nanos > before.nanos);
    }
}
