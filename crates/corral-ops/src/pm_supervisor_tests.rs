use super::*;

fn snapshot() -> Snapshot {
    Snapshot {
        owned: true,
        supervise: true,
        host_alive: false,
        pending: true,
        wake_age: None,
        attention_secs: 60,
        input_limit: false,
        usage_guard_paused: false,
        attempts: Attempts::default(),
        now: 1_000,
    }
}

type DecisionCase = (&'static str, fn(&mut Snapshot), Action);

#[test]
fn recovery_decisions_obey_ownership_opt_in_liveness_and_circuit_breaker() {
    let cases: &[DecisionCase] = &[
        ("foreign socket", |s| s.owned = false, Action::None),
        ("disabled", |s| s.supervise = false, Action::None),
        (
            "dead with work",
            |_| {},
            Action::Relaunch {
                reason: "host_dead",
                replace: false,
            },
        ),
        ("dead without work", |s| s.pending = false, Action::None),
        (
            "live stale wake",
            |s| {
                s.host_alive = true;
                s.wake_age = Some(61);
            },
            Action::Relaunch {
                reason: "wake_unobserved",
                replace: true,
            },
        ),
        (
            "live recent wake",
            |s| {
                s.host_alive = true;
                s.wake_age = Some(60);
            },
            Action::None,
        ),
        (
            "exhausted keys",
            |s| {
                s.host_alive = true;
                s.input_limit = true;
            },
            Action::Relaunch {
                reason: "input_limit",
                replace: true,
            },
        ),
        ("cooldown", |s| s.attempts.last_at = Some(701), Action::None),
        (
            "cooldown expired",
            |s| s.attempts.last_at = Some(700),
            Action::Relaunch {
                reason: "host_dead",
                replace: false,
            },
        ),
        (
            "third accepted launch still has time to recover",
            |s| {
                s.attempts.failures = 3;
                s.attempts.last_at = Some(701);
                s.attempts.successor_generation = Some(4);
            },
            Action::None,
        ),
        (
            "third accepted launch never completed hello",
            |s| {
                s.attempts.failures = 3;
                s.attempts.last_at = Some(700);
                s.attempts.successor_generation = Some(4);
            },
            Action::Failed,
        ),
        ("third failure", |s| s.attempts.failures = 3, Action::Failed),
        (
            "already reported",
            |s| {
                s.attempts.failures = 3;
                s.attempts.reported = true;
            },
            Action::None,
        ),
        (
            "disabled failed circuit",
            |s| {
                s.supervise = false;
                s.attempts.failures = 3;
            },
            Action::None,
        ),
    ];
    for (name, update, expected) in cases {
        let mut state = snapshot();
        update(&mut state);
        assert_eq!(&decide(&state), expected, "{name}");
    }
}

#[test]
fn accepted_launches_require_successor_hello_and_observation_to_reset_the_circuit() {
    let mut state = snapshot();
    for generation in 1..=3 {
        state.now = 1_000 + i64::from(generation - 1) * BACKOFF;
        assert_eq!(
            decide(&state),
            Action::Relaunch {
                reason: "host_dead",
                replace: false
            },
        );
        state.attempts.start(generation, state.now);
        // Relaunch accepted the command and advanced the generation, but no hello followed.
        state.attempts.launched_generation(generation + 1);
        assert!(
            !state
                .attempts
                .observe(Some(generation), generation + 1, false, false, false)
        );
        assert_eq!(state.attempts.failures, generation as u8);
        state.now += BACKOFF - 1;
        assert_eq!(decide(&state), Action::None);
        state.now += 1;
        if generation == 3 {
            assert_eq!(decide(&state), Action::Failed);
        }
    }
    assert_eq!(state.attempts.from_generation, Some(3));
    assert_eq!(state.attempts.last_at, Some(1_600));
    assert!(!state.attempts.observe(Some(4), 4, false, false, true));
    assert!(!state.attempts.observe(Some(4), 4, true, true, false));
    assert_eq!(state.attempts.failures, 3);
    state.attempts.reported = true;
    assert_eq!(decide(&state), Action::None);
    assert!(state.attempts.observe(Some(4), 4, true, true, true));
    assert_eq!(state.attempts.failures, 0);
    assert!(!state.attempts.reported);
    assert_eq!(state.attempts.last_at, Some(1_600));
    assert!(matches!(decide(&state), Action::Relaunch { .. }));

    // A generation unrelated to our expected successor must prove its host live.
    state.attempts.start(4, 2_000);
    assert!(!state.attempts.observe(Some(4), 6, false, false, false));
    assert_eq!(state.attempts.failures, 1);
    assert!(state.attempts.observe(Some(6), 6, true, true, false));
    assert_eq!(state.attempts.failures, 0);
}

#[test]
fn lock_watchdog_reports_once_per_continuously_held_episode() {
    let root = tempfile::tempdir().unwrap();
    let ctx = Ctx::from_store(corral_core::Store::init(root.path()).unwrap());
    let held = ctx.store.try_lock("artifact-storage").unwrap().unwrap();
    let mut heartbeat = Heartbeat::new(Timestamp::now());
    heartbeat.locks.insert(
        "artifact-storage".into(),
        LockWatch {
            first_seen: 100,
            reported: false,
        },
    );
    watchdog(&ctx, &mut heartbeat, 399).unwrap();
    assert!(
        ctx.store
            .events_of(None, &["lock_held"])
            .unwrap()
            .is_empty()
    );
    watchdog(&ctx, &mut heartbeat, 400).unwrap();
    watchdog(&ctx, &mut heartbeat, 401).unwrap();
    // The emitted flag survives a watch-driver restart without a second event.
    let mut restarted: Heartbeat =
        serde_json::from_value(serde_json::to_value(&heartbeat).unwrap()).unwrap();
    watchdog(&ctx, &mut restarted, 900).unwrap();
    assert_eq!(ctx.store.events_of(None, &["lock_held"]).unwrap().len(), 1);
    drop(held);
    watchdog(&ctx, &mut restarted, 901).unwrap();
    assert!(!restarted.locks.contains_key("artifact-storage"));
    let _held_again = ctx.store.try_lock("artifact-storage").unwrap().unwrap();
    watchdog(&ctx, &mut restarted, 1_000).unwrap();
    watchdog(&ctx, &mut restarted, 1_299).unwrap();
    assert_eq!(ctx.store.events_of(None, &["lock_held"]).unwrap().len(), 1);
    watchdog(&ctx, &mut restarted, 1_300).unwrap();
    assert_eq!(ctx.store.events_of(None, &["lock_held"]).unwrap().len(), 2);
}
