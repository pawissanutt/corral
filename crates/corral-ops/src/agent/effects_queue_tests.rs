//! Admission waits never require another model turn.
use super::*;

pub(super) fn queued(fx: &Fx, o: &RequestOrigin) {
    let lease = fx
        .ctx
        .store
        .try_lock_shared("session-ownership")
        .unwrap()
        .unwrap();
    let response = effects::gate(&fx.ctx, o).unwrap().to_json().unwrap();
    assert_eq!(response["state"], "queued", "{response}");
    drop(lease);
}

fn wake_state(fx: &Fx, o: &RequestOrigin) -> WakeState {
    core_agent::container_state(&fx.task(), &o.binding.container)
        .unwrap()
        .wakes
        .iter()
        .find(|w| w.request_id == o.request_id)
        .unwrap()
        .state
}

#[test]
fn queued_gate_survives_reconciliation_and_replays_without_another_request() {
    // Given a worker whose gate cannot acquire the pane-inventory lease.
    let fx = fixture(true);
    let o = fx.call(fx.bind(), "queued", "gate");
    queued(&fx, &o);
    // When sync reconciles the request and the same call is replayed.
    effects::reconcile_task(&fx.ctx, &fx.id).unwrap();
    effects::gate(&fx.ctx, &o).unwrap();
    // Then the request and its wake remain pending, without starting a gate.
    assert_eq!(fx.receipt(&o).unwrap().state, ReceiptState::Queued);
    assert_eq!(wake_state(&fx, &o), WakeState::PendingStart);
    assert_eq!(fx.task().review_wait, Some(Kind::Gate));
    assert_eq!(fx.calls("pane run"), 0);
    let task = fx.task();
    let state = core_agent::container_state(&task, &o.binding.container).unwrap();
    let turn = serde_json::from_value(json!({
        "seq": 1, "generation": o.identity.generation,
        "origin": {"kind": "bridge"}, "started": "2026-01-01T00:00:00Z",
        "ended": "2026-01-01T00:01:00Z"
    }))
    .unwrap();
    assert_eq!(
        super::super::turns::class_at_end(state, None, &turn),
        core_agent::TurnClass::WaitingExternal
    );
}

#[test]
fn queued_gate_is_admitted_once_when_the_lease_is_released() {
    // Given a queued gate.
    let fx = fixture(true);
    let o = fx.call(fx.bind(), "admit", "gate");
    queued(&fx, &o);
    // When two scheduler passes admit it.
    super::super::gate_queue::advance(&fx.ctx).unwrap();
    super::super::gate_queue::advance(&fx.ctx).unwrap();
    // Then only one gate was launched and the existing completion wake is armed.
    assert_eq!(fx.receipt(&o).unwrap().state, ReceiptState::Committed);
    assert_eq!(fx.calls("pane run"), 1);
    assert_eq!(wake_state(&fx, &o), WakeState::Armed);
    assert_eq!(fx.task().review_wait, None);
}

#[test]
fn dirty_candidate_after_queueing_posts_one_actionable_wake() {
    // Given a queued gate whose candidate is no longer clean.
    let fx = fixture(true);
    let o = fx.call(fx.bind(), "dirty", "gate");
    queued(&fx, &o);
    std::fs::write(fx.root().join("app/tracked"), "dirty\n").unwrap();
    // When admission is attempted twice.
    super::super::gate_queue::advance(&fx.ctx).unwrap();
    super::super::gate_queue::advance(&fx.ctx).unwrap();
    // Then no gate runs and exactly one terminal wake has been posted.
    let receipt = fx.receipt(&o).unwrap();
    assert_eq!(receipt.state, ReceiptState::Rejected);
    assert_eq!(receipt.result["outcome"], "not_started");
    assert_eq!(wake_state(&fx, &o), WakeState::Fired);
    assert_eq!(fx.calls("pane run"), 0);
    assert_eq!(
        fx.ctx
            .store
            .events()
            .unwrap()
            .iter()
            .filter(|e| e.kind == "agent_wake_posted")
            .count(),
        1
    );
}

#[test]
fn fenced_queued_gate_is_abandoned_without_launching_or_waking() {
    // Given a queued request from a now-fenced generation.
    let fx = fixture(true);
    let o = fx.call(fx.bind(), "fenced", "gate");
    queued(&fx, &o);
    fence_generation(
        &fx.ctx.store,
        &o.binding.container,
        o.binding.generation,
        "test",
    )
    .unwrap();
    // When sync reconciles and admits queued work.
    effects::reconcile_task(&fx.ctx, &fx.id).unwrap();
    super::super::gate_queue::advance(&fx.ctx).unwrap();
    // Then the obsolete request is cancelled.
    assert_eq!(fx.receipt(&o).unwrap().state, ReceiptState::Abandoned);
    assert_eq!(wake_state(&fx, &o), WakeState::Cancelled);
    assert_eq!(fx.calls("pane run"), 0);
    assert_eq!(fx.task().review_wait, None);
}

#[test]
fn second_gate_request_cannot_duplicate_a_queued_gate() {
    // Given an existing queued gate request.
    let fx = fixture(true);
    let generation = fx.bind();
    let o = fx.call(generation, "first", "gate");
    queued(&fx, &o);
    // When the model submits a different request.
    let error = effects::gate(&fx.ctx, &fx.call(generation, "second", "gate")).unwrap_err();
    // Then admission refuses it before creating a second receipt.
    assert!(error.to_string().contains("gate_queued"));
    assert_eq!(
        core_agent::container_state(&fx.task(), &o.binding.container)
            .unwrap()
            .receipts
            .len(),
        1
    );
}

#[test]
fn capacity_wait_retries_without_changing_the_request_or_waking_the_model() {
    // Given occupied review capacity, even though the worker itself has a slot.
    let mut fx = fixture(true);
    fx.ctx.store.config.limits.review = 1;
    fx.ctx.store.config.limits.count_gates = true;
    fx.ctx.store.save_config().unwrap();
    let other = fx
        .ctx
        .store
        .create("Other gate", Some("app"), Status::Review, "spec")
        .unwrap();
    let mut running = fx.task().current_run().unwrap().clone();
    running.outcome = Some(RunOutcome::Done);
    running.gate = Some(
        serde_json::from_value(json!({
            "result": "running", "at": jiff::Timestamp::now(), "sha": "head", "pane": "w0:p2",
            "socket": fx.ctx.herdr.socket_path().unwrap()
        }))
        .unwrap(),
    );
    std::fs::write(fx.root().join("w0:p2"), "").unwrap();
    fx.ctx
        .store
        .update(&other.id, |task| {
            task.runs.push(running.clone());
            Ok(())
        })
        .unwrap();
    let o = fx.call(fx.bind(), "capacity", "gate");
    effects::gate(&fx.ctx, &o).unwrap();
    assert_eq!(
        fx.receipt(&o).unwrap().state,
        ReceiptState::Queued,
        "{:?}",
        fx.receipt(&o)
    );
    // When the scheduler retries while capacity remains unavailable.
    super::super::gate_queue::advance(&fx.ctx).unwrap();
    // Then there is one pending request and no wake or gate process.
    assert_eq!(
        fx.receipt(&o).unwrap().state,
        ReceiptState::Queued,
        "{:?}",
        fx.receipt(&o)
    );
    assert_eq!(wake_state(&fx, &o), WakeState::PendingStart);
    assert_eq!(fx.calls("pane run"), 0);
    assert_eq!(
        core_agent::container_state(&fx.task(), &o.binding.container)
            .unwrap()
            .receipts
            .len(),
        1
    );
}

#[test]
fn admitted_gate_completion_posts_exactly_one_existing_result_wake() {
    // Given an admitted gate that has finished.
    let fx = fixture(true);
    let o = fx.call(fx.bind(), "complete", "gate");
    queued(&fx, &o);
    super::super::gate_queue::advance(&fx.ctx).unwrap();
    fx.ctx
        .store
        .update(&fx.id, |task| {
            task.current_run_mut()
                .unwrap()
                .gate
                .as_mut()
                .unwrap()
                .result = corral_core::GateResult::Fail;
            Ok(())
        })
        .unwrap();
    let now = jiff::Timestamp::now();
    let due = effects::due_gate_wakes(&fx.task(), now, std::time::Duration::from_secs(300));
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].reason, "fail");
    // When completion delivery is retried.
    let first = super::super::wake::post_due(&fx.ctx, &due).unwrap();
    let replay = super::super::wake::post_due(&fx.ctx, &due).unwrap();
    // Then the existing durable inbox machinery emits one wake.
    assert_eq!((first, replay), (1, 0));
    assert_eq!(wake_state(&fx, &o), WakeState::Fired);
}

#[test]
fn sync_admits_queued_gate_after_releasing_its_inventory_lease() {
    // Given a queued worker gate and a healthy fake pane inventory.
    let fx = fixture(true);
    let o = fx.call(fx.bind(), "sync-admit", "gate");
    queued(&fx, &o);
    // When the actual sync pipeline runs.
    crate::sync_at(&fx.ctx, jiff::Timestamp::now()).unwrap();
    // Then admission is no longer rejected by sync's own shared lease.
    assert_eq!(fx.receipt(&o).unwrap().state, ReceiptState::Committed);
    assert_eq!(fx.calls("pane run"), 1);
}
