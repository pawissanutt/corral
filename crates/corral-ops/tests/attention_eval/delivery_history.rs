//! S21: cumulative proof through real delivery, owning sync, restart and FIFO promotion.
use corral_core::attention::Condition;
use corral_core::inbox::{
    Inbox, MAX_PAYLOAD, Message, OwnerMessage, Receipt, ReceiptLevel, Recipient, Recovery,
    RetryCoverage, RetryHistory, SendOutcome, TerminalState, WitnessReason,
};
use corral_core::work::{Work, count};
use corral_ops::attention;
use corral_ops::delivery::{DeliveryStatus, SessionBinding, deliver};
use serde_json::json;

use crate::delivery::{Transport, condition, post, rejected, world};
use crate::fixture::{Fx, at};
use crate::support::{invariants, stats, sync};

fn entry(fx: &Fx, recipient: &Recipient, seq: u64) -> OwnerMessage {
    Inbox::new(&fx.reopen().store, recipient.clone())
        .unwrap()
        .state()
        .unwrap()
        .owner_header
        .unwrap()
        .iter()
        .find(|entry| entry.seq == seq)
        .unwrap()
        .clone()
}

fn row_at(fx: &Fx, recipient: &Recipient, message: &Message, seconds: i64) -> Condition {
    attention::board(&fx.reopen(), at(seconds))
        .unwrap()
        .tasks
        .into_iter()
        .find(|row| row.task == recipient.task().unwrap())
        .unwrap()
        .assessment
        .conditions
        .into_iter()
        .find(|row| {
            row.key
                .as_ref()
                .is_some_and(|key| key.0.contains(&message.id))
        })
        .unwrap()
}

fn measured_entries(fx: &Fx, recipient: &Recipient, expected: usize) {
    let ctx = fx.reopen();
    let inbox = Inbox::new(&ctx.store, recipient.clone()).unwrap();
    let state = inbox.state().unwrap();
    let counters = [
        Work::InboxRecordRead,
        Work::InboxMessageRead,
        Work::InboxEnvelopeRead,
        Work::InboxList,
    ];
    let before = counters.map(count);
    assert_eq!(inbox.owner_entries(&state).unwrap().len(), expected);
    // Five keyed receipt/terminal probes per entry; the retry claim is mirrored in the failed slot.
    assert_eq!(
        counters
            .map(count)
            .into_iter()
            .zip(before)
            .map(|(after, before)| after - before)
            .collect::<Vec<_>>(),
        vec![u64::try_from(expected).unwrap() * 5, 0, 0, 0]
    );
}

fn metrics(fx: &Fx, row: &Condition, seconds: i64, retries: u64) {
    let stats = stats(fx, seconds);
    let key = &row.key.as_ref().unwrap().0;
    assert_eq!(stats.conditions[key].kind, row.kind);
    assert_eq!(stats.conditions[key].since, row.since);
    assert_eq!(
        stats.actions.by_condition.get(key).copied().unwrap_or(0),
        retries
    );
    // Fixture posts to the same recipient are fresh posts that may join earlier conditions;
    // the exact retry count is asserted on this condition's own key.
    assert_eq!(stats.actions.duplicates, 0);
    invariants(&stats);
}

fn exact_sends(transport: &Transport, recipients: &[Recipient; 4], index: usize, expected: usize) {
    for (other, recipient) in recipients.iter().enumerate() {
        assert_eq!(
            transport.sent_to(recipient),
            if other == index { expected } else { 0 },
            "{recipient:?}"
        );
    }
}

fn older_entries(fx: &Fx, recipient: &Recipient) {
    let ctx = fx.reopen();
    let inbox = Inbox::new(&ctx.store, recipient.clone()).unwrap();
    for _ in 0..8 {
        let message = post(&ctx, recipient, "older cancelled obligation");
        inbox
            .mark_terminal(message.seq, TerminalState::Cancelled, "fixture", "fixture")
            .unwrap();
    }
}

fn restart_and_promote(
    fx: &Fx,
    recipient: &Recipient,
    message: &Message,
    overflow: bool,
    seconds: i64,
) -> Condition {
    let before = row_at(fx, recipient, message, seconds);
    let proof = entry(fx, recipient, message.seq);
    metrics(fx, &before, seconds, 0);
    if overflow {
        let ctx = fx.reopen();
        let inbox = Inbox::new(&ctx.store, recipient.clone()).unwrap();
        let header = inbox.state().unwrap().owner_header.unwrap();
        assert_eq!(header.entries.len(), 8);
        assert_eq!(header.overflow_entries, vec![proof.clone()]);
        let board = attention::board(&ctx, at(seconds)).unwrap();
        assert!(
            board
                .tasks
                .iter()
                .flat_map(|row| &row.assessment.conditions)
                .any(|row| row.kind == "inbox_overflow"
                    && row.next_action.recipient.as_deref() == Some(&recipient.key().unwrap()))
        );
        // Observing #2 supersedes exactly the cancelled primary #1, freeing one slot.
        inbox
            .record_receipt(
                2,
                ReceiptLevel::Observed,
                &Receipt {
                    envelope: None,
                    binding: None,
                    observed_at: at(seconds),
                    via: "fixture".into(),
                    detail: json!({}),
                },
            )
            .unwrap();
        // Header maintenance is explicit in this public-path fixture; sync records its result.
        inbox.maintain_owner_header(at(seconds)).unwrap();
    }
    // When a new Ctx owns the next sync, reconcile the retained or promoted condition.
    sync(fx, seconds + 30);
    let after = row_at(fx, recipient, message, seconds + 30);
    assert_eq!(after.key, before.key);
    assert_eq!(after.since, before.since);
    assert_eq!(after.since_source, before.since_source);
    assert_eq!(after.age_secs, before.age_secs.map(|age| age + 30));
    assert_eq!(after.next_action, before.next_action);
    assert_eq!(entry(fx, recipient, message.seq), proof);
    if overflow {
        let header = Inbox::new(&fx.reopen().store, recipient.clone())
            .unwrap()
            .state()
            .unwrap()
            .owner_header
            .unwrap();
        assert_eq!(
            header
                .entries
                .iter()
                .map(|entry| entry.seq)
                .collect::<Vec<_>>(),
            (2..=9).collect::<Vec<_>>()
        );
        assert!(header.overflow_entries.is_empty());
        assert_eq!(header.overflow_since, None);
    }
    measured_entries(fx, recipient, if overflow { 8 } else { 1 });
    metrics(fx, &after, seconds + 30, 0);
    after
}

#[test]
fn s21_prior_unsafe_attempts_bar_rejected_resends_after_restart_and_fifo_promotion() {
    for index in 0..4 {
        for accepted in [false, true] {
            for attempts in [2, 10] {
                for overflow in [false, true] {
                    // Given an unsafe first attempt, on each primary and ninth recipient entry.
                    let (fx, recipients) = world();
                    let recipient = &recipients[index];
                    if overflow {
                        older_entries(&fx, recipient);
                    }
                    let ctx = fx.reopen();
                    let message = post(&ctx, recipient, "unsafe history");
                    let initial = if accepted {
                        SendOutcome::Accepted
                    } else {
                        SendOutcome::Uncertain {
                            reason: "acknowledgement lost".into(),
                        }
                    };
                    let transport = Transport::new(initial);
                    // When owner resend creates E2 (or ten total attempts), the last rejects.
                    for attempt in 1..=attempts {
                        if attempt == attempts {
                            transport.set(rejected());
                        }
                        assert_eq!(
                            deliver(&fx.reopen(), recipient, &transport, None).unwrap(),
                            DeliveryStatus::Sent { envelope: attempt }
                        );
                        sync(&fx, i64::try_from(attempt).unwrap());
                        if attempt < attempts {
                            Inbox::new(&fx.reopen().store, recipient.clone())
                                .unwrap()
                                .recover(Recovery::Resend)
                                .unwrap();
                        }
                    }
                    let proof = entry(&fx, recipient, message.seq);
                    assert_eq!(proof.retry_evidence.coverage, RetryCoverage::Complete);
                    let RetryHistory::Barred { first } = &proof.retry_evidence.history else {
                        panic!("lost unsafe history: {proof:?}");
                    };
                    assert_eq!(
                        first.reason,
                        if accepted {
                            WitnessReason::Accepted
                        } else {
                            WitnessReason::Uncertain
                        }
                    );
                    let first_attempt = first.attempt.unwrap();
                    assert_eq!((first_attempt.envelope, first_attempt.ordinal), (1, 1));
                    assert_eq!(proof.envelope, Some(attempts));
                    assert_eq!(proof.delivery.as_ref().unwrap().attempt.unwrap().ordinal, 1);
                    assert!(!proof.retry_evidence.permits(TerminalState::Failed));
                    let row = restart_and_promote(
                        &fx,
                        recipient,
                        &message,
                        overflow,
                        i64::try_from(attempts).unwrap(),
                    );
                    // Then the rejected latest state never erases the earlier unsafe witness.
                    assert_eq!(row.kind, "delivery_failed");
                    assert_eq!(
                        row.next_action.command,
                        Some(format!("corral inbox inspect-retry {}", message.id))
                    );
                    assert!(row.next_action.requires_owner);
                    let reopened = fx.reopen();
                    assert!(
                        !Inbox::new(&reopened.store, recipient.clone())
                            .unwrap()
                            .inspect_retry(message.seq)
                            .unwrap()
                            .eligible
                    );
                    assert!(
                        corral_ops::delivery::retry(&reopened.store, recipient, message.seq)
                            .is_err()
                    );
                    assert_eq!(
                        deliver(&reopened, recipient, &transport, None).unwrap(),
                        DeliveryStatus::Idle
                    );
                    exact_sends(
                        &transport,
                        &recipients,
                        index,
                        usize::try_from(attempts).unwrap(),
                    );
                    sync(&fx, 60);
                    metrics(&fx, &row, 60, 0);
                }
            }
        }
    }
}

#[test]
fn s21_ninth_first_rejection_retries_once_after_owning_sync_fifo_promotion() {
    for index in 0..4 {
        // Given eight retained entries and a ninth first definitive rejection.
        let (fx, recipients) = world();
        let recipient = &recipients[index];
        older_entries(&fx, recipient);
        let ctx = fx.reopen();
        let message = post(&ctx, recipient, "safe ninth");
        let transport = Transport::new(rejected());
        deliver(&ctx, recipient, &transport, None).unwrap();
        sync(&fx, 0);
        let row = restart_and_promote(&fx, recipient, &message, true, 0);
        assert_eq!(row.kind, "delivery_failed");
        assert_eq!(
            row.next_action.command,
            Some(format!("corral inbox retry {}", message.id))
        );
        // When the exact-ID retry executes on a reopened store.
        let reopened = fx.reopen();
        let successor =
            corral_ops::delivery::retry(&reopened.store, recipient, message.seq).unwrap();
        assert_eq!(successor.recipient, *recipient);
        transport.set(SendOutcome::Accepted);
        deliver(&reopened, recipient, &transport, None).unwrap();
        sync(&fx, 60);
        // Then it sends exactly once more only to the original recipient and joins its key.
        exact_sends(&transport, &recipients, index, 2);
        metrics(&fx, &row, 60, 1);
    }
}

#[test]
fn s21_unattempted_replacements_retry_original_recipient_after_restart_and_promotion() {
    for index in 0..4 {
        for terminal in [TerminalState::Cancelled, TerminalState::Abandoned] {
            for overflow in [false, true] {
                // Given a published but never attempted replacement, cancelled or abandoned.
                let (fx, recipients) = world();
                let recipient = &recipients[index];
                if overflow {
                    older_entries(&fx, recipient);
                }
                let ctx = fx.reopen();
                let inbox = Inbox::new(&ctx.store, recipient.clone()).unwrap();
                let message = post(&ctx, recipient, "unattempted replacement");
                let envelope = inbox
                    .publish(
                        &SessionBinding {
                            session: recipient.key().unwrap(),
                            endpoint: None,
                            transcript: None,
                        },
                        "fixture",
                        MAX_PAYLOAD,
                        true,
                    )
                    .unwrap()
                    .unwrap();
                assert!(envelope.attempts.is_empty());
                inbox.retire_unattempted(&envelope).unwrap();
                inbox
                    .mark_terminal(message.seq, terminal, "fixture", "fixture")
                    .unwrap();
                sync(&fx, 0);
                let row = restart_and_promote(&fx, recipient, &message, overflow, 0);
                assert_eq!(
                    row.kind,
                    if terminal == TerminalState::Cancelled {
                        "delivery_cancelled"
                    } else {
                        "delivery_abandoned"
                    }
                );
                assert_eq!(
                    entry(&fx, recipient, message.seq).retry_evidence.history,
                    RetryHistory::NeverAttempted
                );
                assert_eq!(
                    row.next_action.command,
                    Some(format!("corral inbox retry {}", message.id))
                );
                // When the real retry and delivery execute after reopening.
                let reopened = fx.reopen();
                let successor =
                    corral_ops::delivery::retry(&reopened.store, recipient, message.seq).unwrap();
                assert_eq!(successor.recipient, *recipient);
                let transport = Transport::new(SendOutcome::Accepted);
                deliver(&reopened, recipient, &transport, None).unwrap();
                sync(&fx, 60);
                // Then exactly one send and one attributed action belong to the original key.
                exact_sends(&transport, &recipients, index, 1);
                metrics(&fx, &row, 60, 1);
            }
        }
    }
}

#[test]
fn s21_completed_legacy_index_remains_incomplete_and_inspect_only_on_every_recipient() {
    for index in 0..4 {
        // Given an unattempted cancelled message in a legacy store without an owner header.
        let (fx, recipients) = world();
        let recipient = &recipients[index];
        let ctx = fx.reopen();
        let inbox = Inbox::new(&ctx.store, recipient.clone()).unwrap();
        let message = post(&ctx, recipient, "legacy obligation");
        inbox
            .mark_terminal(message.seq, TerminalState::Cancelled, "fixture", "fixture")
            .unwrap();
        inbox
            .update_state(|state| {
                state.owner_header = None;
                Ok(())
            })
            .unwrap();
        inbox.maintain_owner_header(at(0)).unwrap();
        sync(&fx, 0);
        assert!(
            inbox
                .state()
                .unwrap()
                .owner_header
                .unwrap()
                .legacy_debt
                .is_some()
        );
        // When explicit index-owner completes and a new owning Ctx reconciles again.
        let header = inbox.index_owner(64).unwrap();
        assert!(header.legacy_debt.is_none());
        assert_eq!(
            header.entries[0].retry_evidence.coverage,
            RetryCoverage::Unknown
        );
        sync(&fx, 30);
        let row = condition(&fx, recipient, &message);
        // Then completion of identity indexing never claims complete eligibility evidence.
        assert_eq!(row.kind, "delivery_evidence_incomplete");
        assert_eq!(
            row.next_action.command,
            Some(format!("corral inbox inspect-retry {}", message.id))
        );
        restart_and_promote(&fx, recipient, &message, false, 30);
        let transport = Transport::new(SendOutcome::Accepted);
        assert_eq!(
            deliver(&fx.reopen(), recipient, &transport, None).unwrap(),
            DeliveryStatus::Idle
        );
        exact_sends(&transport, &recipients, index, 0);
        metrics(&fx, &row, 60, 0);
    }
}

#[test]
fn s21_ten_current_attempts_preserve_prior_unsafe_witness_on_every_recipient() {
    for index in 0..4 {
        for accepted in [false, true] {
            // Given unsafe E1 on a ninth entry, followed by owner-authorized E2.
            let (fx, recipients) = world();
            let recipient = &recipients[index];
            older_entries(&fx, recipient);
            let ctx = fx.reopen();
            let message = post(&ctx, recipient, "ten current attempts");
            let uncertain = SendOutcome::Uncertain {
                reason: "acknowledgement lost".into(),
            };
            let mut transport = Transport::new(if accepted {
                SendOutcome::Accepted
            } else {
                uncertain.clone()
            });
            deliver(&ctx, recipient, &transport, None).unwrap();
            sync(&fx, 0);
            Inbox::new(&fx.reopen().store, recipient.clone())
                .unwrap()
                .recover(Recovery::Resend)
                .unwrap();
            transport.idempotent_retry = true;
            transport.set(uncertain);
            // When E2 retries idempotently ten times, the tenth attempt definitively rejects.
            for ordinal in 1..=10 {
                if ordinal == 10 {
                    transport.set(rejected());
                }
                assert_eq!(
                    deliver(&fx.reopen(), recipient, &transport, None).unwrap(),
                    DeliveryStatus::Sent { envelope: 2 }
                );
                sync(&fx, ordinal);
            }
            let proof = entry(&fx, recipient, message.seq);
            assert_eq!(proof.retry_evidence.coverage, RetryCoverage::Complete);
            let RetryHistory::Barred { first } = &proof.retry_evidence.history else {
                panic!("lost prior witness: {proof:?}");
            };
            assert_eq!(
                first.reason,
                if accepted {
                    WitnessReason::Accepted
                } else {
                    WitnessReason::Uncertain
                }
            );
            assert_eq!(first.attempt.unwrap().envelope, 1);
            let last = proof.delivery.as_ref().unwrap().attempt.unwrap();
            assert_eq!((last.envelope, last.ordinal), (2, 10));
            // Then restart and FIFO promotion retain the bar, not just the latest rejection.
            let row = restart_and_promote(&fx, recipient, &message, true, 10);
            assert_eq!(row.kind, "delivery_failed");
            assert_eq!(
                row.next_action.command,
                Some(format!("corral inbox inspect-retry {}", message.id))
            );
            assert!(
                corral_ops::delivery::retry(&fx.reopen().store, recipient, message.seq).is_err()
            );
            assert_eq!(
                deliver(&fx.reopen(), recipient, &transport, None).unwrap(),
                DeliveryStatus::Idle
            );
            exact_sends(&transport, &recipients, index, 11);
            sync(&fx, 60);
            metrics(&fx, &row, 60, 0);
        }
    }
}
