use super::*;

#[test]
fn one_adapter_keeps_distinct_full_session_cursors_after_restart() {
    for replacement in [
        binding("session-2"),
        SessionBinding {
            endpoint: Some("/new.sock".into()),
            transcript: Some("/new.jsonl".into()),
            ..binding("session-1")
        },
    ] {
        // Given one adapter with a retired stream and a new publication in another full binding.
        let (dir, ctx) = fixture();
        let fake = Fake::new(SendOutcome::Accepted);
        let first = post(&ctx, "old session");
        tick(&ctx, &fake);
        let inbox = Inbox::new(&ctx.store, Recipient::Pm).unwrap();
        inbox.recover(Recovery::Abandon).unwrap();
        tick(&ctx, &fake);
        *fake.binding.borrow_mut() = BindOutcome::Bound(replacement.clone());
        let second = post(&ctx, "replacement session");
        assert_eq!(tick(&ctx, &fake), DeliveryStatus::Sent { envelope: 2 });
        let reopened = Ctx::from_store(Store::open(dir.path()).unwrap());
        for (number, seq) in [(1, first.seq), (2, second.seq)] {
            fake.observations.borrow_mut().push(Observation::Delivered {
                envelope: number,
                members: vec![seq],
                detail: json!({"record":number}),
            });
        }

        // When both streams obtain complete evidence after restart.
        assert_eq!(tick(&reopened, &fake), DeliveryStatus::Idle);

        // Then each scan resumes its own offset, even when only endpoint/transcript changed.
        let reads = fake.reads.borrow();
        let old = &reads[reads.len() - 2];
        let new = &reads[reads.len() - 1];
        assert_eq!(old.0, binding("session-1"));
        assert_eq!(old.1.last_complete_line, 2);
        assert_eq!(new.0, replacement);
        assert_eq!(new.1, ObserveCursor::default());
        let inbox = Inbox::new(&reopened.store, Recipient::Pm).unwrap();
        assert_eq!(
            inbox
                .receipt(first.seq, ReceiptLevel::Observed)
                .unwrap()
                .unwrap()
                .binding,
            Some(binding("session-1"))
        );
        assert_eq!(
            inbox
                .receipt(second.seq, ReceiptLevel::Observed)
                .unwrap()
                .unwrap()
                .binding,
            Some(replacement)
        );
        assert_eq!(fake.sent.borrow().len(), 2);
    }
}

#[test]
fn retired_reader_error_does_not_block_an_unattempted_publication_in_the_same_stream() {
    // Given an abandoned send and a new publication interrupted before its first attempt.
    let (_dir, ctx) = fixture();
    let fake = Fake::new(SendOutcome::Accepted);
    post(&ctx, "old send");
    tick(&ctx, &fake);
    let inbox = Inbox::new(&ctx.store, Recipient::Pm).unwrap();
    inbox.recover(Recovery::Abandon).unwrap();
    post(&ctx, "new send");
    {
        let _deliver = inbox.delivery_lock().unwrap();
        inbox
            .publish(&binding("session-1"), "fake", MAX_PAYLOAD, false)
            .unwrap()
            .unwrap();
    }
    fake.fail_observation.set(true);

    // When the retired stream cannot be read before the new publication has been attempted.
    assert_eq!(tick(&ctx, &fake), DeliveryStatus::Sent { envelope: 2 });

    // Then only the new envelope is sent; the old observation error remains visible.
    assert_eq!(fake.sent.borrow().len(), 2);
    assert_ne!(fake.sent.borrow()[0].key, fake.sent.borrow()[1].key);
    assert!(
        ctx.store
            .events()
            .unwrap()
            .iter()
            .any(|event| event.kind == "delivery_observation_failed")
    );
}

#[test]
fn late_observation_after_terminal_retirement_wins_after_reopen_without_resend() {
    for abandon in [false, true] {
        // Given an attempted envelope retired during an empty observation pass.
        let (dir, ctx) = fixture();
        let message = post(&ctx, "delivered after retirement");
        let fake = Fake::new(SendOutcome::Accepted);
        tick(&ctx, &fake);
        let inbox = Inbox::new(&ctx.store, Recipient::Pm).unwrap();
        if abandon {
            inbox.recover(Recovery::Abandon).unwrap();
        } else {
            inbox
                .mark_terminal(message.seq, TerminalState::Cancelled, "cancel", "manual")
                .unwrap();
        }
        assert_eq!(tick(&ctx, &fake), DeliveryStatus::Idle);
        assert_eq!(inbox.state().unwrap().open_envelope, None);
        let reopened = Ctx::from_store(Store::open(dir.path()).unwrap());
        fake.observations.borrow_mut().push(Observation::Delivered {
            envelope: 1,
            members: vec![message.seq],
            detail: json!({"record":"late-complete"}),
        });

        // When the complete receipt arrives after retirement and process restart.
        assert_eq!(tick(&reopened, &fake), DeliveryStatus::Idle);

        // Then observed wins even without a new message or an open send gate.
        let inbox = Inbox::new(&reopened.store, Recipient::Pm).unwrap();
        let receipt = inbox
            .receipt(message.seq, ReceiptLevel::Observed)
            .unwrap()
            .expect("late evidence must survive terminal retirement");
        assert_eq!(receipt.binding, Some(binding("session-1")));
        assert_eq!(receipt.detail, json!({"record":"late-complete"}));
        assert_eq!(inbox.terminal(message.seq).unwrap(), None);
        assert_eq!(inbox.state().unwrap().open_envelope, None);
        assert_eq!(fake.sent.borrow().len(), 1);
    }
}

#[test]
fn retired_observation_preserves_adapter_and_full_binding_cursors() {
    for replacement in [
        binding("session-2"),
        SessionBinding {
            endpoint: Some("/replacement.sock".into()),
            transcript: Some("/replacement.jsonl".into()),
            ..binding("session-1")
        },
        binding("session-1"),
    ] {
        // Given a retired fallback and a newer native send, including a cursor saved before restart.
        let (dir, ctx) = fixture();
        let first = post(&ctx, "old session");
        let native = Fake::new(SendOutcome::Accepted);
        *native.readiness.borrow_mut() = Readiness::Unavailable {
            reason: "missing".into(),
        };
        let mut fallback = Fake::new(SendOutcome::Accepted);
        fallback.name = "terminal";
        deliver(&ctx, &Recipient::Pm, &native, Some(&fallback)).unwrap();
        let inbox = Inbox::new(&ctx.store, Recipient::Pm).unwrap();
        inbox.recover(Recovery::Abandon).unwrap();
        deliver(&ctx, &Recipient::Pm, &native, Some(&fallback)).unwrap();
        *native.readiness.borrow_mut() = Readiness::Native;
        *native.binding.borrow_mut() = BindOutcome::Bound(replacement.clone());
        *fallback.binding.borrow_mut() = BindOutcome::Bound(replacement.clone());
        let second = post(&ctx, "new session");
        // The former adapter may be absent without blocking publication.
        assert_eq!(
            deliver(&ctx, &Recipient::Pm, &native, None).unwrap(),
            DeliveryStatus::Sent { envelope: 2 }
        );
        let reopened = Ctx::from_store(Store::open(dir.path()).unwrap());
        fallback
            .observations
            .borrow_mut()
            .push(Observation::Delivered {
                envelope: 1,
                members: vec![first.seq],
                detail: json!({"record":"old-session"}),
            });

        // When the original reader returns after the active binding has changed.
        assert_eq!(
            deliver(&reopened, &Recipient::Pm, &native, Some(&fallback)).unwrap(),
            DeliveryStatus::Waiting
        );

        // Then old evidence uses its original adapter/session cursor, never the new one's.
        let inbox = Inbox::new(&reopened.store, Recipient::Pm).unwrap();
        assert!(
            inbox
                .receipt(first.seq, ReceiptLevel::Observed)
                .unwrap()
                .is_some()
        );
        let old_read = fallback.reads.borrow().last().unwrap().clone();
        assert_eq!(old_read.0, binding("session-1"));
        assert_eq!(old_read.1.last_complete_line, 1);
        let new_read = native.reads.borrow().last().unwrap().clone();
        assert_eq!(new_read.0, replacement);
        assert_eq!(new_read.1, ObserveCursor::default());
        assert_eq!(inbox.state().unwrap().binding, Some(replacement));
        assert_eq!(inbox.state().unwrap().observe_cursor.last_complete_line, 1);
        assert_eq!(inbox.envelope(2).unwrap().members, vec![second.seq]);
        assert_eq!(native.sent.borrow().len(), 1);
        assert_eq!(fallback.sent.borrow().len(), 1);
    }
}

#[test]
fn retired_reader_error_is_reported_without_blocking_a_later_send() {
    // Given an abandoned attempted fallback whose original transcript is temporarily unreadable.
    let (_dir, ctx) = fixture();
    let first = post(&ctx, "old send");
    let native = Fake::new(SendOutcome::Accepted);
    *native.readiness.borrow_mut() = Readiness::Unavailable {
        reason: "missing".into(),
    };
    let mut fallback = Fake::new(SendOutcome::Accepted);
    fallback.name = "terminal";
    deliver(&ctx, &Recipient::Pm, &native, Some(&fallback)).unwrap();
    let inbox = Inbox::new(&ctx.store, Recipient::Pm).unwrap();
    inbox.recover(Recovery::Abandon).unwrap();
    fallback.fail_observation.set(true);
    *native.readiness.borrow_mut() = Readiness::Native;
    *native.binding.borrow_mut() = BindOutcome::Bound(binding("session-2"));
    post(&ctx, "later send");

    // When another message can be delivered independently.
    assert_eq!(
        deliver(&ctx, &Recipient::Pm, &native, Some(&fallback)).unwrap(),
        DeliveryStatus::Sent { envelope: 2 }
    );

    // Then the old read failure is visible, and it neither resends nor blocks new delivery.
    assert!(
        ctx.store
            .events()
            .unwrap()
            .iter()
            .any(|event| { event.kind == "delivery_observation_failed" })
    );
    assert_eq!(
        inbox.terminal(first.seq).unwrap(),
        Some(TerminalState::Abandoned)
    );
    assert_eq!(fallback.sent.borrow().len(), 1);
    assert_eq!(native.sent.borrow().len(), 1);
    fallback.fail_observation.set(false);
    fallback
        .observations
        .borrow_mut()
        .push(Observation::Delivered {
            envelope: 1,
            members: vec![first.seq],
            detail: json!({"record":"reader-recovered"}),
        });
    deliver(&ctx, &Recipient::Pm, &native, Some(&fallback)).unwrap();
    assert!(
        inbox
            .receipt(first.seq, ReceiptLevel::Observed)
            .unwrap()
            .is_some()
    );
}

#[test]
fn multiple_retired_envelopes_share_one_observation_pass_per_binding() {
    // Given two cancelled attempted envelopes in the same recorded session.
    let (_dir, ctx) = fixture();
    let fake = Fake::new(SendOutcome::Accepted);
    let inbox = Inbox::new(&ctx.store, Recipient::Pm).unwrap();
    let mut messages = Vec::new();
    for body in ["first", "second"] {
        let message = post(&ctx, body);
        tick(&ctx, &fake);
        inbox
            .mark_terminal(message.seq, TerminalState::Cancelled, "cancel", "manual")
            .unwrap();
        tick(&ctx, &fake);
        messages.push(message);
    }
    let before = fake.reads.borrow().len();
    for (number, message) in [1, 2].into_iter().zip(&messages) {
        fake.observations.borrow_mut().push(Observation::Delivered {
            envelope: number,
            members: vec![message.seq],
            detail: json!({"record":number}),
        });
    }

    // When both complete records become visible after both send gates have retired.
    assert_eq!(tick(&ctx, &fake), DeliveryStatus::Idle);

    // Then one session scan reconciles both, without another send.
    assert_eq!(fake.reads.borrow().len(), before + 1);
    for message in messages {
        assert!(
            inbox
                .receipt(message.seq, ReceiptLevel::Observed)
                .unwrap()
                .is_some()
        );
        assert_eq!(inbox.terminal(message.seq).unwrap(), None);
    }
    assert_eq!(fake.sent.borrow().len(), 2);
}
