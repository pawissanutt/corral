//! Per-thread invocation budgets and independent process-wide subprocess wall time.

use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

thread_local! {
    static INVOCATIONS: Cell<u64> = const { Cell::new(0) };
}

static CALLS: AtomicU64 = AtomicU64::new(0);
static NANOS: AtomicU64 = AtomicU64::new(0);

/// Records one herdr invocation on the current thread.
pub fn record() {
    INVOCATIONS.with(|count| count.set(count.get().saturating_add(1)));
}

/// The current thread's running total of herdr invocations.
pub fn invocations() -> u64 {
    INVOCATIONS.with(Cell::get)
}

/// Times a subprocess without changing the thread-local work-budget counter.
pub fn timed() -> Timer {
    Timer(Instant::now())
}

pub struct Timer(Instant);

impl Drop for Timer {
    fn drop(&mut self) {
        CALLS.fetch_add(1, Ordering::Relaxed);
        NANOS.fetch_add(
            u64::try_from(self.0.elapsed().as_nanos()).unwrap_or(u64::MAX),
            Ordering::Relaxed,
        );
    }
}

/// Process-wide completed subprocess attempts: (calls, nanoseconds).
pub fn totals() -> (u64, u64) {
    (CALLS.load(Ordering::Relaxed), NANOS.load(Ordering::Relaxed))
}
