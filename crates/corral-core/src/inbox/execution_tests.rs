use super::*;
use crate::Store;
use crate::inbox::queue_test_support::*;
use crate::inbox::{Receipt, ReceiptLevel};

#[test]
fn queue_evidence_replay_is_monotonic_and_independent_of_observation() {
    let (root, store, recipient) = fixture("pm");
    let inbox = Inbox::new(&store, recipient.clone()).unwrap();
    inbox.post(post("original")).unwrap();
    let scope = scope(&inbox);
    inbox.track_execution(&scope).unwrap();
    let queued = queued(&scope, 1);
    assert!(inbox.record_execution(&queued).unwrap());
    assert!(inbox.entry(1).unwrap().observed.is_none());
    let receipt = Receipt {
        envelope: Some(scope.envelope),
        binding: Some(scope.binding.clone()),
        observed_at: store.now(),
        via: "corral-pi".into(),
        detail: json!({}),
    };
    inbox
        .record_receipt(1, ReceiptLevel::Observed, &receipt)
        .unwrap();
    assert_eq!(inbox.pending_executions(Some(&scope)).unwrap().len(), 1);
    let started = ExecutionRecord {
        state: ExecutionState::Started,
        turn: Some(8),
        ..queued.clone()
    };
    assert!(inbox.record_execution(&started).unwrap());
    drop(store);
    let reopened = Store::open(root.path()).unwrap();
    let inbox = Inbox::new(&reopened, recipient).unwrap();
    let events = reopened.events().unwrap();
    assert!(!inbox.record_execution(&queued).unwrap());
    assert!(!inbox.record_execution(&started).unwrap());
    assert_eq!(reopened.events().unwrap(), events);
    let withdrawn = ExecutionRecord {
        state: ExecutionState::Withdrawn,
        op_key: Some("other".into()),
        ..queued
    };
    assert!(inbox.record_execution(&withdrawn).is_err());
    assert_eq!(inbox.entry(1).unwrap().execution, Some(started));
    assert_eq!(inbox.entry(1).unwrap().observed, Some(receipt));
    assert!(inbox.pending_executions(None).unwrap().is_empty());
    assert!(
        reopened
            .events()
            .unwrap()
            .iter()
            .any(|event| event.kind == "queue_execution_conflict")
    );
    assert_eq!(inbox.records("execution-events").unwrap().len(), 2);
}

#[test]
fn legacy_and_unknown_evidence_do_not_invent_execution_or_definite_outcomes() {
    let (_root, store, recipient) = fixture("pm");
    let inbox = Inbox::new(&store, recipient).unwrap();
    let legacy = inbox.post(post("legacy")).unwrap();
    let mut serialized = serde_json::to_value(&legacy).unwrap();
    assert!(serialized.get("supersedes").is_none());
    serialized.as_object_mut().unwrap().remove("supersedes");
    assert_eq!(
        serde_json::from_value::<super::super::Message>(serialized)
            .unwrap()
            .supersedes,
        None
    );
    assert!(inbox.entry(1).unwrap().execution.is_none());
    assert!(inbox.pending_executions(None).unwrap().is_empty());
    let scope = scope(&inbox);
    let queued = queued(&scope, 1);
    let unknown = ExecutionRecord {
        state: ExecutionState::Unknown,
        ..queued.clone()
    };
    inbox.record_execution(&unknown).unwrap();
    assert_eq!(inbox.pending_executions(None).unwrap().len(), 1);
    inbox.record_execution(&queued).unwrap();
    assert!(!inbox.record_execution(&unknown).unwrap());
    let handled = ExecutionRecord {
        state: ExecutionState::Handled,
        ..queued.clone()
    };
    inbox.record_execution(&handled).unwrap();
    assert!(!inbox.record_execution(&queued).unwrap());
    assert!(inbox.pending_executions(None).unwrap().is_empty());
    let foreign = ExecutionRecord {
        scope: ExecutionScope {
            bind: "b".repeat(64),
            ..scope
        },
        ..queued
    };
    assert!(inbox.record_execution(&foreign).is_err());
    assert_eq!(inbox.execution_view(1).unwrap(), Some(handled));
}
