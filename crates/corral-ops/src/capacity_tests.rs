use super::*;
use corral_core::{RunOutcome, Store};
use serde_json::json;

fn run() -> corral_core::Run {
    serde_json::from_value(json!({
        "id":"R1", "agent":"fake", "worktree":"/tmp/capacity-fixture", "branch":"b",
        "base":"main", "base_sha":"head", "started":"2026-01-01T00:00:00Z"
    }))
    .unwrap()
}

#[test]
fn gate_and_reviewer_reservations_belong_to_independent_pools() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::init(dir.path()).unwrap();
    store.config.limits.capacity = Some(CapacityLimits {
        unfinished: 2,
        agents: 1,
        gates: 1,
        adhoc_reserve: 0,
    });
    store.save_config().unwrap();
    let task = store.create("candidate", None, Status::Review, "").unwrap();
    store
        .update(&task.id, |t| {
            let mut r = run();
            r.outcome = Some(RunOutcome::Done);
            t.runs.push(r);
            t.review_reservation = Some(ReviewWork::Gate);
            Ok(())
        })
        .unwrap();
    let limits = CapacityLimits {
        unfinished: 2,
        agents: 1,
        gates: 1,
        adhoc_reserve: 0,
    };
    let used = CapacityUsage::from_heads(&[store.head(&task.id).unwrap()]);
    assert_eq!(
        (used.agents.total(), used.gates, used.unfinished.total()),
        (0, 1, 1)
    );
    assert!(used.gate_reason(&limits).is_some());
    let head = store.head(&task.id).unwrap();
    assert!(
        crate::slots::capacity_reason(&store, &head, 0)
            .unwrap()
            .is_none()
    );
    assert!(admission::gate_reason(&store, 0).unwrap().is_some());
    assert!(used.agent_reason(&limits, CapacityLane::Normal).is_none());
    store
        .update(&task.id, |t| {
            t.review_reservation = Some(ReviewWork::Reviewer);
            Ok(())
        })
        .unwrap();
    let used = CapacityUsage::from_heads(&[store.head(&task.id).unwrap()]);
    assert_eq!(
        (used.agents.total(), used.gates, used.unfinished.total()),
        (1, 0, 1)
    );
    assert!(used.gate_reason(&limits).is_none());
    let head = store.head(&task.id).unwrap();
    assert!(
        crate::slots::capacity_reason(&store, &head, 0)
            .unwrap()
            .is_some()
    );
    assert!(admission::gate_reason(&store, 0).unwrap().is_none());
    assert!(used.agent_reason(&limits, CapacityLane::Normal).is_some());
}

#[test]
fn tracker_checkpoint_reservation_uses_only_the_gate_pool() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::init(dir.path()).unwrap();
    let tracker = store.create("aggregate", None, Status::Ready, "").unwrap();
    store
        .update(&tracker.id, |task| {
            task.tracker = true;
            task.capacity_checkpoint = Some("C1".into());
            Ok(())
        })
        .unwrap();
    let used = CapacityUsage::from_heads(&[store.head(&tracker.id).unwrap()]);
    assert_eq!(
        (used.agents.total(), used.gates, used.unfinished.total()),
        (0, 1, 0)
    );
    store
        .update(&tracker.id, |task| {
            task.capacity_checkpoint = None;
            Ok(())
        })
        .unwrap();
    assert_eq!(
        CapacityUsage::from_heads(&[store.head(&tracker.id).unwrap()]),
        CapacityUsage::default()
    );
}

#[test]
fn durable_lane_and_unfinished_position_survive_park_and_stage_change() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::init(dir.path()).unwrap();
    store.config.limits.capacity = Some(CapacityLimits {
        unfinished: 1,
        agents: 1,
        gates: 1,
        adhoc_reserve: 0,
    });
    store.save_config().unwrap();
    let task = store.create("adhoc", None, Status::Ready, "").unwrap();
    store
        .update(&task.id, |t| {
            t.capacity_lane = CapacityLane::Adhoc;
            t.status = Status::Running;
            let mut r = run();
            r.obligations.slot_wait = Some(corral_core::SlotWait {
                since: r.started,
                on: vec!["T-999".into()],
            });
            t.runs.push(r);
            Ok(())
        })
        .unwrap();
    let used = CapacityUsage::from_heads(&[store.head(&task.id).unwrap()]);
    assert_eq!(used.agents.total(), 0);
    assert!(
        store
            .update(&task.id, |task| {
                task.capacity_lane = CapacityLane::Normal;
                Ok(())
            })
            .is_err()
    );
    assert_eq!(
        used.unfinished,
        CapacityPool {
            normal: 0,
            adhoc: 1
        }
    );
    store
        .update(&task.id, |t| {
            t.status = Status::Review;
            t.current_run_mut().unwrap().outcome = Some(RunOutcome::Done);
            t.review_reservation = Some(ReviewWork::Reviewer);
            Ok(())
        })
        .unwrap();
    let used = CapacityUsage::from_heads(&[store.head(&task.id).unwrap()]);
    assert_eq!(
        used.agents,
        CapacityPool {
            normal: 0,
            adhoc: 1
        }
    );
    assert_eq!(
        used.unfinished,
        CapacityPool {
            normal: 0,
            adhoc: 1
        }
    );
    let full = store.get(&task.id).unwrap();
    assert_eq!(full.capacity_lane, CapacityLane::Adhoc);
    assert_eq!(CapacityUsage::from_tasks(&[full]), used);
    // Cancelling/replacing a run is not abandoning its goal. Even a field reset cannot
    // accidentally erase the committed position while the task remains non-terminal.
    store
        .update(&task.id, |task| {
            task.status = Status::Ready;
            task.capacity_started = None;
            task.review_reservation = None;
            task.current_run_mut().unwrap().outcome = Some(RunOutcome::Superseded);
            task.plan_runs.push(
                serde_json::from_value(json!({
                    "id":"D1", "profile":"fake", "harness":"fake", "spec_revision":"spec",
                    "started":"2026-01-01T00:00:00Z", "output_dir":"/tmp/capacity-fixture-plan",
                    "rounds":0, "state":"cancelled"
                }))
                .unwrap(),
            );
            Ok(())
        })
        .unwrap();
    let head = store.head(&task.id).unwrap();
    assert_eq!(head.capacity_started, Some(true));
    assert_eq!(
        CapacityUsage::from_heads(std::slice::from_ref(&head))
            .unfinished
            .total(),
        1
    );
    assert!(
        admission::unfinished_reason(&store, &head)
            .unwrap()
            .is_none()
    );
    let next = store
        .create("not started", None, Status::Backlog, "")
        .unwrap();
    assert!(
        admission::unfinished_reason(&store, &store.head(&next.id).unwrap())
            .unwrap()
            .is_some()
    );
    store
        .update(&task.id, |task| {
            task.status = Status::Dropped;
            Ok(())
        })
        .unwrap();
    store
        .update(&task.id, |task| {
            task.status = Status::Backlog;
            Ok(())
        })
        .unwrap();
    let head = store.head(&task.id).unwrap();
    assert_eq!(head.capacity_started, Some(false));
    assert!(
        !unfinished(&head),
        "reopening a completed/abandoned goal does not reactivate its old history"
    );
    store
        .update(&task.id, |task| {
            task.launch = Some(serde_json::from_value(
                json!({"stage":"submitting", "reserved":false}),
            )?);
            Ok(())
        })
        .unwrap();
    let pending = CapacityUsage::from_heads(&[store.head(&task.id).unwrap()]);
    assert_eq!(
        (pending.agents.total(), pending.unfinished.total()),
        (1, 1),
        "an uncertain submission keeps both positions even without a reserved flag"
    );
    store
        .update(&task.id, |task| {
            task.launch = None;
            Ok(())
        })
        .unwrap();
    let cleared = CapacityUsage::from_heads(&[store.head(&task.id).unwrap()]);
    assert_eq!((cleared.agents.total(), cleared.unfinished.total()), (0, 0));
}
