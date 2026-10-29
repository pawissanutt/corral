use corral_core::{ReviewWork, Status, WaitState};
use serde_json::{Value, json};

use crate::queue_support::Fixture;

const LIMIT: usize = 2;

fn fixture() -> Fixture {
    let mut f = Fixture::new();
    f.ctx.store.config.limits.running = LIMIT;
    f.ctx.store.config.limits.review = 1;
    f.ctx.store.config.dispatch.auto_backlog = false;
    f.ctx.store.save_config().unwrap();
    f
}

fn running(f: &Fixture, project: &str) -> String {
    let task = f.task(project, Status::Ready);
    f.ok(&["dispatch", &task.id]);
    task.id
}

fn json_file(f: &Fixture, name: &str, value: &Value) -> String {
    let path = f.temp.path().join(name);
    std::fs::write(&path, value.to_string()).unwrap();
    path.to_string_lossy().into_owned()
}

/// The worker of `id` records a wait for `on` to land, as the T-505/T-506 workers did.
fn park(f: &Fixture, id: &str, on: &str) {
    let wait = json!({
        "version": 1, "id": "landing", "run": "R1",
        "condition": format!("{on} lands on main"), "owner": "worker", "source": "worker",
        "deadline": "2099-01-01T00:00:00Z",
        "handle": {"kind": "channel", "channel_id": format!("task:{on}:landed"), "capability": "wait_v1"},
    });
    f.ok(&[
        "run",
        "wait",
        id,
        "--file",
        &json_file(f, &format!("wait-{id}.json"), &wait),
    ]);
}

fn wake(f: &Fixture, id: &str) -> std::process::Output {
    // Derive the observation from the registration the subprocess recorded, not the wall clock.
    let registered = f
        .ctx
        .store
        .get(id)
        .unwrap()
        .current_run()
        .unwrap()
        .obligations
        .waits
        .iter()
        .find(|w| w.request.id == "landing")
        .unwrap()
        .registered_at;
    let receipt = json!({
        "version": 1, "id": "landed", "run": "R1", "outcome": "ready",
        "delivery": "acknowledged",
        "observed_at": registered, "answer": null,
    });
    let file = json_file(f, &format!("receipt-{id}.json"), &receipt);
    f.ok(&["run", "resume", id, "--wait", "landing", "--file", &file])
}

fn queue(f: &Fixture) -> Value {
    serde_json::from_slice(&f.ok(&["queue", "list", "--json"]).stdout).unwrap()
}

fn reason(queue: &Value, id: &str) -> String {
    queue
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == id)
        .and_then(|e| e["reason"].as_str())
        .unwrap_or_else(|| panic!("{id} missing from {queue:#}"))
        .to_string()
}

fn slot_wait(f: &Fixture, id: &str) -> bool {
    let task = f.ctx.store.get(id).unwrap();
    task.current_run().unwrap().obligations.slot_wait.is_some()
}

#[test]
fn slot_limit_parked_runs_do_not_block_the_task_they_wait_on() {
    // Given every coding slot held by a worker, and the fix X queued behind them.
    let f = fixture();
    let workers: Vec<_> = (0..LIMIT).map(|_| running(&f, "app")).collect();
    let fix = f.task("other", Status::Ready);
    f.ok(&["queue", "add", &fix.id]);
    f.ok(&["sync"]);
    assert!(reason(&queue(&f), &fix.id).contains("waiting for a slot"));

    // When all N = limit workers record a wait on X landing.
    for id in &workers {
        park(&f, id, &fix.id);
    }

    // Then the parked runs hold no slot and X dispatches on the next sync.
    let usage = corral_ops::slot_usage(&f.ctx).unwrap();
    assert_eq!((usage.tasks, usage.parked, usage.total()), (0, LIMIT, 0));
    let listed = queue(&f);
    for id in &workers {
        let reason = reason(&listed, id);
        assert!(
            reason.contains(&format!("parked on {}", fix.id)),
            "{reason}"
        );
        assert!(
            reason.contains("released the R1 coding slot (project app)"),
            "{reason}"
        );
    }
    let text = String::from_utf8(f.ok(&["queue", "list"]).stdout).unwrap();
    assert!(
        text.contains(&format!("({LIMIT} released by parked runs)")),
        "{text}"
    );
    f.ok(&["sync"]);
    assert_eq!(f.ctx.store.get(&fix.id).unwrap().status, Status::Running);
    for id in &workers {
        assert_eq!(f.ctx.store.get(id).unwrap().status, Status::Running);
    }
}

#[test]
fn a_woken_run_waits_in_queue_then_reacquires_ahead_of_queued_work() {
    // Given A parked on X while B and the fix X fill both slots.
    let f = fixture();
    let a = running(&f, "app");
    let b = running(&f, "app");
    let fix = f.task("other", Status::Ready);
    park(&f, &a, &fix.id);
    f.ok(&["dispatch", &fix.id]);
    let later = f.task("other", Status::Ready);
    f.ok(&["queue", "add", &later.id]);

    // When A's wait fires with no slot free.
    let out = wake(&f, &a);

    // Then A queues for a slot instead of taking one, and leads the queue.
    assert!(String::from_utf8_lossy(&out.stderr).contains("queued to re-acquire a coding slot"));
    assert!(slot_wait(&f, &a));
    let task = f.ctx.store.get(&a).unwrap();
    assert_eq!(
        task.current_run().unwrap().obligations.waits[0]
            .observation
            .state,
        WaitState::Ready
    );
    assert_eq!(corral_ops::slot_usage(&f.ctx).unwrap().total(), LIMIT);
    let listed = queue(&f);
    assert_eq!(listed[0]["id"], a);
    assert!(reason(&listed, &a).contains(&format!("woken from {}", fix.id)));
    f.ok(&["sync"]);
    assert!(slot_wait(&f, &a));

    // When B finishes and frees one slot.
    f.ok(&[
        "done",
        &b,
        "--summary",
        "Verified: fixture\nNot verified: none",
    ]);
    assert!(
        reason(&queue(&f), &later.id).contains(&format!("{a} take the next coding slot first"))
    );
    f.ok(&["sync"]);

    // Then A re-acquires that slot before the queued task dispatches.
    assert!(!slot_wait(&f, &a));
    assert!(f.ctx.store.get(&later.id).unwrap().runs.is_empty());
    let usage = corral_ops::slot_usage(&f.ctx).unwrap();
    assert_eq!((usage.tasks, usage.parked), (LIMIT, 0));
    assert!(
        f.ctx
            .store
            .events()
            .unwrap()
            .iter()
            .any(|e| e.task.as_deref() == Some(a.as_str()) && e.kind == "slot_reacquired")
    );
}

#[test]
fn a_woken_run_with_a_free_slot_reacquires_it_at_once() {
    // Given A parked on X with a slot free.
    let f = fixture();
    let a = running(&f, "app");
    park(&f, &a, "T-999");
    assert_eq!(corral_ops::slot_usage(&f.ctx).unwrap().total(), 0);

    // When the wait fires.
    let out = wake(&f, &a);

    // Then A holds a slot again before the command returns.
    assert!(!String::from_utf8_lossy(&out.stderr).contains("queued"));
    assert!(!slot_wait(&f, &a));
    assert_eq!(corral_ops::slot_usage(&f.ctx).unwrap().tasks, 1);
}

#[test]
fn red_main_fix_jumps_the_queue_and_takes_the_next_slot() {
    // Given one busy slot, an older P0 task and a newer P3 red-main fix both queued.
    let mut f = fixture();
    f.ctx.store.config.limits.running = 3;
    f.ctx.store.save_config().unwrap();
    running(&f, "app");
    let busy = running(&f, "app");
    let urgent = f.task("app", Status::Ready);
    let fix = f.task("other", Status::Ready);
    for (task, priority, labels) in [
        (&urgent, "P0", vec![]),
        (&fix, "P3", vec!["red_main".to_string()]),
    ] {
        f.ctx
            .store
            .update(&task.id, |t| {
                t.priority = priority.into();
                t.labels = labels.clone();
                Ok(())
            })
            .unwrap();
    }
    f.ok(&["queue", "add", &urgent.id]);
    f.ok(&["queue", "add", &fix.id]);

    // Then the fix leads the queue and the P0 task may not take the last slot by hand.
    let listed = queue(&f);
    assert_eq!(listed[0]["id"], fix.id);
    let out = f.cli(&["dispatch", &urgent.id]);
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr)
            .contains(&format!("{} take the next coding slot first", fix.id))
    );

    // When a run finishes and the queue drains one automatic slot beside the finishing reserve.
    f.ok(&[
        "done",
        &busy,
        "--summary",
        "Verified: fixture\nNot verified: none",
    ]);
    f.ok(&["sync"]);

    // Then the fix takes it and the P0 task waits.
    assert_eq!(
        f.ctx.store.get(&fix.id).unwrap().status,
        Status::Running,
        "{:#}",
        queue(&f)
    );
    assert!(f.ctx.store.get(&urgent.id).unwrap().runs.is_empty());
}

#[test]
fn red_main_fix_takes_the_next_gate_slot() {
    // Given two finished runs and a red-main fix already waiting for gate admission.
    let mut f = fixture();
    f.ctx.store.config.review.gate = Some(vec!["true".into()]);
    f.ctx.store.config.limits.running = 4;
    f.ctx.store.config.limits.count_gates = false;
    f.ctx.store.save_config().unwrap();
    let finished = |f: &Fixture| {
        let id = running(f, "app");
        f.ok(&[
            "done",
            &id,
            "--summary",
            "Verified: fixture\nNot verified: none",
        ]);
        id
    };
    let other = finished(&f);
    let fix = finished(&f);
    f.ctx
        .store
        .update(&fix, |t| {
            t.labels = vec!["red_main".into()];
            t.review_wait = Some(ReviewWork::Gate);
            Ok(())
        })
        .unwrap();

    // When the other run asks for the only gate slot.
    let out = f.cli(&["gate", &other]);

    // Then it waits for the fix, which then takes the slot.
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr)
            .contains(&format!("red-main fix {fix} takes the next gate slot"))
    );
    f.ok(&["gate", &fix]);
    assert!(
        f.ctx
            .store
            .get(&fix)
            .unwrap()
            .current_run()
            .unwrap()
            .gate
            .is_some()
    );
}
