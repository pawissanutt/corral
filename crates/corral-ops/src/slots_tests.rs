use super::*;
use crate::SlotUsage;
use corral_core::{GateRecord, ReviewWork, RunOutcome};

fn full_workers(review_limit: usize) -> (Fixture, Vec<String>) {
    let mut fx = Fixture::new(review_limit);
    fx.ctx.store.config.limits.running = 2;
    fx.ctx.store.config.limits.count_gates = true;
    fx.ctx
        .store
        .config
        .limits
        .per_project
        .insert("app".into(), 2);
    fx.ctx.store.save_config().unwrap();
    let ids: Vec<_> = (0..2).map(|_| fx.task()).collect();
    for id in &ids {
        fx.ctx
            .store
            .update(id, |task| {
                task.status = Status::Running;
                Ok(())
            })
            .unwrap();
    }
    (fx, ids)
}

fn gate() -> GateRecord {
    serde_json::from_value(json!({
        "result": "running", "at": "2026-10-15T00:00:00Z", "sha": "head"
    }))
    .unwrap()
}

#[test]
fn single_agent_and_gate_pools_allow_one_unfinished_task_to_finish() {
    let mut fx = Fixture::new(1);
    fx.ctx.store.config.limits.capacity = Some(corral_core::CapacityLimits {
        unfinished: 1,
        agents: 1,
        gates: 1,
        adhoc_reserve: 0,
    });
    fx.ctx.store.save_config().unwrap();
    let id = fx.task();
    fx.ctx
        .store
        .update(&id, |task| {
            task.status = Status::Running;
            Ok(())
        })
        .unwrap();
    let gate = ReviewLaunch::reserve(&fx.ctx, &id, Kind::Gate).unwrap();
    let usage = crate::slot_usage(&fx.ctx).unwrap().independent.unwrap();
    assert_eq!(
        (usage.agents.total(), usage.gates, usage.unfinished.total()),
        (1, 1, 1)
    );
    drop(gate);
    // The worker settles before review. Its existing unfinished position is retained.
    fx.ctx
        .store
        .update(&id, |task| {
            task.status = Status::Review;
            task.current_run_mut().unwrap().outcome = Some(RunOutcome::Done);
            Ok(())
        })
        .unwrap();
    assert!(
        crate::wip::reason(&fx.ctx.store, &fx.ctx.store.get(&id).unwrap())
            .unwrap()
            .is_none()
    );
    let reviewer = ReviewLaunch::reserve(&fx.ctx, &id, Kind::Reviewer).unwrap();
    let usage = crate::slot_usage(&fx.ctx).unwrap().independent.unwrap();
    assert_eq!(
        (usage.agents.total(), usage.gates, usage.unfinished.total()),
        (1, 0, 1)
    );
    drop(reviewer);
    fx.ctx
        .store
        .update(&id, |task| {
            task.status = Status::Done;
            Ok(())
        })
        .unwrap();
    assert_eq!(
        crate::slot_usage(&fx.ctx).unwrap().independent.unwrap(),
        crate::capacity::CapacityUsage::default()
    );
}

#[test]
fn current_gate_reservation_shares_full_worker_capacity() {
    // Given full global and project worker capacity with gate counting enabled.
    let (fx, ids) = full_workers(2);

    // When a worker reserves its current-run gate.
    let launch = ReviewLaunch::reserve(&fx.ctx, &ids[0], Kind::Gate).unwrap();

    // Then the reservation needs no additional global or project slot.
    let usage = crate::slot_usage(&fx.ctx).unwrap();
    assert_eq!(usage.tasks, 2);
    assert_eq!(usage.reservations, 0);
    assert_eq!(usage.total(), 2);
    drop(launch);
}

#[test]
fn current_running_gates_share_full_worker_capacity() {
    // Given two full worker slots and current-run gate processes.
    let (fx, ids) = full_workers(2);
    let mut tasks = fx.ctx.store.list().unwrap();
    for task in &mut tasks {
        task.current_run_mut().unwrap().gate = Some(gate());
    }

    // When usage is computed with gates counted.
    let usage = SlotUsage::from_tasks(&tasks, &fx.ctx.store.config.limits);

    // Then gates remain visible for review admission but do not double count workers.
    assert_eq!(usage.tasks, ids.len());
    assert_eq!(usage.gates, 2);
    assert_eq!(usage.total(), 2);
}

#[test]
fn review_gate_and_reservation_do_not_share_worker_slots() {
    // Given full workers and a separate Review task.
    let (fx, _) = full_workers(2);
    let review = fx.task();
    let mut task = fx.ctx.store.get(&review).unwrap();
    task.review_reservation = Some(ReviewWork::Gate);
    let limits = &fx.ctx.store.config.limits;

    // When the Review task reserves a gate and later records the process.
    let reserved = SlotUsage::from_tasks(std::slice::from_ref(&task), limits);
    task.current_run_mut().unwrap().gate = Some(gate());
    let running = SlotUsage::from_tasks(&[task], limits);

    // Then both forms consume an independent slot, and full workers deny admission.
    assert_eq!(reserved.reservations, 1);
    assert_eq!(reserved.total(), 1);
    assert_eq!(running.gates, 1);
    assert_eq!(running.reservations, 0);
    assert_eq!(running.total(), 1);
    let error = ReviewLaunch::reserve(&fx.ctx, &review, Kind::Gate)
        .err()
        .unwrap();
    assert!(error.downcast_ref::<CapacityWait>().is_some(), "{error:#}");
}

#[test]
fn historical_gate_does_not_share_current_worker_slot() {
    // Given a current worker whose older run still holds a gate slot.
    let (fx, ids) = full_workers(2);
    let mut task = fx.ctx.store.get(&ids[0]).unwrap();
    let mut previous = task.current_run().unwrap().clone();
    previous.id = "R0".into();
    previous.gate = Some(gate());
    task.runs.insert(0, previous);

    // When slot usage is computed.
    let usage = SlotUsage::from_tasks(&[task], &fx.ctx.store.config.limits);

    // Then the older gate occupies an independent slot.
    assert_eq!(usage.tasks, 1);
    assert_eq!(usage.gates, 1);
    assert_eq!(usage.total(), 2);
}

#[test]
fn failed_worker_cannot_share_gate_slot() {
    // Given a Running task whose failed run no longer holds a worker slot.
    let (fx, ids) = full_workers(2);
    let mut task = fx.ctx.store.get(&ids[0]).unwrap();
    let run = task.current_run_mut().unwrap();
    run.outcome = Some(RunOutcome::Failed);
    run.gate = Some(gate());

    // When slot usage is computed.
    let usage = SlotUsage::from_tasks(&[task], &fx.ctx.store.config.limits);

    // Then the gate still consumes its independent slot.
    assert_eq!(usage.tasks, 0);
    assert_eq!(usage.gates, 1);
    assert_eq!(usage.total(), 1);
}

#[test]
fn shared_worker_gate_still_obeys_review_limit() {
    // Given full workers and an occupied review process slot.
    let (fx, ids) = full_workers(1);
    crate::gate::gate_start(&fx.ctx, &ids[0]).unwrap();

    // When a worker tries reserving its gate.
    let error = ReviewLaunch::reserve(&fx.ctx, &ids[1], Kind::Gate)
        .err()
        .unwrap();

    // Then sharing cannot bypass the review limit.
    let waiting = error.downcast_ref::<CapacityWait>().unwrap();
    assert_eq!(waiting.reason, "review process limit reached (1/1)");
    assert_eq!(crate::slot_usage(&fx.ctx).unwrap().total(), 2);
}

#[test]
fn shared_worker_gate_still_obeys_resource_admission() {
    // Given full workers and a deterministic low-memory observation.
    let (mut fx, ids) = full_workers(2);
    let mut probe = crate::resources::FixedResourceProbe::healthy();
    probe.0.mem_available = 0;
    fx.ctx.resource_probe = std::sync::Arc::new(probe);

    // When a worker tries reserving its gate.
    let error = ReviewLaunch::reserve(&fx.ctx, &ids[0], Kind::Gate)
        .err()
        .unwrap();

    // Then sharing cannot bypass machine admission.
    let waiting = error.downcast_ref::<CapacityWait>().unwrap();
    assert!(waiting.reason.starts_with("waiting: machine busy"));
    assert_eq!(crate::slot_usage(&fx.ctx).unwrap().total(), 2);
}

#[test]
fn reviewer_does_not_share_full_worker_capacity() {
    // Given full workers with reviewer counting enabled.
    let (mut fx, ids) = full_workers(2);
    fx.ctx.store.config.limits.count_reviews = true;
    fx.ctx.store.save_config().unwrap();

    // When a worker requests an additional reviewer.
    let error = ReviewLaunch::reserve(&fx.ctx, &ids[0], Kind::Reviewer)
        .err()
        .unwrap();

    // Then only gates qualify for worker slot sharing.
    assert!(error.downcast_ref::<CapacityWait>().is_some(), "{error:#}");
}
