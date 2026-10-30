use super::*;
use crate::{
    DecisionRecord, DecisionRequest, ObligationSource, ReceiptOutcome, ResumeDelivery,
    ResumeReceipt, WaitObservation, WaitRequest,
};

fn at() -> Timestamp {
    "2026-10-28T03:00:00Z".parse().unwrap()
}

fn channel(id: &str) -> WaitHandle {
    WaitHandle::Channel {
        channel_id: id.into(),
        capability: Some("wait_v1".into()),
    }
}

fn wait(id: &str, handle: WaitHandle) -> WaitRecord {
    WaitRecord {
        request: WaitRequest {
            version: 1,
            id: id.into(),
            run: "R1".into(),
            condition: "lands".into(),
            owner: "worker".into(),
            source: ObligationSource::Worker,
            deadline: Some(at()),
            handle,
        },
        registered_at: at(),
        observation: WaitObservation {
            at: at(),
            state: WaitState::Pending,
            detail: String::new(),
        },
        receipt: None,
    }
}

#[test]
fn channel_ids_name_the_awaited_task() {
    for (id, task) in [
        ("task:T-512:landed", Some("T-512")),
        ("worker:T-512-landed", Some("T-512")),
        ("corral:T-506:landed-and-installed", Some("T-506")),
        ("T-1043", Some("T-1043")),
        ("CI-1", None),
        ("worker:ci-42", None),
        ("T-12", None),
        ("mon_Z87AFVNW7FZJF884", None),
        ("worker:T-512x", None),
    ] {
        assert_eq!(awaited_task(&channel(id)), task, "{id}");
    }
    let gate = WaitHandle::Gate {
        sha: "abc".into(),
        pane: "p".into(),
    };
    assert_eq!(awaited_task(&gate), None);
}

#[test]
fn only_open_task_waits_park_a_run() {
    let mut o = RunObligations {
        waits: vec![
            wait("a", channel("task:T-512:landed")),
            wait("b", channel("worker:T-512-installed")),
            wait("c", channel("task:T-506:landed")),
        ],
        ..RunObligations::default()
    };
    assert_eq!(o.parked_on("T-505"), ["T-512", "T-506"]);
    // A wait on the run's own task or on anything that is not a task keeps the slot.
    assert!(o.parked_on("T-512").is_empty());
    o.waits.push(wait("ci", channel("worker:ci-42")));
    assert!(o.parked_on("T-505").is_empty());
    o.waits.pop();
    o.decisions.push(DecisionRecord {
        request: DecisionRequest {
            version: 1,
            id: "q".into(),
            run: "R1".into(),
            question: "?".into(),
            requested_action: "answer".into(),
            owner: "owner".into(),
            source: ObligationSource::Worker,
            deadline: None,
        },
        registered_at: at(),
        observation: o.waits[0].observation.clone(),
        receipt: None,
    });
    assert!(o.parked_on("T-505").is_empty());
}

#[test]
fn a_fired_wait_queues_the_run_for_a_slot() {
    let mut o = RunObligations {
        waits: vec![wait("a", channel("task:T-512:landed"))],
        ..RunObligations::default()
    };
    let before = o.parked_on("T-505");
    o.waits[0].receipt = Some(ResumeReceipt {
        version: 1,
        id: "landed".into(),
        run: "R1".into(),
        outcome: ReceiptOutcome::Ready,
        delivery: ResumeDelivery::Acknowledged,
        observed_at: at(),
        answer: None,
        data: serde_json::Value::Null,
    });
    o.settle_parking(&before, "T-505", at());
    assert_eq!(
        o.slot_wait,
        Some(SlotWait {
            since: at(),
            on: vec!["T-512".into()]
        })
    );
    assert!(!o.is_empty());
    // Parking again on a new task drops the stale slot request.
    o.waits.push(wait("b", channel("task:T-520:landed")));
    o.settle_parking(&[], "T-505", at());
    assert_eq!(o.slot_wait, None);
}
