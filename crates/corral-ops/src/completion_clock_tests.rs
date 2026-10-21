use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use corral_core::clock::MonotonicClock;
use corral_core::{GateRecord, GateResult, Run};
use jiff::Timestamp;

use super::{check_capture_window, predates_attempt};

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

fn run_started(started: Timestamp) -> Run {
    serde_json::from_value(serde_json::json!({
        "id": "R1", "agent": "generic", "worktree": "/tmp/unused",
        "branch": "main", "base": "main", "base_sha": "0", "started": started,
    }))
    .unwrap()
}

fn with_gate(mut run: Run, at: Timestamp) -> Run {
    run.gate = Some(GateRecord {
        identity: None,
        commands: Vec::new(),
        log_sha256: None,
        result: GateResult::Pass,
        process_lost: false,
        failed_cmd: None,
        at,
        sha: "0".into(),
        pane: None,
        socket: None,
    });
    run
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
    // Given a run and gate stamped, then the wall clock stepping back 604 ms in-process.
    let wall = SteppedWall::new();
    let run = run_started(wall.clock.now());
    let run = with_gate(run, wall.clock.now());
    wall.step(-STEP_NS);
    // Raw wall readings would order the later capture before the gate (the T-178 failure).
    assert_eq!(
        refusal(check_capture_window(wall.raw(), &run, wall.raw())),
        "capture_outside_attempt"
    );
    // When the capture and the recording check are stamped through the process clock.
    let captured = wall.clock.now();
    let now = wall.clock.now();
    // Then the capture and an artifact added with it are accepted.
    check_capture_window(captured, &run, now).unwrap();
    assert!(!predates_attempt(captured, &run));
}

#[test]
fn stamp_before_a_boundary_issued_during_a_backward_step_is_refused() {
    // Given a capture, a backward step, then a new attempt and gate stamped before the
    // wall clock catches up (the review's rollback-plateau counterexample).
    let wall = SteppedWall::new();
    let old = wall.clock.now();
    wall.step(-STEP_NS);
    let no_gate = run_started(wall.clock.now());
    wall.step(1_000);
    let gated = with_gate(no_gate.clone(), wall.clock.now());
    let now = wall.clock.now();
    for run in [&no_gate, &gated] {
        // When checking the older capture and an artifact added at the same instant.
        let capture = check_capture_window(old, run, now);
        // Then both are refused against the run-start and gate boundaries alike.
        assert_eq!(refusal(capture), "capture_outside_attempt");
        assert!(predates_attempt(old, run));
    }
    // And a capture issued after the gate on the same plateau is still accepted.
    let fresh = wall.clock.now();
    check_capture_window(fresh, &gated, wall.clock.now()).unwrap();
}
