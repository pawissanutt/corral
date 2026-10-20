use super::*;

fn interrupted_replacement(published_fallback: bool, rebind: bool) {
    // Given a publication retired by cancellation or rebinding before its replacement exists.
    let (dir, ctx) = fixture();
    let first = post(&ctx, "first instruction");
    let second = post(&ctx, "second instruction");
    let native = Fake::new(SendOutcome::Accepted);
    let mut fallback = Fake::new(SendOutcome::Accepted);
    fallback.name = "terminal";
    let original_adapter = if published_fallback {
        &fallback
    } else {
        &native
    };
    let (original, record, payload) = {
        let inbox = Inbox::new(&ctx.store, Recipient::Pm).unwrap();
        let _deliver = inbox.delivery_lock().unwrap();
        let original = inbox
            .publish(
                &binding("session-1"),
                original_adapter.name,
                MAX_PAYLOAD,
                false,
            )
            .unwrap()
            .unwrap();
        let record = std::fs::read(inbox.envelope_path(original.number)).unwrap();
        let payload = inbox.payload(&original).unwrap();
        if !rebind {
            inbox
                .mark_terminal(first.seq, TerminalState::Cancelled, "withdrawn", "manual")
                .unwrap();
        }
        inbox.retire_unattempted(&original).unwrap();
        (original, record, payload)
    };
    drop(ctx);
    let reopened = Ctx::from_store(Store::open(dir.path()).unwrap());
    let session = binding(if rebind { "session-2" } else { "session-1" });
    *original_adapter.binding.borrow_mut() = BindOutcome::Bound(session.clone());
    *original_adapter.readiness.borrow_mut() = Readiness::Unavailable {
        reason: "published adapter temporarily unavailable".into(),
    };

    // When delivery restarts with only the alternative adapter ready.
    let unavailable = deliver(&reopened, &Recipient::Pm, &native, Some(&fallback)).unwrap();

    // Then survivors remain pinned, even if the original adapter is omitted entirely.
    assert!(
        matches!(unavailable, DeliveryStatus::Blocked { .. }),
        "replacement changed published adapter: {unavailable:?}"
    );
    let alternative = if published_fallback {
        &native
    } else {
        &fallback
    };
    assert!(matches!(
        deliver(&reopened, &Recipient::Pm, alternative, None).unwrap(),
        DeliveryStatus::Blocked { .. }
    ));
    assert!(native.sent.borrow().is_empty());
    assert!(fallback.sent.borrow().is_empty());

    // Restoring the published adapter delivers the eligible members with a fresh identity.
    *original_adapter.readiness.borrow_mut() = Readiness::Native;
    assert_eq!(
        deliver(&reopened, &Recipient::Pm, &native, Some(&fallback)).unwrap(),
        DeliveryStatus::Sent { envelope: 2 }
    );
    let inbox = Inbox::new(&reopened.store, Recipient::Pm).unwrap();
    let replacement = inbox.envelope(2).unwrap();
    assert_eq!(
        replacement.members,
        if rebind {
            vec![first.seq, second.seq]
        } else {
            vec![second.seq]
        }
    );
    assert_eq!(replacement.adapter, original.adapter);
    assert_eq!(replacement.binding, session);
    assert_ne!(replacement.idempotency_key, original.idempotency_key);
    assert_eq!(original_adapter.sent.borrow().len(), 1);
    assert!(alternative.sent.borrow().is_empty());
    assert_eq!(
        std::fs::read(inbox.envelope_path(original.number)).unwrap(),
        record
    );
    assert_eq!(inbox.payload(&original).unwrap(), payload);
    assert_eq!(inbox.envelope(original.number).unwrap(), original);
}

#[test]
fn interrupted_cancellation_replacement_keeps_native_adapter() {
    interrupted_replacement(false, false);
}

#[test]
fn interrupted_cancellation_replacement_keeps_fallback_adapter() {
    interrupted_replacement(true, false);
}

#[test]
fn interrupted_rebind_replacement_keeps_native_adapter() {
    interrupted_replacement(false, true);
}

#[test]
fn interrupted_rebind_replacement_keeps_fallback_adapter() {
    interrupted_replacement(true, true);
}

#[test]
fn replacement_pin_does_not_capture_independent_messages_after_members_settle() {
    // Given an interrupted replacement whose original members are subsequently cancelled.
    let (dir, ctx) = fixture();
    let first = post(&ctx, "withdraw first");
    let second = post(&ctx, "withdraw second");
    {
        let inbox = Inbox::new(&ctx.store, Recipient::Pm).unwrap();
        let _deliver = inbox.delivery_lock().unwrap();
        let original = inbox
            .publish(&binding("session-1"), "fake", MAX_PAYLOAD, false)
            .unwrap()
            .unwrap();
        inbox.retire_unattempted(&original).unwrap();
        for message in [first, second] {
            inbox
                .mark_terminal(message.seq, TerminalState::Cancelled, "withdrawn", "manual")
                .unwrap();
        }
    }
    drop(ctx);
    let reopened = Ctx::from_store(Store::open(dir.path()).unwrap());
    let independent = post(&reopened, "independent message");
    let native = Fake::new(SendOutcome::Accepted);
    *native.readiness.borrow_mut() = Readiness::Unavailable {
        reason: "native offline".into(),
    };
    let mut fallback = Fake::new(SendOutcome::Accepted);
    fallback.name = "terminal";

    // When an independent message first selects its own available adapter.
    assert_eq!(
        deliver(&reopened, &Recipient::Pm, &native, Some(&fallback)).unwrap(),
        DeliveryStatus::Sent { envelope: 2 }
    );

    // Then the completed replacement obligation cannot impose its old adapter.
    let inbox = Inbox::new(&reopened.store, Recipient::Pm).unwrap();
    assert_eq!(inbox.envelope(2).unwrap().adapter, fallback.name);
    assert_eq!(inbox.envelope(2).unwrap().members, vec![independent.seq]);
    assert_eq!(inbox.state().unwrap().replacement_envelope, None);
    assert!(native.sent.borrow().is_empty());
    assert_eq!(fallback.sent.borrow().len(), 1);
}
