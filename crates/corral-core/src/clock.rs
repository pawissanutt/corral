//! Wall-clock timestamps that strictly increase within one process.
//!
//! Attempt windows compare persisted wall-clock stamps (`run.started`, `gate.at`,
//! `artifact.added`, `observation.captured`). A host clock step backwards (observed up to
//! ~1.9 s on WSL2) could make a stamp taken later in the same process sort before an
//! earlier one, refusing fresh evidence as out-of-window. Stamping through [`now`] keeps
//! in-process ordering strict: each reading is at least one nanosecond after the last one
//! issued, so during a backward step time advances by 1 ns per stamp until the wall clock
//! catches up. Stamps never tie, so a capture issued before an attempt or gate boundary
//! always sorts strictly before it and is still refused.
//! Stamps written by other processes keep plain wall-clock semantics.

use std::sync::{LazyLock, Mutex, PoisonError};

use jiff::{SignedDuration, Timestamp};

type WallSource = Box<dyn Fn() -> Timestamp + Send + Sync>;

/// Strictly increasing view of a wall-clock source. The source is injectable for tests.
pub struct MonotonicClock {
    wall: WallSource,
    last: Mutex<Option<Timestamp>>,
}

impl MonotonicClock {
    pub fn new(wall: impl Fn() -> Timestamp + Send + Sync + 'static) -> Self {
        Self {
            wall: Box::new(wall),
            last: Mutex::new(None),
        }
    }

    /// The later of the wall reading and one nanosecond after the last value issued.
    pub fn now(&self) -> Timestamp {
        let wall = (self.wall)();
        let mut last = self.last.lock().unwrap_or_else(PoisonError::into_inner);
        let issued = last.map_or(wall, |previous| {
            previous
                .checked_add(SignedDuration::from_nanos(1))
                .unwrap_or(previous)
                .max(wall)
        });
        *last = Some(issued);
        issued
    }
}

static PROCESS: LazyLock<MonotonicClock> = LazyLock::new(|| MonotonicClock::new(Timestamp::now));

thread_local! {
    static PINNED: std::cell::Cell<Option<Timestamp>> = const { std::cell::Cell::new(None) };
}

/// Pins every stamp this thread takes through [`now`] and `Store::now` to `at`, or unpins with
/// `None`: a budget fixture's records then serialize with one width at every size.
#[doc(hidden)]
pub fn pin_thread(at: Option<Timestamp>) {
    PINNED.set(at);
}

pub(crate) fn pinned() -> Option<Timestamp> {
    PINNED.get()
}

/// Whether this thread's stamps come from a pinned or test clock rather than the process clock.
/// Another process (the daemon) cannot share that clock, so such callers execute locally.
pub fn is_pinned() -> bool {
    if pinned().is_some() {
        return true;
    }
    #[cfg(feature = "test-clock")]
    if TEST_CLOCK.with(|clock| clock.borrow().is_some()) {
        return true;
    }
    false
}

/// Pins a CLI fixture's main-thread clock across subprocesses. The environment input is
/// compiled only with `test-clock`, which production dependencies never enable.
#[doc(hidden)]
pub fn pin_from_test_env() -> anyhow::Result<()> {
    #[cfg(feature = "test-clock")]
    {
        use anyhow::Context;
        if let Some(raw) = std::env::var_os("CORRAL_TEST_NOW") {
            let at = raw
                .to_str()
                .context("CORRAL_TEST_NOW must be UTF-8")?
                .parse()
                .context("CORRAL_TEST_NOW must be a timestamp")?;
            pin_thread(Some(at));
        }
    }
    Ok(())
}

/// Process-wide strictly increasing wall clock for attempt-window stamps.
pub fn now() -> Timestamp {
    if let Some(stamp) = pinned() {
        return stamp;
    }
    #[cfg(feature = "test-clock")]
    if let Some(stamp) = TEST_CLOCK.with(|clock| clock.borrow().as_ref().map(MonotonicClock::now)) {
        return stamp;
    }
    PROCESS.now()
}

/// Installs a thread-local clock for a test, restoring the previous clock even on panic.
/// Other threads and production builds continue to use the process clock.
#[cfg(feature = "test-clock")]
pub fn with_test_wall<T>(
    wall: impl Fn() -> Timestamp + Send + Sync + 'static,
    f: impl FnOnce() -> T,
) -> T {
    struct Restore(Option<MonotonicClock>);
    impl Drop for Restore {
        fn drop(&mut self) {
            TEST_CLOCK.with(|clock| *clock.borrow_mut() = self.0.take());
        }
    }
    let previous = TEST_CLOCK.with(|clock| clock.borrow_mut().replace(MonotonicClock::new(wall)));
    let _restore = Restore(previous);
    f()
}

#[cfg(feature = "test-clock")]
thread_local! {
    static TEST_CLOCK: std::cell::RefCell<Option<MonotonicClock>> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicI64, Ordering};

    use super::*;

    fn stepped(start_ns: i64) -> (Arc<AtomicI64>, MonotonicClock) {
        let wall = Arc::new(AtomicI64::new(start_ns));
        let source = Arc::clone(&wall);
        let clock = MonotonicClock::new(move || {
            Timestamp::from_nanosecond(source.load(Ordering::SeqCst).into()).unwrap()
        });
        (wall, clock)
    }

    #[test]
    fn backward_wall_step_never_reorders_issued_stamps() {
        // Given a stamp issued, then the wall clock stepping back 604 ms.
        let (wall, clock) = stepped(1_700_000_000_000_000_000);
        let first = clock.now();
        wall.fetch_sub(603_819_663, Ordering::SeqCst);
        // When stamping twice more, each stamp advances one nanosecond past the last.
        let second = clock.now();
        let third = clock.now();
        assert_eq!(second.as_nanosecond() - first.as_nanosecond(), 1);
        assert_eq!(third.as_nanosecond() - second.as_nanosecond(), 1);
        // Then once the wall clock passes the issued value it is followed again.
        wall.fetch_add(603_819_663 + 5, Ordering::SeqCst);
        assert_eq!(clock.now().as_nanosecond() - first.as_nanosecond(), 5);
    }
}
