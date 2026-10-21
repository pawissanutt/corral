use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use corral_core::clock::MonotonicClock;
use jiff::Timestamp;

use super::check_capture_window;

const STEP_NS: i64 = 603_819_663;

struct SteppedWall {
    ns: Arc<AtomicI64>,
    clock: MonotonicClock,
}

impl SteppedWall {
    fn new() -> Self {
        let ns = Arc::new(AtomicI64::new(1_790_000_000_000_000_000));
        let source = Arc::clone(&ns);
        let clock = MonotonicClock::new(move || {
            Timestamp::from_nanosecond(source.load(Ordering::SeqCst).into()).unwrap()
        });
        Self { ns, clock }
    }

    fn step(&self, delta_ns: i64) {
        self.ns.fetch_add(delta_ns, Ordering::SeqCst);
    }

    fn raw(&self) -> Timestamp {
        Timestamp::from_nanosecond(self.ns.load(Ordering::SeqCst).into()).unwrap()
    }
}

fn refusal(result: anyhow::Result<()>) -> String {
    result
        .unwrap_err()
        .to_string()
        .split(':')
        .next()
        .unwrap()
        .to_owned()
}

#[test]
fn capture_after_gate_survives_backward_wall_step() {
    // Given a gate stamped, then the wall clock stepping back 604 ms in the same process.
    let wall = SteppedWall::new();
    let gate_at = wall.clock.now();
    wall.step(-STEP_NS);
    // Raw wall readings would order the later capture before the gate (the T-178 failure).
    assert_eq!(
        refusal(check_capture_window(wall.raw(), gate_at, wall.raw())),
        "capture_outside_attempt"
    );
    // When the capture and the recording check are stamped through the process clock.
    let captured = wall.clock.now();
    wall.step(1_000);
    let now = wall.clock.now();
    // Then the capture is accepted.
    check_capture_window(captured, gate_at, now).unwrap();
}

#[test]
fn capture_older_than_attempt_is_still_refused() {
    // Given a capture taken one second before the gate, and a later backward wall step.
    let wall = SteppedWall::new();
    let captured = wall.clock.now();
    wall.step(1_000_000_000);
    let gate_at = wall.clock.now();
    wall.step(-STEP_NS);
    let now = wall.clock.now();
    // When checking the capture window.
    let result = check_capture_window(captured, gate_at, now);
    // Then the stale capture is refused.
    assert_eq!(refusal(result), "capture_outside_attempt");
}
