use std::sync::Barrier;
use std::thread;

use super::*;

fn publication(ctx: &Ctx, total: usize, adapter: &str) -> (Vec<Message>, Envelope) {
    let messages = (0..total)
        .map(|index| post(ctx, &format!("matrix instruction {index}")))
        .collect();
    let inbox = Inbox::new(&ctx.store, Recipient::Pm).unwrap();
    let _deliver = inbox.delivery_lock().unwrap();
    let envelope = inbox
        .publish(&binding("session-1"), adapter, MAX_PAYLOAD, false)
        .unwrap()
        .unwrap();
    (messages, envelope)
}

fn cancel(ctx: &Ctx, messages: &[Message], cancelled: &[usize]) {
    let inbox = Inbox::new(&ctx.store, Recipient::Pm).unwrap();
    let _deliver = inbox.delivery_lock().unwrap();
    for index in cancelled {
        inbox
            .mark_terminal(
                messages[*index].seq,
                TerminalState::Cancelled,
                "withdrawn",
                "manual",
            )
            .unwrap();
    }
}

fn before_first_attempt(total: usize, cancelled: &[usize]) {
    // Given a recovered publication containing the requested membership shape.
    let (dir, ctx) = fixture();
    let (messages, original) = publication(&ctx, total, "fake");
    let inbox = Inbox::new(&ctx.store, Recipient::Pm).unwrap();
    let record = std::fs::read(inbox.envelope_path(original.number)).unwrap();
    let payload = inbox.payload(&original).unwrap();

    // When cancellation wins before the first attempt and the process restarts.
    cancel(&ctx, &messages, cancelled);
    let reopened = Ctx::from_store(Store::open(dir.path()).unwrap());
    let fake = Fake::new(SendOutcome::Accepted);
    let status = tick(&reopened, &fake);

    // Then only eligible survivors get a first send; the old bytes never change.
    let inbox = Inbox::new(&reopened.store, Recipient::Pm).unwrap();
    let remaining: Vec<_> = messages
        .iter()
        .enumerate()
        .filter(|(index, _)| !cancelled.contains(index))
        .map(|(_, message)| message.seq)
        .collect();
    if remaining.is_empty() {
        assert_eq!(status, DeliveryStatus::Idle);
        assert!(fake.sent.borrow().is_empty());
        assert_eq!(inbox.envelopes().unwrap().len(), 1);
    } else {
        assert_eq!(status, DeliveryStatus::Sent { envelope: 2 });
        let replacement = inbox.envelope(2).unwrap();
        assert_eq!(replacement.members, remaining);
        assert_eq!(replacement.adapter, original.adapter);
        assert_eq!(replacement.binding, original.binding);
        assert_ne!(replacement.idempotency_key, original.idempotency_key);
        let sent = fake.sent.borrow();
        assert_eq!(sent.len(), 1);
        for (index, message) in messages.iter().enumerate() {
            assert_eq!(
                sent[0].payload.contains(&message.body),
                !cancelled.contains(&index),
                "cancelled indices: {cancelled:?}, member: {index}"
            );
        }
    }
    assert_eq!(
        std::fs::read(inbox.envelope_path(original.number)).unwrap(),
        record
    );
    assert_eq!(inbox.payload(&original).unwrap(), payload);
    assert!(inbox.envelope(original.number).unwrap().attempts.is_empty());
}

#[test]
fn single_cancellation_before_first_attempt_has_no_send() {
    before_first_attempt(1, &[0]);
}

#[test]
fn partial_cancellation_before_first_attempt_only_sends_survivors() {
    for (total, cancelled) in [(2, vec![0]), (2, vec![1]), (3, vec![1]), (3, vec![0, 2])] {
        before_first_attempt(total, &cancelled);
    }
}

#[test]
fn all_members_cancelled_before_first_attempt_have_no_send() {
    before_first_attempt(2, &[0, 1]);
    before_first_attempt(3, &[0, 1, 2]);
}

fn after_first_attempt(total: usize, cancelled: &[usize]) {
    for outcome in [
        SendOutcome::Accepted,
        SendOutcome::Admitted { detail: json!({}) },
        SendOutcome::Held {
            reason: "offline".into(),
        },
        SendOutcome::Uncertain {
            reason: "response lost".into(),
        },
    ] {
        for idempotent in [false, true] {
            // Given a first send with fixed bytes, adapter, binding and key.
            let (dir, ctx) = fixture();
            let messages: Vec<_> = (0..total)
                .map(|index| post(&ctx, &format!("attempted instruction {index}")))
                .collect();
            let mut fake = Fake::new(outcome.clone());
            fake.caps.idempotent_retry = idempotent;
            assert_eq!(tick(&ctx, &fake), DeliveryStatus::Sent { envelope: 1 });
            let inbox = Inbox::new(&ctx.store, Recipient::Pm).unwrap();
            let original = inbox.envelope(1).unwrap();
            let payload = inbox.payload(&original).unwrap();

            // When the owner cancels this membership shape after that first attempt.
            cancel(&ctx, &messages, cancelled);
            let reopened = Ctx::from_store(Store::open(dir.path()).unwrap());
            let status = tick(&reopened, &fake);

            // Then an attempted publication is never repacked or given a new key.
            let all = cancelled.len() == total;
            let uncertain = matches!(outcome, SendOutcome::Uncertain { .. });
            let replay = !all && uncertain && idempotent;
            assert_eq!(
                status,
                if all {
                    DeliveryStatus::Idle
                } else if replay {
                    DeliveryStatus::Sent { envelope: 1 }
                } else if uncertain {
                    DeliveryStatus::Uncertain
                } else {
                    DeliveryStatus::Waiting
                }
            );
            let inbox = Inbox::new(&reopened.store, Recipient::Pm).unwrap();
            let current = inbox.envelope(1).unwrap();
            assert_eq!(inbox.envelopes().unwrap().len(), 1);
            assert_eq!(current.members, original.members);
            assert_eq!(current.adapter, original.adapter);
            assert_eq!(current.binding, original.binding);
            assert_eq!(current.idempotency_key, original.idempotency_key);
            assert_eq!(inbox.payload(&current).unwrap(), payload);
            let sent = fake.sent.borrow();
            assert_eq!(sent.len(), if replay { 2 } else { 1 });
            if replay {
                assert_eq!(sent[0], sent[1]);
            }
        }
    }
}

#[test]
fn single_cancellation_after_first_attempt_never_resends() {
    after_first_attempt(1, &[0]);
}

#[test]
fn partial_cancellation_after_first_attempt_preserves_retry_contract() {
    for cancelled in [vec![0], vec![1], vec![0, 2]] {
        after_first_attempt(3, &cancelled);
    }
}

#[test]
fn all_members_cancelled_after_first_attempt_never_resend() {
    after_first_attempt(2, &[0, 1]);
    after_first_attempt(3, &[0, 1, 2]);
}

#[test]
fn observation_wins_cancellation_matrix_in_both_orders_and_barrier_race() {
    for attempted in [false, true] {
        for (total, cancelled) in [(1, vec![0]), (3, vec![1]), (3, vec![0, 1, 2])] {
            for order in 0..3 {
                // Given single, partial or all-member settlement at either attempt boundary.
                let (_dir, ctx) = fixture();
                let (messages, original) = publication(&ctx, total, "fake");
                let fake = Fake::new(SendOutcome::Accepted);
                if attempted {
                    assert_eq!(tick(&ctx, &fake), DeliveryStatus::Sent { envelope: 1 });
                }
                let inbox = Inbox::new(&ctx.store, Recipient::Pm).unwrap();
                let evidence = Receipt {
                    envelope: Some(original.number),
                    binding: Some(original.binding.clone()),
                    observed_at: Timestamp::now(),
                    via: "manual".into(),
                    detail: json!({"complete":true}),
                };
                let observe = || {
                    for index in &cancelled {
                        inbox
                            .record_receipt(messages[*index].seq, ReceiptLevel::Observed, &evidence)
                            .unwrap();
                    }
                };

                // When receipt and cancellation serialize in either order or race at a barrier.
                match order {
                    0 => {
                        cancel(&ctx, &messages, &cancelled);
                        observe();
                    }
                    1 => {
                        observe();
                        cancel(&ctx, &messages, &cancelled);
                    }
                    2 => {
                        let barrier = Barrier::new(2);
                        thread::scope(|scope| {
                            let observation = scope.spawn(|| {
                                barrier.wait();
                                observe();
                            });
                            let cancellation = scope.spawn(|| {
                                barrier.wait();
                                cancel(&ctx, &messages, &cancelled);
                            });
                            observation.join().unwrap();
                            cancellation.join().unwrap();
                        });
                    }
                    _ => unreachable!(),
                }

                // Then observed wins, and only previously-unsent survivors are republished.
                let status = tick(&ctx, &fake);
                let all = cancelled.len() == total;
                assert_eq!(
                    status,
                    if all {
                        DeliveryStatus::Idle
                    } else if attempted {
                        DeliveryStatus::Waiting
                    } else {
                        DeliveryStatus::Sent { envelope: 2 }
                    }
                );
                assert_eq!(fake.sent.borrow().len(), usize::from(attempted || !all));
                for index in &cancelled {
                    let seq = messages[*index].seq;
                    assert_eq!(inbox.terminal(seq).unwrap(), None);
                    assert_eq!(
                        inbox.receipt(seq, ReceiptLevel::Observed).unwrap(),
                        Some(evidence.clone())
                    );
                }
                if !all && !attempted {
                    assert_eq!(
                        inbox.envelope(2).unwrap().members,
                        messages
                            .iter()
                            .enumerate()
                            .filter(|(index, _)| !cancelled.contains(index))
                            .map(|(_, message)| message.seq)
                            .collect::<Vec<_>>()
                    );
                    assert_eq!(inbox.envelope(1).unwrap(), original);
                }
            }
        }
    }
}

#[test]
fn partial_cancellation_keeps_published_adapter_without_fallback() {
    for published_fallback in [false, true] {
        // Given a partially-cancelled publication whose original adapter is unavailable.
        let (dir, ctx) = fixture();
        let (messages, original) = publication(
            &ctx,
            2,
            if published_fallback {
                "terminal"
            } else {
                "fake"
            },
        );
        let native = Fake::new(SendOutcome::Accepted);
        let mut fallback = Fake::new(SendOutcome::Accepted);
        fallback.name = "terminal";
        cancel(&ctx, &messages, &[0]);
        if !published_fallback {
            *native.readiness.borrow_mut() = Readiness::Unavailable {
                reason: "original temporarily unavailable".into(),
            };
        }
        let reopened = Ctx::from_store(Store::open(dir.path()).unwrap());
        assert!(matches!(
            deliver(
                &reopened,
                &Recipient::Pm,
                &native,
                (!published_fallback).then_some(&fallback as &dyn DeliveryAdapter),
            )
            .unwrap(),
            DeliveryStatus::Blocked { .. }
        ));
        let inbox = Inbox::new(&reopened.store, Recipient::Pm).unwrap();
        assert_eq!(inbox.envelope(1).unwrap(), original);
        assert_eq!(inbox.state().unwrap().open_envelope, Some(1));
        assert!(native.sent.borrow().is_empty());
        assert!(fallback.sent.borrow().is_empty());

        // When that same published adapter becomes ready, only it sends the survivor.
        *native.readiness.borrow_mut() = Readiness::Native;
        assert_eq!(
            deliver(&reopened, &Recipient::Pm, &native, Some(&fallback)).unwrap(),
            DeliveryStatus::Sent { envelope: 2 }
        );
        assert_eq!(inbox.envelope(2).unwrap().members, vec![messages[1].seq]);
        assert_eq!(inbox.envelope(2).unwrap().adapter, original.adapter);
        assert_eq!(native.sent.borrow().len(), usize::from(!published_fallback));
        assert_eq!(
            fallback.sent.borrow().len(),
            usize::from(published_fallback)
        );
    }
}

#[test]
fn cancelling_only_wake_preserves_unattempted_info_survivor_eligibility() {
    // Given a wake and info member whose consumed publication already authorized both.
    let (dir, ctx) = fixture();
    let wake = post(&ctx, "wake withdrawn");
    let inbox = Inbox::new(&ctx.store, Recipient::Pm).unwrap();
    let info = inbox
        .post(Post {
            kind: "main_movement",
            urgency: Urgency::Info,
            author: "owner",
            body: "still needed information",
            binding: MessageBinding::default(),
            subject: None,
            max_payload: MAX_PAYLOAD,
        })
        .unwrap();
    let original = {
        let _deliver = inbox.delivery_lock().unwrap();
        inbox
            .publish(&binding("session-1"), "fake", MAX_PAYLOAD, false)
            .unwrap()
            .unwrap()
    };
    cancel(&ctx, &[wake], &[0]);
    let reopened = Ctx::from_store(Store::open(dir.path()).unwrap());
    let fake = Fake::new(SendOutcome::Accepted);

    // When the wake is cancelled before the first attempt and delivery restarts.
    assert_eq!(tick(&reopened, &fake), DeliveryStatus::Sent { envelope: 2 });

    // Then the surviving info is not held or lost for lack of another wake.
    assert_eq!(inbox.envelope(1).unwrap(), original);
    assert_eq!(inbox.envelope(2).unwrap().members, vec![info.seq]);
    assert!(fake.sent.borrow()[0].payload.contains(&info.body));
    assert!(!inbox.flush_requested());
}
