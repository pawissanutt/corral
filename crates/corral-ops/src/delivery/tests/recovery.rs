use super::*;

#[test]
fn available_delivery_observation_wins_terminal_state_before_retirement() {
    for use_fallback in [false, true] {
        for outcome in [
            SendOutcome::Accepted,
            SendOutcome::Admitted { detail: json!({}) },
            SendOutcome::Held {
                reason: "offline".into(),
            },
            SendOutcome::Uncertain {
                reason: "lost response".into(),
            },
        ] {
            for terminal in [
                TerminalState::Cancelled,
                TerminalState::Failed,
                TerminalState::Abandoned,
            ] {
                // Given an attempted terminal envelope with complete recipient-side evidence.
                let (dir, ctx) = fixture();
                let message = post(&ctx, "delivered before cancellation");
                let native = Fake::new(outcome.clone());
                let mut fallback = Fake::new(outcome.clone());
                fallback.name = "terminal";
                let selected = if use_fallback {
                    *native.readiness.borrow_mut() = Readiness::Unavailable {
                        reason: "not installed".into(),
                    };
                    &fallback
                } else {
                    &native
                };
                deliver(&ctx, &Recipient::Pm, &native, Some(&fallback)).unwrap();
                let inbox = Inbox::new(&ctx.store, Recipient::Pm).unwrap();
                inbox
                    .mark_terminal(message.seq, terminal, "owner cancelled", "manual")
                    .unwrap();
                selected
                    .observations
                    .borrow_mut()
                    .push(Observation::Delivered {
                        envelope: 1,
                        members: vec![message.seq],
                        detail: json!({"complete_record":7}),
                    });
                *native.readiness.borrow_mut() = Readiness::Native;
                let reopened = Ctx::from_store(Store::open(dir.path()).unwrap());

                // When normal delivery reconciles after cancellation and a process restart.
                let status = deliver(&reopened, &Recipient::Pm, &native, Some(&fallback)).unwrap();

                // Then observation wins through the original adapter, without another send.
                assert_eq!(status, DeliveryStatus::Idle);
                let inbox = Inbox::new(&reopened.store, Recipient::Pm).unwrap();
                let receipt = inbox
                    .receipt(message.seq, ReceiptLevel::Observed)
                    .unwrap()
                    .expect("available delivery evidence must be reconciled before retirement");
                assert_eq!(receipt.via, selected.name);
                assert_eq!(receipt.binding, Some(binding("session-1")));
                assert_eq!(receipt.detail, json!({"complete_record":7}));
                assert_eq!(inbox.terminal(message.seq).unwrap(), None);
                assert_eq!(inbox.state().unwrap().open_envelope, None);
                assert_eq!(selected.sent.borrow().len(), 1);
                assert!(selected.observations.borrow().is_empty());
            }
        }
    }
}

#[test]
fn flushed_info_rebinds_after_interruption_before_first_attempt() {
    // Given the persisted boundary after an info-only publication consumes its flush.
    let (dir, ctx) = fixture();
    let inbox = Inbox::new(&ctx.store, Recipient::Pm).unwrap();
    let message = inbox
        .post(Post {
            kind: "main_movement",
            urgency: Urgency::Info,
            author: "owner",
            body: "main moved",
            binding: MessageBinding::default(),
            subject: None,
            max_payload: MAX_PAYLOAD,
        })
        .unwrap();
    inbox.request_flush().unwrap();
    let original = {
        let _deliver = inbox.delivery_lock().unwrap();
        let envelope = inbox
            .publish(&binding("old-session"), "fake", MAX_PAYLOAD, true)
            .unwrap()
            .unwrap();
        inbox.consume_flush().unwrap();
        envelope
    };
    assert!(original.attempts.is_empty());
    assert!(!inbox.flush_requested());
    let original_bytes = std::fs::read(inbox.envelope_path(original.number)).unwrap();
    let reopened = Ctx::from_store(Store::open(dir.path()).unwrap());
    let fake = Fake::new(SendOutcome::Accepted);

    // When delivery restarts against the replacement session.
    let status = tick(&reopened, &fake);

    // Then the previous flush authorizes a new immutable publication without owner action.
    assert_eq!(status, DeliveryStatus::Sent { envelope: 2 });
    let inbox = Inbox::new(&reopened.store, Recipient::Pm).unwrap();
    assert_eq!(inbox.message(message.seq).unwrap(), message);
    assert_eq!(
        std::fs::read(inbox.envelope_path(original.number)).unwrap(),
        original_bytes
    );
    assert_eq!(inbox.envelope(2).unwrap().members, original.members);
    assert_ne!(
        inbox.envelope(2).unwrap().idempotency_key,
        original.idempotency_key
    );
    assert_eq!(fake.sent.borrow().len(), 1);
    assert_eq!(fake.sent.borrow()[0].binding, binding("session-1"));
    assert!(!inbox.flush_requested());
}
