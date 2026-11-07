use super::*;
use crate::Store;
use crate::inbox::queue_test_support::*;

fn request(scope: &ExecutionScope) -> QueueOperationRequest {
    QueueOperationRequest {
        members: vec![1],
        scope: scope.clone(),
        item: Some("q-g3-aaaaaaaaaaaa-1".into()),
        operation: QueueOperationKind::Edit {
            replacement: QueueReplacement::new("corrected".into(), MAX_PAYLOAD),
        },
        actor: "owner".into(),
        reason: "correction".into(),
        expected: Some(ExecutionState::Queued),
    }
}

fn crash<T>(point: &'static str, f: impl FnOnce() -> Result<T>) -> Result<T> {
    super::super::header::CRASH_AT.with(|slot| slot.set(Some(point)));
    let result = f();
    super::super::header::CRASH_AT.with(|slot| slot.set(None));
    result
}

#[test]
fn prepared_intent_and_atomic_edit_publication_survive_restart_boundaries() {
    for kind in ["pm", "assistant", "task"] {
        let (root, store, recipient) = fixture(kind);
        let inbox = Inbox::new(&store, recipient.clone()).unwrap();
        let original = inbox.post(post("original")).unwrap();
        let scope = scope(&inbox);
        let envelope = inbox.envelope(scope.envelope).unwrap();
        let payload = inbox.payload(&envelope).unwrap();
        inbox.record_execution(&queued(&scope, 1)).unwrap();
        let request = request(&scope);
        assert!(
            crash("queue_operation_prepare", || inbox
                .prepare_queue_operation(request.clone()))
            .is_err()
        );
        assert!(inbox.open_queue_operations().unwrap().is_empty());
        assert!(
            inbox
                .records("queue-operation-requests")
                .unwrap()
                .is_empty()
        );
        let intent = inbox.prepare_queue_operation(request.clone()).unwrap();
        assert_eq!(intent.number, 1);
        assert_eq!(
            inbox.prepare_queue_operation(request.clone()).unwrap(),
            intent
        );
        let mut changed = request;
        changed.reason = "different operation".into();
        assert!(inbox.prepare_queue_operation(changed).is_err());
        // Host I/O would run here, outside any Store unit. Reopen after preparing and
        // after each failed/successful publication boundary, without a timing crash.
        drop(store);
        let reopened = Store::open(root.path()).unwrap();
        let inbox = Inbox::new(&reopened, recipient.clone()).unwrap();
        assert_eq!(inbox.open_queue_operations().unwrap(), vec![intent.clone()]);
        assert_eq!(
            Inbox::queue_work_recipients(&reopened).unwrap(),
            vec![recipient.clone()]
        );
        inbox
            .record_execution(&ExecutionRecord {
                state: ExecutionState::Withdrawn,
                op_key: Some(intent.op_key.clone()),
                ..queued(&scope, 1)
            })
            .unwrap();
        let before = inbox.state().unwrap();
        assert!(
            crash("queue_edit_commit", || inbox
                .commit_queue_edit(&intent.op_key, |_, _| Ok(())))
            .is_err()
        );
        assert_eq!(inbox.state().unwrap(), before);
        assert!(inbox.message_optional(2).unwrap().is_none());
        assert!(
            inbox
                .queue_operation_outcome(&intent.op_key)
                .unwrap()
                .is_none()
        );
        assert_eq!(inbox.open_queue_operations().unwrap(), vec![intent.clone()]);
        let replacement = inbox
            .commit_queue_edit(&intent.op_key, |_, _| Ok(()))
            .unwrap();
        assert_eq!(
            replacement.supersedes.as_deref(),
            Some(original.id.as_str())
        );
        assert_eq!(replacement.seq, 2);
        assert_eq!(
            replacement.body,
            format!("[corral {}#2 note]\ncorrected", inbox.key())
        );
        assert_eq!(inbox.message(1).unwrap(), original);
        assert_eq!(inbox.envelope(scope.envelope).unwrap(), envelope);
        assert_eq!(inbox.payload(&envelope).unwrap(), payload);
        let outcome = inbox
            .queue_operation_outcome(&intent.op_key)
            .unwrap()
            .unwrap();
        drop(reopened);
        let reopened = Store::open(root.path()).unwrap();
        let inbox = Inbox::new(&reopened, recipient).unwrap();
        assert_eq!(
            inbox
                .commit_queue_edit(&intent.op_key, |_, _| anyhow::bail!(
                    "must not rerun admission"
                ))
                .unwrap(),
            replacement
        );
        assert_eq!(
            inbox.queue_operation_outcome(&intent.op_key).unwrap(),
            Some(outcome)
        );
        assert!(inbox.open_queue_operations().unwrap().is_empty());
        assert!(Inbox::queue_work_recipients(&reopened).unwrap().is_empty());
        assert_eq!(inbox.messages().unwrap().len(), 2);
        assert_eq!(
            reopened
                .events()
                .unwrap()
                .iter()
                .filter(|event| event.kind == "message_posted")
                .count(),
            2
        );
    }
}

#[test]
fn start_first_and_blocked_replacement_outcomes_never_publish_and_cannot_be_overwritten() {
    for withdrawn in [false, true] {
        let (_root, store, recipient) = fixture("pm");
        let inbox = Inbox::new(&store, recipient).unwrap();
        inbox.post(post("original")).unwrap();
        let scope = scope(&inbox);
        inbox.record_execution(&queued(&scope, 1)).unwrap();
        let intent = inbox.prepare_queue_operation(request(&scope)).unwrap();
        // No canonical withdrawal owned by this operation: no replacement is possible.
        assert!(
            inbox
                .commit_queue_edit(&intent.op_key, |_, _| Ok(()))
                .is_err()
        );
        let outcome = QueueOperationOutcome {
            version: 1,
            op_key: intent.op_key.clone(),
            withdrawal: if withdrawn {
                QueueWithdrawalOutcome::Withdrawn
            } else {
                QueueWithdrawalOutcome::AlreadyStarted { turn: Some(4) }
            },
            replacement: withdrawn.then(|| QueueReplacementOutcome::Blocked {
                reason: "generation changed".into(),
            }),
        };
        assert!(inbox.settle_queue_operation(&outcome).unwrap());
        assert!(!inbox.settle_queue_operation(&outcome).unwrap());
        assert!(
            inbox
                .commit_queue_edit(&intent.op_key, |_, _| Ok(()))
                .is_err()
        );
        let changed = QueueOperationOutcome {
            withdrawal: QueueWithdrawalOutcome::UnknownItem,
            replacement: None,
            ..outcome.clone()
        };
        assert!(inbox.settle_queue_operation(&changed).is_err());
        assert_eq!(
            inbox.queue_operation_outcome(&intent.op_key).unwrap(),
            Some(outcome)
        );
        assert_eq!(inbox.messages().unwrap().len(), 1);
        assert!(inbox.open_queue_operations().unwrap().is_empty());
    }
}

#[test]
fn pending_execution_and_open_operations_decode_only_outstanding_work() {
    use crate::work::{Work, count};
    for history in [2, 4] {
        let (_root, store, recipient) = fixture("pm");
        let inbox = Inbox::new(&store, recipient).unwrap();
        for seq in 1..=history + 1 {
            inbox.post(post("small history")).unwrap();
            let scope = scope(&inbox);
            inbox.track_execution(&scope).unwrap();
            let queued = queued(&scope, seq);
            inbox.record_execution(&queued).unwrap();
            let request = QueueOperationRequest {
                members: vec![seq],
                operation: QueueOperationKind::Withdraw,
                ..request(&scope)
            };
            let intent = inbox.prepare_queue_operation(request).unwrap();
            if seq <= history {
                inbox
                    .record_execution(&ExecutionRecord {
                        state: ExecutionState::Withdrawn,
                        op_key: Some(intent.op_key.clone()),
                        ..queued
                    })
                    .unwrap();
                inbox
                    .settle_queue_operation(&QueueOperationOutcome {
                        version: 1,
                        op_key: intent.op_key,
                        withdrawal: QueueWithdrawalOutcome::Withdrawn,
                        replacement: None,
                    })
                    .unwrap();
                inbox
                    .mark_terminal(seq, TerminalState::Cancelled, "fixture", "fixture")
                    .unwrap();
                inbox
                    .close_settled(&inbox.envelope(scope.envelope).unwrap())
                    .unwrap();
            }
        }
        let before = [
            Work::InboxRecordRead,
            Work::InboxMessageRead,
            Work::InboxEnvelopeRead,
        ]
        .map(count);
        assert_eq!(inbox.pending_executions(None).unwrap().len(), 1);
        assert_eq!(inbox.open_queue_operations().unwrap().len(), 1);
        let after = [
            Work::InboxRecordRead,
            Work::InboxMessageRead,
            Work::InboxEnvelopeRead,
        ]
        .map(count);
        assert_eq!(
            after
                .into_iter()
                .zip(before)
                .map(|(a, b)| a - b)
                .collect::<Vec<_>>(),
            vec![2, 0, 0]
        );
    }
}
