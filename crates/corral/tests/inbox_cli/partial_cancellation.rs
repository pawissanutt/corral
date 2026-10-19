use std::cell::RefCell;

use super::*;

struct Capture {
    sent: RefCell<Vec<(String, String)>>,
}

impl DeliveryAdapter for Capture {
    fn harness(&self) -> &'static str {
        "fixture"
    }
    fn capabilities(&self) -> Capabilities {
        LateUncertainty.capabilities()
    }
    fn readiness(&self, ctx: &Ctx, recipient: &RecipientRef) -> anyhow::Result<Readiness> {
        LateUncertainty.readiness(ctx, recipient)
    }
    fn bind(&self, ctx: &Ctx, recipient: &RecipientRef) -> anyhow::Result<BindOutcome> {
        LateUncertainty.bind(ctx, recipient)
    }
    fn send(
        &self,
        ctx: &Ctx,
        _: &SessionBinding,
        envelope: &corral_core::inbox::Envelope,
    ) -> SendOutcome {
        let inbox = Inbox::new(&ctx.store, Recipient::Pm).unwrap();
        self.sent.borrow_mut().push((
            envelope.idempotency_key.clone(),
            inbox.payload(envelope).unwrap(),
        ));
        SendOutcome::Accepted
    }
    fn observe(
        &self,
        _: &Ctx,
        _: &SessionBinding,
        _: &mut corral_core::inbox::ObserveCursor,
    ) -> anyhow::Result<Vec<Observation>> {
        Ok(Vec::new())
    }
}

#[test]
fn real_cli_partial_cancellation_excludes_withdrawn_member_from_first_send() {
    for cancelled_index in 0..2 {
        // Given two members frozen in a persisted, never-attempted publication.
        let dir = tempfile::tempdir().unwrap();
        let ctx = Ctx::from_store(Store::init(dir.path()).unwrap());
        let inbox = Inbox::new(&ctx.store, Recipient::Pm).unwrap();
        let messages = [
            post(&inbox, "FIRST-INSTRUCTION"),
            post(&inbox, "SECOND-INSTRUCTION"),
        ];
        let session = SessionBinding {
            session: "fixture".into(),
            endpoint: None,
            transcript: None,
        };
        let original = {
            let _deliver = inbox.delivery_lock().unwrap();
            inbox
                .publish(&session, "fixture", MAX_PAYLOAD, false)
                .unwrap()
                .unwrap()
        };
        let original_record = std::fs::read(inbox.envelope_path(original.number)).unwrap();
        let original_payload = inbox.payload(&original).unwrap();

        // When the real CLI cancels either member before delivery restarts.
        ok(
            dir.path(),
            &[
                "mark",
                &messages[cancelled_index].id,
                "--cancel",
                "--reason",
                "withdrawn before first send",
            ],
        );
        let reopened = Ctx::from_store(Store::open(dir.path()).unwrap());
        let adapter = Capture {
            sent: RefCell::new(Vec::new()),
        };
        let result = deliver(&reopened, &Recipient::Pm, &adapter, None).unwrap();

        // Then only the survivor is sent under a new immutable publication.
        let remaining = &messages[1 - cancelled_index];
        let inbox = Inbox::new(&reopened.store, Recipient::Pm).unwrap();
        assert_eq!(result, DeliveryStatus::Sent { envelope: 2 });
        assert_eq!(inbox.envelope(2).unwrap().members, vec![remaining.seq]);
        let sent = adapter.sent.borrow();
        assert_eq!(sent.len(), 1);
        assert_ne!(sent[0].0, original.idempotency_key);
        assert!(sent[0].1.contains(&remaining.body));
        assert!(!sent[0].1.contains(&messages[cancelled_index].body));
        assert_eq!(inbox.envelope(2).unwrap().adapter, original.adapter);
        assert_eq!(inbox.envelope(2).unwrap().binding, original.binding);
        assert_eq!(
            std::fs::read(inbox.envelope_path(original.number)).unwrap(),
            original_record
        );
        assert_eq!(inbox.payload(&original).unwrap(), original_payload);
        assert_eq!(
            inbox.terminal(messages[cancelled_index].seq).unwrap(),
            Some(TerminalState::Cancelled)
        );
        assert!(inbox.envelope(original.number).unwrap().attempts.is_empty());
    }
}
