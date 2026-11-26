use std::path::PathBuf;

use serde_json::{Value, json};

use super::*;
use crate::events::NewEvent;

struct Fixture {
    root: tempfile::TempDir,
    engine: Engine,
}

impl Fixture {
    fn new(role: Role, status: Status, pi: bool) -> (Self, Run) {
        let root = crate::tests::tempdir();
        let mut engine = Engine::init(root.path(), None).unwrap();
        engine.config.sync.quiet_after_secs = 120;
        let mut task = crate::tests::task("T-001");
        task.status = status;
        engine.store.insert_task(&task).unwrap();
        let mut run = crate::tests::run(&task.id, "R1");
        run.role = role;
        let inner = SessionRef::Herdr {
            workspace: "fake".into(),
            pane: "fake".into(),
        };
        run.session = Some(if pi {
            SessionRef::Pi {
                inner: Box::new(inner),
                socket: PathBuf::new(),
                dir: PathBuf::new(),
            }
        } else {
            inner
        });
        engine.store.insert_run(&run).unwrap();
        (Self { root, engine }, run)
    }

    fn event(&self, run: &Run, kind: EventKind, detail: Value) {
        let store = &self.engine.store;
        store
            .transaction(|store| {
                store.append_event(&NewEvent {
                    task: Some(run.task.clone()),
                    run: Some(run.id.clone()),
                    kind,
                    actor: Actor::Auto,
                    detail,
                })
            })
            .unwrap();
    }

    fn reason(&self, run: &Run) -> Option<String> {
        self.engine.store.require_task(&run.task).unwrap().attention
    }

    fn sample(&self, run: &Run, secs: i64, sample: Observation) {
        let seq = self.engine.store.task_event_seq(&run.task).unwrap();
        self.engine
            .reconcile_sample(run, seq, sample, Timestamp::from_second(secs).unwrap())
            .unwrap();
    }
}

fn idle() -> Observation {
    Observation {
        status: Some(SessionStatus::Idle),
        paused: None,
        activity: None,
    }
}

fn pi(settled: bool, busy: bool, waiting: bool, inputs: Value) -> Observation {
    Observation {
        paused: Some(false),
        activity: Some(
            serde_json::from_value(
                json!({ "settled": settled, "busy": busy, "waiting": waiting, "turn": 1, "inputs": inputs }),
            )
            .unwrap(),
        ),
        ..idle()
    }
}

#[test]
fn feedback_pickup_is_distinct_from_an_applied_turn_that_owes_submission() {
    let (mut f, run) = Fixture::new(Role::Worker, Status::Running, true);
    f.event(
        &run,
        EventKind::SentBack,
        json!({"pickup": {"input_id": "board-current", "rejected": false, "before_turn": 1}}),
    );
    // Even if the previous turn had settled, neither its outcome nor a different
    // input's completion can fulfill the new obligation.
    f.sample(
        &run,
        100,
        pi(
            true,
            false,
            false,
            json!({"board-old": {"state": "applied"}}),
        ),
    );
    let pending = |quiet_ms| {
        pi(
            true,
            false,
            false,
            json!({"board-current": {"state": "pending", "quiet_ms": quiet_ms}}),
        )
    };
    f.sample(&run, 129, pending(0));
    assert_eq!(f.reason(&run), None);
    // The engine's older quiet clock is not evidence that this input was pending.
    f.sample(&run, 130, pending(999));
    assert_eq!(f.reason(&run), None);
    f.sample(&run, 160, pending(30_000));
    let queued = f.reason(&run).unwrap();
    assert!(queued.starts_with("Send-back feedback wasn’t picked up"));
    let watcher = Engine::open(f.root.path()).unwrap();
    let version = watcher.store.data_version().unwrap();
    // No event stream is replayed. The persisted obligation and a fresh snapshot suffice.
    f.engine = Engine::open(f.root.path()).unwrap();
    f.sample(&run, 161, pending(31_000));
    assert_eq!(f.reason(&run).as_deref(), Some(queued.as_str()));
    assert_eq!(watcher.store.data_version().unwrap(), version);

    f.sample(
        &run,
        162,
        pi(
            false,
            true,
            false,
            json!({"board-current": {"state": "applied"}}),
        ),
    );
    assert_eq!(f.reason(&run), None);
    f.sample(
        &run,
        163,
        pi(
            true,
            false,
            false,
            json!({"board-current": {"state": "applied"}}),
        ),
    );
    assert!(
        f.reason(&run)
            .unwrap()
            .starts_with("Worker settled without submitting")
    );
    // A Pi-looking pane without a native observation is unknown, not idle.
    f.sample(&run, 200, idle());
    assert_eq!(f.reason(&run), None);
    let failed = || {
        pi(
            true,
            false,
            false,
            json!({"board-current": {"state": "failed", "turn": 1}}),
        )
    };
    f.sample(&run, 201, failed());
    assert_eq!(f.reason(&run).as_deref(), Some(queued.as_str()));
    let mut resumed = failed();
    resumed.activity.as_mut().unwrap().turn = 2;
    f.sample(&run, 202, resumed);
    assert!(
        f.reason(&run)
            .unwrap()
            .starts_with("Worker settled without submitting")
    );
    assert_eq!(
        f.engine.store.require_task(&run.task).unwrap().status,
        Status::Running
    );
}

#[test]
fn only_current_unfulfilled_work_gets_native_attention() {
    let (f, run) = Fixture::new(Role::Reviewer, Status::Review, true);
    f.sample(&run, 100, pi(true, false, false, json!({})));
    assert_eq!(f.reason(&run), None);
    let stale = f.engine.store.task_event_seq(&run.task).unwrap();
    f.event(
        &run,
        EventKind::ReviewStarted,
        json!({"round": 0, "pickup": null}),
    );
    f.engine
        .reconcile_sample(
            &run,
            stale,
            pi(true, false, false, json!({})),
            Timestamp::from_second(101).unwrap(),
        )
        .unwrap();
    assert_eq!(f.reason(&run), None);

    for sample in [
        pi(false, true, false, json!({})), // transient agent_end
        pi(true, false, true, json!({})),  // explicit wait
        Observation {
            paused: Some(true),
            ..pi(true, false, false, json!({}))
        },
    ] {
        f.sample(&run, 102, sample);
        assert_eq!(f.reason(&run), None);
    }
    f.sample(&run, 103, pi(true, false, false, json!({})));
    assert!(
        f.reason(&run)
            .unwrap()
            .starts_with("Reviewer settled without a verdict")
    );
    f.event(&run, EventKind::Verdict, json!({"round": 0}));
    f.sample(&run, 104, pi(true, false, false, json!({})));
    assert_eq!(f.reason(&run), None);

    let (f, worker) = Fixture::new(Role::Worker, Status::Review, true);
    f.sample(&worker, 100, pi(true, false, false, json!({})));
    assert_eq!(f.reason(&worker), None); // gate/pipeline owns the next action
}

#[test]
fn non_pi_requires_sustained_idle_and_resets_after_progress() {
    let (f, run) = Fixture::new(Role::Worker, Status::Running, false);
    f.sample(&run, 100, idle());
    f.sample(&run, 219, idle());
    assert_eq!(f.reason(&run), None);
    f.sample(&run, 220, idle());
    assert!(f.reason(&run).unwrap().starts_with("Possibly stalled"));
    f.sample(
        &run,
        221,
        Observation {
            status: Some(SessionStatus::Working),
            ..idle()
        },
    );
    f.sample(&run, 500, idle());
    assert_eq!(f.reason(&run), None);
    f.sample(
        &run,
        610,
        Observation {
            status: None,
            ..idle()
        },
    );
    f.sample(&run, 621, idle());
    assert_eq!(f.reason(&run), None);
}
