//! Wall-clock timestamps that never run backwards within one process.
//!
//! Attempt windows compare persisted wall-clock stamps (`run.started`, `gate.at`,
//! `artifact.added`, `observation.captured`). A host clock step backwards (observed up to
//! ~1.9 s on WSL2) could make a stamp taken later in the same process sort before an
//! earlier one, refusing fresh evidence as out-of-window. Stamping through [`now`] keeps
//! in-process ordering intact: a reading never precedes one already issued, so time holds
//! at the last issued value until the wall clock catches up. Real elapsed time is never
//! invented, so a capture genuinely older than an attempt still sorts before it.
//! Stamps written by other processes keep plain wall-clock semantics.

use std::sync::{LazyLock, Mutex, PoisonError};

use jiff::Timestamp;

type WallSource = Box<dyn Fn() -> Timestamp + Send + Sync>;

/// Non-decreasing view of a wall-clock source. The source is injectable for tests.
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

    /// The later of the wall reading and the last value this clock issued.
    pub fn now(&self) -> Timestamp {
        let wall = (self.wall)();
        let mut last = self.last.lock().unwrap_or_else(PoisonError::into_inner);
        let issued = last.map_or(wall, |previous| previous.max(wall));
        *last = Some(issued);
        issued
    }
}

static PROCESS: LazyLock<MonotonicClock> = LazyLock::new(|| MonotonicClock::new(Timestamp::now));

/// Process-wide non-decreasing wall clock for attempt-window stamps.
pub fn now() -> Timestamp {
    PROCESS.now()
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
        // When stamping again, the later stamp holds at the issued value.
        assert_eq!(clock.now(), first);
        // Then once the wall clock passes the issued value it is followed again.
        wall.fetch_add(603_819_663 + 5, Ordering::SeqCst);
        assert_eq!(clock.now().as_nanosecond() - first.as_nanosecond(), 5);
    }
}
