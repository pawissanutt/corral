use std::sync::{Arc, Barrier};

use corral_core::backend::{BackendConfig, FaultInjector, ManualClock, fake::FakeBackend};
use corral_core::work::{self, Counter, HotScope};

use super::*;

#[test]
fn delivery_markers_deduplicate_concurrent_handles_without_files() {
    // Given separate handles racing to report the same alert and blocked reason.
    let (dir, ctx) = fixture();
    let barrier = Arc::new(Barrier::new(4));
    let handles: Vec<_> = (0..4)
        .map(|_| {
            let store = Store::open_unobserved(dir.path()).unwrap();
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                let inbox = Inbox::new(&store, Recipient::Pm).unwrap();
                barrier.wait();
                alert_once(&inbox, "delivery_uncertain", "env1", json!({})).unwrap();
                note_blocked(&inbox, "unbound").unwrap();
            })
        })
        .collect();

    // When all reports finish, then each marker and event was committed only once.
    for handle in handles {
        handle.join().unwrap();
    }
    let inbox = Inbox::new(&ctx.store, Recipient::Pm).unwrap();
    for kind in ["delivery_uncertain", "delivery_blocked"] {
        assert_eq!(
            ctx.store
                .events()
                .unwrap()
                .iter()
                .filter(|e| e.kind == kind)
                .count(),
            1
        );
    }
    assert_eq!(
        inbox
            .record::<bool>("alerts", "delivery_uncertain-env1")
            .unwrap(),
        Some(true)
    );
    assert_eq!(
        inbox
            .record::<String>("control", "blocked")
            .unwrap()
            .as_deref(),
        Some("unbound")
    );
    assert!(!inbox.dir().join("alerts").exists());
    assert!(!inbox.dir().join("blocked").exists());
}

#[test]
fn blocked_marker_reports_reason_changes_and_resets_after_clear() {
    // Given a recipient blocked for one reason.
    let (_dir, ctx) = fixture();
    let inbox = Inbox::new(&ctx.store, Recipient::Pm).unwrap();
    note_blocked(&inbox, "unbound").unwrap();

    // When the reason changes and a cleared condition recurs.
    note_blocked(&inbox, "busy").unwrap();
    note_blocked(&inbox, "busy").unwrap();
    clear_blocked(&inbox).unwrap();
    clear_blocked(&inbox).unwrap();
    assert_eq!(inbox.record::<String>("control", "blocked").unwrap(), None);
    note_blocked(&inbox, "busy").unwrap();

    // Then only transitions emit events.
    let events = ctx.store.events().unwrap();
    let reasons: Vec<_> = events
        .iter()
        .filter(|e| e.kind == "delivery_blocked")
        .map(|e| e.detail["reason"].as_str().unwrap())
        .collect();
    assert_eq!(reasons, ["unbound", "busy", "busy"]);
}

#[test]
fn failed_marker_operations_publish_neither_record_nor_event() {
    for blocked in [false, true] {
        for nth in 1..=3 {
            // Given a backend that fails at each primitive of the marker/event unit.
            let (_dir, ctx) = fixture();
            let faults = Arc::new(FaultInjector::default());
            let store = ctx.store.with_backend(Box::new(FakeBackend::new(
                BackendConfig::default(),
                Arc::new(ManualClock::new("2026-10-25T00:00:00Z".parse().unwrap())),
                Arc::clone(&faults),
            )));
            let inbox = Inbox::new(&store, Recipient::Pm).unwrap();
            let before = store.view().unwrap().seq();
            faults.arm(nth);

            // When the operation fails after a staged primitive write.
            let result = if blocked {
                note_blocked(&inbox, "unbound")
            } else {
                alert_once(&inbox, "delivery_uncertain", "env1", json!({}))
            };

            // Then neither half escaped, and the same report can be retried.
            assert!(result.is_err(), "primitive {nth}");
            assert_eq!(store.view().unwrap().seq(), before);
            assert!(inbox.records("alerts").unwrap().is_empty());
            assert_eq!(inbox.record::<String>("control", "blocked").unwrap(), None);
            assert!(store.events().unwrap().is_empty());
            if blocked {
                note_blocked(&inbox, "unbound").unwrap();
            } else {
                alert_once(&inbox, "delivery_uncertain", "env1", json!({})).unwrap();
            }
            assert_eq!(store.events().unwrap().len(), 1);
            assert_eq!(store.view().unwrap().seq().0, before.0 + 1);
        }
    }
}

#[test]
fn original_session_evidence_after_resend_ignores_grown_finished_history() {
    let mut measurements = Vec::new();
    for history in [10, 100] {
        // Given finished envelope history and an owner resend into a new session.
        let (dir, ctx) = fixture();
        let inbox = Inbox::new(&ctx.store, Recipient::Pm).unwrap();
        let now = "2026-10-25T00:00:00Z".parse().unwrap();
        for _ in 0..history {
            let message = post(&ctx, "finished");
            let mut envelope = inbox
                .publish(&binding("history"), "fake", MAX_PAYLOAD, false)
                .unwrap()
                .unwrap();
            inbox.begin_attempt(&mut envelope, now).unwrap();
            inbox
                .record_receipt(
                    message.seq,
                    ReceiptLevel::Observed,
                    &Receipt {
                        envelope: Some(envelope.number),
                        binding: Some(envelope.binding.clone()),
                        observed_at: now,
                        via: "fake".into(),
                        detail: json!({}),
                    },
                )
                .unwrap();
            inbox.save_envelope(&envelope).unwrap();
            inbox.close_settled(&envelope).unwrap();
        }
        inbox
            .update_state(|state| {
                state.next_seq = 1001;
                state.next_envelope = 1001;
                Ok(())
            })
            .unwrap();
        let message = post(&ctx, "resend");
        let fake = Fake::new(SendOutcome::Accepted);
        assert_eq!(
            deliver_at(&ctx, &Recipient::Pm, &fake, None, now).unwrap(),
            DeliveryStatus::Sent { envelope: 1001 }
        );
        inbox.recover(Recovery::Resend).unwrap();
        *fake.binding.borrow_mut() = BindOutcome::Bound(binding("session-2"));
        assert_eq!(
            deliver_at(&ctx, &Recipient::Pm, &fake, None, now).unwrap(),
            DeliveryStatus::Sent { envelope: 1002 }
        );
        assert_eq!(
            inbox.entry(message.seq).unwrap().envelope.unwrap().number,
            1002
        );
        assert_eq!(inbox.pending_observation_envelopes().unwrap().len(), 2);
        let reopened = Ctx::from_store(Store::open(dir.path()).unwrap());
        fake.reads.borrow_mut().clear();
        fake.observations.borrow_mut().push(Observation::Delivered {
            envelope: 1001,
            members: vec![message.seq],
            detail: json!({"original":true}),
        });
        let before = work::snapshot().total(Counter::RecordDecodes);

        // When original-session evidence arrives after the latest mapping was superseded.
        {
            let _hot = HotScope::enter();
            assert_eq!(
                deliver_at(&reopened, &Recipient::Pm, &fake, None, now).unwrap(),
                DeliveryStatus::Idle
            );
        }
        measurements.push(work::snapshot().total(Counter::RecordDecodes) - before);

        // Then original custody wins without resend or a history-dependent scan.
        let inbox = Inbox::new(&reopened.store, Recipient::Pm).unwrap();
        let receipt = inbox
            .receipt(message.seq, ReceiptLevel::Observed)
            .unwrap()
            .unwrap();
        assert_eq!(receipt.binding, Some(binding("session-1")));
        assert_eq!(receipt.envelope, Some(1001));
        assert_eq!(fake.reads.borrow().len(), 2);
        assert_eq!(fake.reads.borrow()[0].0, binding("session-1"));
        assert_eq!(fake.sent.borrow().len(), 2);
        assert!(inbox.pending_observation_envelopes().unwrap().is_empty());
        assert!(inbox.records("observation-pending").unwrap().is_empty());
    }
    assert!(measurements[0] > 0);
    assert_eq!(measurements[0], measurements[1]);
}

#[test]
fn observation_index_keeps_terminal_and_partially_observed_envelopes() {
    // Given an attempted envelope containing two messages, then abandoned by its owner.
    let (_dir, ctx) = fixture();
    let first = post(&ctx, "first");
    let second = post(&ctx, "second");
    let inbox = Inbox::new(&ctx.store, Recipient::Pm).unwrap();
    let now = "2026-10-25T00:00:00Z".parse().unwrap();
    let mut envelope = inbox
        .publish(&binding("original"), "fake", MAX_PAYLOAD, false)
        .unwrap()
        .unwrap();
    assert!(inbox.pending_observation_envelopes().unwrap().is_empty());
    inbox.begin_attempt(&mut envelope, now).unwrap();
    inbox.recover(Recovery::Abandon).unwrap();
    let receipt = Receipt {
        envelope: Some(envelope.number),
        binding: Some(envelope.binding.clone()),
        observed_at: now,
        via: "fake".into(),
        detail: json!({}),
    };
    inbox
        .record_receipt(first.seq, ReceiptLevel::Observed, &receipt)
        .unwrap();
    inbox.save_envelope(&envelope).unwrap();
    assert_eq!(
        inbox.pending_observation_envelopes().unwrap(),
        vec![envelope.clone()]
    );

    // When the last member obtains an observed receipt.
    inbox
        .record_receipt(second.seq, ReceiptLevel::Observed, &receipt)
        .unwrap();

    // Then selection drops the index only after every member has been observed.
    assert!(inbox.pending_observation_envelopes().unwrap().is_empty());
    assert!(inbox.records("observation-pending").unwrap().is_empty());
}

#[test]
fn failed_attempt_rolls_back_envelope_and_observation_indexes_together() {
    for nth in 1..=4 {
        // Given a publication and failure at each primitive of its first attempt.
        let (_dir, ctx) = fixture();
        let faults = Arc::new(FaultInjector::default());
        let store = ctx.store.with_backend(Box::new(FakeBackend::new(
            BackendConfig::default(),
            Arc::new(ManualClock::new("2026-10-25T00:00:00Z".parse().unwrap())),
            Arc::clone(&faults),
        )));
        let ctx = Ctx::from_store(store);
        post(&ctx, "attempt");
        let inbox = Inbox::new(&ctx.store, Recipient::Pm).unwrap();
        let mut envelope = inbox
            .publish(&binding("original"), "fake", MAX_PAYLOAD, false)
            .unwrap()
            .unwrap();
        let before = ctx.store.view().unwrap().seq();
        faults.arm(nth);

        // When the attempt cannot commit.
        assert!(
            inbox
                .begin_attempt(&mut envelope, "2026-10-25T00:00:00Z".parse().unwrap())
                .is_err()
        );

        // Then no partial attempt or index survives.
        assert_eq!(ctx.store.view().unwrap().seq(), before);
        assert!(inbox.envelope(envelope.number).unwrap().attempts.is_empty());
        assert!(inbox.records("observation-pending").unwrap().is_empty());
        assert!(inbox.records("late-evidence").unwrap().is_empty());
    }
}
