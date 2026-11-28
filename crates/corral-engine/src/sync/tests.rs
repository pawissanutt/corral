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
            .reconcile_sample(
                run,
                seq,
                sample,
                None,
                Timestamp::from_second(secs).unwrap(),
            )
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
            None,
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

#[cfg(unix)]
fn terminal_error_samples<const N: usize>(
    f: &Fixture,
    mut run: Run,
    input_id: &str,
    samples: [(bool, Value, Option<&str>); N],
    clock: Option<[i64; N]>,
) -> [Task; N] {
    use std::{
        io::{BufRead, BufReader, Write},
        os::unix::net::UnixListener,
        thread,
    };

    let dir = tempfile::tempdir_in("/tmp").unwrap();
    let socket = dir.path().join("board.sock");
    let Some(SessionRef::Pi { socket: path, .. }) = &mut run.session else {
        unreachable!("expected a Pi run");
    };
    *path = socket.clone();
    f.engine.store.update_run(&run).unwrap();
    let states = samples.map(|(waiting, input, question)| {
        let owner_questions: Vec<_> = question
            .into_iter()
            .map(|question| json!({"question": question}))
            .collect();
        json!({
            "paused": false, "turn": {"open": false}, "owner_questions": owner_questions,
            "last_error": "Provider failed after closing the turn",
            "activity": {"settled": true, "busy": false, "waiting": waiting,
                "turn": 1, "inputs": {input_id: input}},
        })
    });
    let listener = UnixListener::bind(socket).unwrap();
    let host = thread::spawn(move || {
        for state in states {
            let (mut connection, _) = listener.accept().unwrap();
            let mut line = String::new();
            BufReader::new(&connection).read_line(&mut line).unwrap();
            let req: Value = serde_json::from_str(&line).unwrap();
            assert_eq!(req["command"], "get_state");
            writeln!(
                connection,
                "{}",
                json!({
                    "id": req["id"], "version": 2, "ok": true, "result": state,
                })
            )
            .unwrap();
        }
    });
    let tasks = std::array::from_fn(|index| {
        if let Some(clock) = clock {
            let (status, paused, activity) = f
                .engine
                .session_runner()
                .observe_activity(run.session.as_ref().unwrap())
                .unwrap();
            f.sample(
                &run,
                clock[index],
                Observation {
                    status: Some(status),
                    paused,
                    activity,
                },
            );
        } else {
            f.engine.reconcile(&run).unwrap();
        }
        f.engine.store.require_task(&run.task).unwrap()
    });
    host.join().unwrap();
    tasks
}

#[cfg(unix)]
#[test]
fn pi_reviewer_error_does_not_mask_an_outstanding_verdict_or_failed_pickup() {
    let (f, run) = Fixture::new(Role::Reviewer, Status::Review, true);
    f.event(
        &run,
        EventKind::ReviewStarted,
        json!({"round": 0, "pickup": {"input_id": "board-review", "rejected": false}}),
    );
    let [outstanding, waiting, failed] = terminal_error_samples(
        &f,
        run,
        "board-review",
        [
            (false, json!({"state": "applied"}), None),
            (true, json!({"state": "applied"}), None),
            (false, json!({"state": "failed", "turn": 1}), None),
        ],
        None,
    );
    assert!(outstanding.attention.as_deref().is_some_and(|reason| {
        reason.starts_with("Reviewer settled without a verdict") && reason.contains("follow up")
    }));
    assert_eq!(
        waiting.attention, None,
        "an explicit wait still suppresses attention"
    );
    assert!(
        failed
            .attention
            .as_deref()
            .is_some_and(|reason| reason.starts_with("Review request wasn’t picked up"))
    );
    assert_eq!(failed.status, Status::Review);
}

#[cfg(unix)]
#[test]
fn worker_send_back_pickup_failure_survives_a_coarse_provider_block() {
    for (receipt, question) in [
        ("failed", None),
        ("failed", Some("Which option should I take?")),
        ("applied", None),
    ] {
        let (f, run) = Fixture::new(Role::Worker, Status::Running, true);
        f.event(
            &run,
            EventKind::SentBack,
            json!({"pickup": {"input_id": "board-feedback", "rejected": false}}),
        );
        let [_, task] = terminal_error_samples(
            &f,
            run,
            "board-feedback",
            [(); 2].map(|()| {
                (
                    question.is_some(),
                    json!({"state": receipt, "turn": 1}),
                    question,
                )
            }),
            None,
        );
        if receipt == "failed" && question.is_none() {
            assert_eq!(task.status, Status::Running);
            assert!(task.attention.as_deref().is_some_and(|reason|
                reason.starts_with("Send-back feedback wasn’t picked up")));
        } else {
            assert_eq!(task.status, Status::Blocked);
            assert_eq!(task.attention, None);
            assert_eq!(
                task.blocked.unwrap().detail.as_deref(),
                Some(question.unwrap_or("Provider failed after closing the turn"))
            );
        }
    }
}

#[cfg(unix)]
#[test]
fn worker_pending_pickup_reaches_deadline_despite_a_coarse_provider_block() {
    let (f, run) = Fixture::new(Role::Worker, Status::Running, true);
    f.event(
        &run,
        EventKind::SentBack,
        json!({"pickup": {"input_id": "board-feedback", "rejected": false}}),
    );
    let [first, early, expired] = terminal_error_samples(
        &f,
        run,
        "board-feedback",
        // Pi admitted the input before the engine's first quiet sample.
        [2_000, 31_000, 32_000].map(|quiet_ms| {
            (
                false,
                json!({"state": "pending", "quiet_ms": quiet_ms}),
                None,
            )
        }),
        Some([100, 129, 130]),
    );
    for pending in [first, early] {
        assert_eq!(pending.status, Status::Running);
        assert_eq!(pending.attention, None, "before the pickup deadline");
    }
    assert_eq!(expired.status, Status::Running);
    assert!(
        expired
            .attention
            .as_deref()
            .is_some_and(|reason| reason.starts_with("Send-back feedback wasn’t picked up"))
    );
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

#[test]
fn observation_failures_form_restart_safe_per_run_episodes() {
    use std::sync::{Arc, Mutex};

    use crate::{
        Origin,
        runner::{LaunchSpec, Runner},
    };

    #[derive(Clone, Copy)]
    enum Sample {
        Down,
        Timeout,
        Unknown,
        Working,
        Unclassified(&'static str),
        PiRefused(&'static str),
    }

    struct Observer(Arc<Mutex<Sample>>);
    impl Runner for Observer {
        fn reconciliation_status(&self, _: &SessionRef) -> Result<Option<SessionStatus>> {
            match *self.0.lock().unwrap() {
                Sample::Down => Err(anyhow::Error::from(corral_herdr::EngineProblem::Down)
                    .context("transport diagnostic containing a secret")),
                Sample::Timeout => Err(std::io::Error::from(std::io::ErrorKind::TimedOut).into()),
                Sample::Unknown => Ok(None),
                Sample::Working => Ok(Some(SessionStatus::Working)),
                Sample::Unclassified(message) => Err(anyhow::anyhow!(message)),
                Sample::PiRefused(code) => Err(crate::runner::pi::PiRefusal {
                    command: "get_state".to_owned(),
                    code: code.to_owned(),
                    reason: "private diagnostic".to_owned(),
                }
                .into()),
            }
        }
        fn start(&self, _: &LaunchSpec) -> Result<SessionRef> {
            panic!("unexpected restart")
        }
        fn status(&self, _: &SessionRef) -> Result<SessionStatus> {
            panic!("expected reconciliation read")
        }
        fn send(&self, _: &SessionRef, _: &str, _: Origin) -> Result<()> {
            panic!("unexpected input")
        }
        fn stop(&self, _: &SessionRef) -> Result<()> {
            panic!("unexpected stop")
        }
        fn focus(&self, _: &SessionRef) -> Result<()> {
            panic!("unexpected focus")
        }
    }

    let (mut f, worker) = Fixture::new(Role::Worker, Status::Running, false);
    let mut reviewer = worker.clone();
    reviewer.id = "R2".into();
    reviewer.role = Role::Reviewer;
    f.engine.store.insert_run(&reviewer).unwrap();
    // Simulate a pre-episode workspace before the first upgraded writable open.
    f.engine
        .store
        .connection
        .execute("DROP TABLE run_observation_episodes", [])
        .unwrap();
    let sample = Arc::new(Mutex::new(Sample::Down));
    let open =
        || Engine::open_with_runner(f.root.path(), Box::new(Observer(sample.clone()))).unwrap();
    f.engine = open();
    f.engine.sync().unwrap();
    let log = |engine: &Engine| engine.store.list_events(&worker.task).unwrap();
    let first = log(&f.engine);
    assert_eq!(first.len(), 2, "first observation is visible for each run");
    f.engine = open();
    f.engine.sync().unwrap();
    *sample.lock().unwrap() = Sample::Unknown;
    f.engine.sync().unwrap();
    assert_eq!(
        log(&f.engine),
        first,
        "repeats and unknown status do not append events"
    );
    assert!(!serde_json::to_string(&first).unwrap().contains("secret"));
    assert_eq!(f.engine.store.live_runs(None).unwrap().len(), 2);
    let failed = || Observation {
        status: None,
        paused: None,
        activity: None,
    };
    let down = episodes::Failure::from_error(&corral_herdr::EngineProblem::Down.into());
    let last_seen = Timestamp::from_second(first[1].ts.as_second() + 60).unwrap();
    for run in [&worker, &reviewer] {
        let seq = f.engine.store.task_event_seq(&run.task).unwrap();
        f.engine
            .reconcile_sample(run, seq, failed(), Some(&down), last_seen)
            .unwrap();
    }
    assert_eq!(
        f.engine.store.require_task(&worker.task).unwrap().status,
        Status::Running
    );

    let attention = || {
        let mut tasks = [f.engine.store.require_task(&worker.task).unwrap()];
        tasks[0].attention = Some("Existing advisory.".into());
        f.engine
            .store
            .add_observation_attention(&mut tasks)
            .unwrap();
        tasks[0].attention.take().unwrap()
    };
    let current = attention();
    assert!(current.starts_with("Existing advisory."));
    assert!(current.contains("R1 session state unknown"));
    assert!(current.contains("R2 session state unknown"));
    assert_eq!(current.matches("3 failed").count(), 2);
    assert_eq!(
        current
            .matches(format!("last {last_seen}").as_str())
            .count(),
        2
    );

    for next in [Sample::Timeout, Sample::Working, Sample::Timeout] {
        *sample.lock().unwrap() = next;
        f.engine.sync().unwrap();
        if matches!(next, Sample::Working) {
            assert_eq!(attention(), "Existing advisory.");
        }
    }
    let events = log(&f.engine);
    assert_eq!(
        &events[..first.len()],
        first,
        "opening events are immutable"
    );
    for run in [&worker, &reviewer] {
        let history: Vec<_> = events
            .iter()
            .filter(|event| event.run.as_ref() == Some(&run.id))
            .collect();
        let states: Vec<_> = history
            .iter()
            .map(|event| event.detail["state"].as_str().unwrap())
            .collect();
        assert_eq!(
            states,
            ["opened", "cause_changed", "opened", "recovered", "opened"]
        );
        assert_eq!(history[1].detail["observations"], 3);
        assert_eq!(
            history[1].detail["first_seen"],
            history[0].detail["first_seen"]
        );
        assert_eq!(history[1].detail["last_seen"], json!(last_seen));
        assert_ne!(history[0].detail["cause"], history[2].detail["cause"]);
        assert_eq!(history[2].detail["cause"], history[4].detail["cause"]);
        assert_eq!(history[3].detail["opened_seq"], history[2].seq);
    }

    // A sample captured before the run retired cannot reopen its episode or
    // shift the failure onto the other live run.
    let seq = f.engine.store.task_event_seq(&worker.task).unwrap();
    let mut retired = worker.clone();
    retired.ended = Some(Timestamp::now());
    retired.outcome = Some(Outcome::Stopped);
    f.engine.store.update_run(&retired).unwrap();
    let retired_log = log(&f.engine);
    let closure = retired_log.last().unwrap();
    assert_eq!(closure.run.as_deref(), Some(worker.id.as_str()));
    assert_eq!(closure.detail["state"], "run_ended");
    f.engine
        .reconcile_sample(&worker, seq, failed(), Some(&down), Timestamp::now())
        .unwrap();
    assert_eq!(log(&f.engine), retired_log);
    assert!(!attention().contains("R1 session state unknown"));
    assert!(attention().contains("R2 session state unknown"));

    for next in [
        Sample::Unclassified("private A"),
        Sample::Unclassified("private A"),
        Sample::Unclassified("private B"),
        Sample::PiRefused("private C"),
        Sample::PiRefused("private C"),
        Sample::PiRefused("private D"),
    ] {
        *sample.lock().unwrap() = next;
        f.engine.sync().unwrap();
    }
    let events = log(&f.engine);
    let openings: Vec<_> = events
        .iter()
        .skip(retired_log.len())
        .filter(|event| event.detail["state"] == "opened")
        .map(|event| event.detail["signature"].as_str().unwrap())
        .collect();
    assert_eq!(openings.len(), 4);
    assert_ne!(openings[0], openings[1]);
    assert_ne!(openings[2], openings[3]);
    assert!(!serde_json::to_string(&events).unwrap().contains("private"));
}
