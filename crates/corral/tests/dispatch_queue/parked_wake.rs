//! Review round 3 (T-513): native wakes wait for the slot grant, saturated claimants reserve
//! nothing, and the documented slot wait cannot miss a grant.
use std::process::Stdio;

use corral_core::agent::{AgentRunState, WakeState};
use corral_core::{RunOutcome, Status, Task, WaitState};
use jiff::{SignedDuration, Timestamp};
use serde_json::json;

use crate::queue_support::Fixture;

fn running(f: &Fixture, project: &str) -> String {
    let task = f.task(project, Status::Ready);
    f.ok(&["dispatch", &task.id]);
    task.id
}

fn park(f: &Fixture, id: &str, wait: &str, on: &str, deadline: Timestamp, now: Timestamp) {
    let request = json!({
        "version": 1, "id": wait, "run": "R1", "condition": format!("{on} lands"),
        "owner": "worker", "source": "worker", "deadline": deadline,
        "handle": {"kind": "channel", "channel_id": format!("task:{on}:landed"), "capability": "wait_v1"},
    });
    corral_ops::register_wait(&f.ctx, id, serde_json::from_value(request).unwrap(), now).unwrap();
}

fn receipt(f: &Fixture, id: &str, wait: &str, registered: Timestamp) {
    // Keep the observation inside the controlled lifetime even if wall time steps back.
    let now = registered + SignedDuration::from_secs(1);
    let receipt = json!({
        "version": 1, "id": format!("{wait}-landed"), "run": "R1", "outcome": "ready",
        "delivery": "acknowledged", "observed_at": now, "answer": null,
    });
    corral_ops::resume_run(
        &f.ctx,
        id,
        corral_ops::ResumeTarget::Wait(wait),
        serde_json::from_value(receipt).unwrap(),
        now,
    )
    .unwrap();
}

/// Binds the run to a native worker with one armed wake per wait, as the Pi-native `wait` op does.
fn bind_native(f: &Fixture, id: &str, waits: &[&str]) {
    let wakes: Vec<_> = waits
        .iter()
        .map(|w| {
            json!({"key": w, "request_id": format!("req-{w}"), "kind": "wait",
            "target": w, "state": "armed", "at": Timestamp::now()})
        })
        .collect();
    let state: AgentRunState = serde_json::from_value(json!({
        "version": 1,
        "binding": {"role": "worker", "store": "isolated-fixture", "project": "app",
            "generation": 1, "health": "healthy"},
        "wakes": wakes,
    }))
    .unwrap();
    f.ctx
        .store
        .update(id, |t| {
            t.current_run_mut().unwrap().agent_host = Some(Box::new(state.clone()));
            Ok(())
        })
        .unwrap();
}

fn wake_states(task: &Task) -> Vec<WakeState> {
    let state = task.current_run().unwrap().agent_host.as_ref().unwrap();
    state.wakes.iter().map(|w| w.state).collect()
}

fn posted(f: &Fixture, id: &str) -> usize {
    let events = f.ctx.store.events().unwrap();
    events
        .iter()
        .filter(|e| e.task.as_deref() == Some(id) && e.kind == "agent_wake_posted")
        .count()
}

/// Posts whatever the production selector finds due, as the sync hook does.
fn post_due(f: &Fixture, id: &str) -> usize {
    let task = f.ctx.store.get(id).unwrap();
    corral_ops::agent::wake::post_due(&f.ctx, &corral_ops::agent::wake::due_wait_wakes(&task))
        .unwrap()
}

fn finish(f: &Fixture, id: &str) {
    f.ctx
        .store
        .update(id, |t| {
            t.status = Status::Done;
            t.current_run_mut().unwrap().outcome = Some(RunOutcome::Done);
            Ok(())
        })
        .unwrap();
}

fn grant(f: &Fixture, id: &str, at: Timestamp) {
    corral_ops::sync_at(&f.ctx, at).unwrap();
    let task = f.ctx.store.get(id).unwrap();
    assert!(task.current_run().unwrap().obligations.slot_wait.is_none());
}

#[test]
fn receipt_clock_follows_registration_and_rejects_an_earlier_observation() {
    let f = Fixture::new();
    let a = running(&f, "app");
    // Model a registration ahead of the wall clock after a backwards clock step.
    let registered = Timestamp::now() + SignedDuration::from_hours(1);
    park(
        &f,
        &a,
        "landing",
        "T-999",
        registered + SignedDuration::from_hours(1),
        registered,
    );
    let early = json!({
        "version": 1, "id": "early", "run": "R1", "outcome": "ready",
        "delivery": "acknowledged",
        "observed_at": registered - SignedDuration::from_secs(1), "answer": null,
    });
    let err = corral_ops::resume_run(
        &f.ctx,
        &a,
        corral_ops::ResumeTarget::Wait("landing"),
        serde_json::from_value(early).unwrap(),
        registered + SignedDuration::from_secs(1),
    )
    .unwrap_err();
    assert!(
        err.to_string()
            .contains("outside this registration's lifetime")
    );
    let task = f.ctx.store.get(&a).unwrap();
    assert!(
        task.current_run().unwrap().obligations.waits[0]
            .receipt
            .is_none()
    );
    receipt(&f, &a, "landing", registered);
}

#[test]
fn native_wake_from_a_receipt_waits_for_the_slot_grant_then_fires_once() {
    // Given A parked on X, B holding the only slot, and A bound to a native worker.
    let f = Fixture::new();
    let a = running(&f, "app");
    let now = Timestamp::now();
    park(
        &f,
        &a,
        "landing",
        "T-999",
        now + SignedDuration::from_hours(24),
        now,
    );
    let b = running(&f, "other");
    bind_native(&f, &a, &["landing"]);

    // When A's wait fires while no slot is free.
    receipt(&f, &a, "landing", now);

    // Then the wake stays owed: the selector skips it and a stale selection posts nothing.
    let task = f.ctx.store.get(&a).unwrap();
    assert!(task.current_run().unwrap().obligations.slot_wait.is_some());
    assert!(corral_ops::agent::wake::due_wait_wakes(&task).is_empty());
    let mut stale = task.clone();
    stale.current_run_mut().unwrap().obligations.slot_wait = None;
    let selected = corral_ops::agent::wake::due_wait_wakes(&stale);
    assert_eq!(selected.len(), 1);
    assert_eq!(
        corral_ops::agent::wake::post_due(&f.ctx, &selected).unwrap(),
        0
    );
    assert_eq!(post_due(&f, &a), 0);
    let task = f.ctx.store.get(&a).unwrap();
    assert_eq!(wake_states(&task), [WakeState::Armed]);
    assert_eq!(posted(&f, &a), 0);

    // When B finishes and sync grants A the slot.
    finish(&f, &b);
    grant(&f, &a, now + SignedDuration::from_secs(2));

    // Then exactly one wake is delivered.
    post_due(&f, &a);
    post_due(&f, &a);
    assert_eq!(posted(&f, &a), 1);
    let task = f.ctx.store.get(&a).unwrap();
    assert_eq!(wake_states(&task), [WakeState::Fired]);
}

#[test]
fn native_wakes_from_expiry_and_a_second_task_wait_wait_for_the_grant() {
    // Given A parked on two tasks with deadlines, B holding the only slot, and a native binding.
    let f = Fixture::new();
    let a = running(&f, "app");
    let now = Timestamp::now();
    let deadline = now + SignedDuration::from_hours(1);
    park(&f, &a, "first", "T-998", deadline, now);
    park(
        &f,
        &a,
        "second",
        "T-999",
        now + SignedDuration::from_hours(2),
        now,
    );
    let b = running(&f, "other");
    bind_native(&f, &a, &["first", "second"]);

    // When the first wait expires while the second is still open.
    let later = deadline + SignedDuration::from_secs(1);
    corral_ops::sync_at(&f.ctx, later).unwrap();

    // Then A stays parked on T-999 and no wake is delivered.
    let task = f.ctx.store.get(&a).unwrap();
    let run = task.current_run().unwrap();
    assert_eq!(
        run.obligations.waits[0].observation.state,
        WaitState::Expired
    );
    assert_eq!(run.parked_on(&a), ["T-999"]);
    assert_eq!(post_due(&f, &a), 0);

    // When the second wait expires too, A queues for the slot B still holds.
    let last = now + SignedDuration::from_hours(2) + SignedDuration::from_secs(1);
    corral_ops::sync_at(&f.ctx, last).unwrap();
    let task = f.ctx.store.get(&a).unwrap();
    assert!(task.current_run().unwrap().obligations.slot_wait.is_some());
    assert_eq!(post_due(&f, &a), 0);
    assert_eq!(posted(&f, &a), 0);

    // Then after the grant both owed wakes are delivered exactly once.
    finish(&f, &b);
    grant(&f, &a, last);
    post_due(&f, &a);
    post_due(&f, &a);
    assert_eq!(posted(&f, &a), 2);
    let task = f.ctx.store.get(&a).unwrap();
    assert_eq!(wake_states(&task), [WakeState::Fired, WakeState::Fired]);
}

#[test]
fn saturated_project_claimants_do_not_reserve_another_projects_slot() {
    // Given two global slots, app capped at one and held by a worker.
    let mut f = Fixture::new();
    f.ctx.store.config.limits.running = 2;
    f.ctx
        .store
        .config
        .limits
        .per_project
        .insert("app".into(), 1);
    f.ctx.store.save_config().unwrap();
    let woken = running(&f, "app");
    let now = Timestamp::now();
    park(
        &f,
        &woken,
        "landing",
        "T-999",
        now + SignedDuration::from_hours(1),
        now,
    );
    let busy = running(&f, "app");

    // And both an app red-main fix and an app woken run waiting for capacity.
    let fix = f.task("app", Status::Ready);
    f.ctx
        .store
        .update(&fix.id, |t| {
            t.labels = vec!["red_main".into()];
            Ok(())
        })
        .unwrap();
    receipt(&f, &woken, "landing", now);
    assert!(
        f.ctx
            .store
            .get(&woken)
            .unwrap()
            .current_run()
            .unwrap()
            .obligations
            .slot_wait
            .is_some()
    );

    // When another project's task asks for the spare global slot.
    let fixed = f.cli(&["dispatch", &fix.id]);
    let other = f.task("other", Status::Ready);
    f.ok(&["dispatch", &other.id]);

    // Then the fix is refused by its project cap and the other project's task runs.
    assert!(!fixed.status.success());
    assert!(String::from_utf8_lossy(&fixed.stderr).contains("for project app"));
    assert_eq!(f.ctx.store.get(&other.id).unwrap().status, Status::Running);

    // And once app has room, the red-main fix still goes first.
    finish(&f, &busy);
    finish(&f, &other.id);
    let listed: serde_json::Value =
        serde_json::from_slice(&f.ok(&["queue", "list", "--json"]).stdout).unwrap();
    let blocked = |id: &str| {
        listed
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["id"] == id)
            .unwrap()["reason"]
            .as_str()
            .unwrap()
            .to_string()
    };
    assert!(blocked(&woken).contains("woken"), "{listed:#}");
    let out = f.cli(&["dispatch", &fix.id]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn slot_since(f: &Fixture, id: &str) -> String {
    let task = f.ctx.store.get(id).unwrap();
    let slot = task.current_run().unwrap().obligations.slot_wait.clone();
    slot.unwrap().since.to_string()
}

fn wait_args<'a>(id: &'a str, since: &'a str, timeout: &'a str) -> Vec<&'a str> {
    vec![
        "wait",
        "--for",
        "slot_reacquired",
        "--task",
        id,
        "--since",
        since,
        "--timeout",
        timeout,
        "--json",
    ]
}

#[test]
fn slot_wait_command_sees_a_grant_logged_before_or_after_it_starts() {
    // Given two woken runs queued behind B, which holds the only slot.
    let f = Fixture::new();
    let now = Timestamp::now();
    let a = running(&f, "app");
    park(
        &f,
        &a,
        "landing",
        "T-998",
        now + SignedDuration::from_hours(1),
        now,
    );
    let c = running(&f, "app");
    park(
        &f,
        &c,
        "landing",
        "T-999",
        now + SignedDuration::from_hours(1),
        now,
    );
    let b = running(&f, "other");
    receipt(&f, &a, "landing", now);
    receipt(&f, &c, "landing", now);
    let (a_since, c_since) = (slot_since(&f, &a), slot_since(&f, &c));

    // Then events other than the grant (run_resumed, wait_transition) never release the wait.
    let out = f.cli(&wait_args(&a, &a_since, "200ms"));
    assert_eq!(out.status.code(), Some(124), "{out:?}");

    // When C's wait is already subscribed and A's starts only after its grant.
    let subscribed = f
        .command(&wait_args(&c, &c_since, "60s"))
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    finish(&f, &b);
    grant(&f, &a, now + SignedDuration::from_secs(2));
    let a_out = f.ok(&wait_args(&a, &a_since, "60s"));
    finish(&f, &a);
    grant(&f, &c, now + SignedDuration::from_secs(3));
    let c_out = subscribed.wait_with_output().unwrap();

    // Then both waits return that run's grant.
    for (id, out) in [(&a, a_out), (&c, c_out)] {
        assert!(out.status.success(), "{out:?}");
        let event: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(event["kind"], "slot_reacquired", "{event:#}");
        assert_eq!(event["task"], id.as_str());
    }
}
