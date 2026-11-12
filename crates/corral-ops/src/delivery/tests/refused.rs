use super::*;

/// Envelope 1, attempted once into `session` and refused before admission, as the corral_pi
/// adapter records an `input_limit` refusal.
fn refused(ctx: &Ctx, session: &str) -> Envelope {
    let inbox = Inbox::new(&ctx.store, Recipient::Pm).unwrap();
    let mut envelope = inbox
        .publish(&binding(session), "fake", MAX_PAYLOAD, false)
        .unwrap()
        .unwrap();
    inbox
        .begin_attempt(&mut envelope, Timestamp::now())
        .unwrap();
    let refusal = Refusal {
        attempt: 1,
        code: "input_limit".into(),
        generation: Some(1),
    };
    inbox.record_refusal(envelope.number, &refusal).unwrap();
    inbox
        .finish_attempt(
            &mut envelope,
            SendOutcome::Rejected {
                reason: "input_limit".into(),
                retryable_after_fix: true,
            },
        )
        .unwrap();
    envelope
}

#[test]
fn a_refused_envelope_waits_for_its_session_and_is_requeued_not_orphaned_on_replacement() {
    // Given an envelope the bound session refused before admission:
    let (_dir, ctx) = fixture();
    let first = post(&ctx, "first");
    let original = refused(&ctx, "session-1");
    let fake = Fake::new(SendOutcome::Accepted);
    let inbox = Inbox::new(&ctx.store, Recipient::Pm).unwrap();

    // While that session stays bound it is neither resent nor failed:
    assert!(matches!(tick(&ctx, &fake), DeliveryStatus::Blocked { .. }));
    assert!(fake.sent.borrow().is_empty());
    assert_eq!(inbox.terminal(first.seq).unwrap(), None);

    // Its replacement receives the member once, in a fresh envelope:
    *fake.binding.borrow_mut() = BindOutcome::Bound(binding("session-2"));
    assert_eq!(tick(&ctx, &fake), DeliveryStatus::Sent { envelope: 2 });
    assert_eq!(tick(&ctx, &fake), DeliveryStatus::Waiting);
    let sent = fake.sent.borrow();
    assert_eq!(sent.len(), 1);
    assert_ne!(sent[0].key, original.idempotency_key);
    assert_eq!(sent[0].binding, binding("session-2"));
    assert_eq!(inbox.envelope(2).unwrap().members, [first.seq]);
    assert_eq!(inbox.terminal(first.seq).unwrap(), None);
}
