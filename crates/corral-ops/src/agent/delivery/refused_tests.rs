//! A PM generation out of input keys refuses new ones with `input_limit` before admission: the
//! envelope reached nobody. It is never resubmitted to that generation, and once relaunch
//! recovery settles it the next generation receives its messages once, in a fresh envelope.
use super::*;
use crate::agent::bridge::notice::settle_unconfirmed;

fn pm() -> BindTarget {
    BindTarget::Pm {
        project: "corral".into(),
    }
}

fn endpoint(fx: &Fx, generation: u32) -> PathBuf {
    generation_dir(&fx.ctx.store, &Recipient::Pm, generation)
        .unwrap()
        .join(SOCKET_FILE)
}

/// Generation 1 of a bound PM refuses every new key; returns its host, its scripted bridge and
/// the wake it refused.
fn refused(fx: &Fx) -> (ts::Host, FakeHost, u64) {
    let host = bound(fx, &pm(), "pm-pane", "s-pm-g1");
    let fake = FakeHost::start(fx, &Recipient::Pm, &host);
    fake.script().input_limit = true;
    let seq = post(fx, &Recipient::Pm, "question_wake", "T-001/R1/Q1 is open.");
    let status = deliver(&fx.ctx, &Recipient::Pm, &CorralPiAdapter, None).unwrap();
    assert!(
        matches!(&status, DeliveryStatus::Blocked { reason } if reason.starts_with("host_refused")),
        "{status:?}"
    );
    assert_eq!(fake.submits().len(), 1);
    (host, fake, seq)
}

#[test]
fn a_refused_pm_envelope_is_not_resubmitted_while_its_generation_is_bound() {
    let fx = ts::fx();
    let _runtime = runtime(&fx, &[&Recipient::Pm]);
    let (_, fake, seq) = refused(&fx);

    let status = deliver(&fx.ctx, &Recipient::Pm, &CorralPiAdapter, None).unwrap();

    assert!(
        matches!(status, DeliveryStatus::Blocked { .. }),
        "{status:?}"
    );
    assert_eq!(fake.submits().len(), 1, "no second submit to generation 1");
    let inbox = Inbox::new(&fx.ctx.store, Recipient::Pm).unwrap();
    assert_eq!(inbox.terminal(seq).unwrap(), None);
}

#[test]
fn an_input_limit_refusal_reaches_the_next_pm_generation_once() {
    // Given generation 1 refused the PM's wake with input_limit:
    let fx = ts::fx();
    let _runtime = runtime(&fx, &[&Recipient::Pm]);
    let (host, old, seq) = refused(&fx);
    let inbox = Inbox::new(&fx.ctx.store, Recipient::Pm).unwrap();

    // When relaunch recovery settles it from a session without its delivery entry:
    let session = host.sessions.join("session.jsonl");
    let unconfirmed =
        settle_unconfirmed(&fx.ctx, &Recipient::Pm, &endpoint(&fx, 1), &session).unwrap();

    // Then the wake is pending again, neither failed nor unknown, and envelope 1 is closed:
    assert!(unconfirmed.is_empty(), "{unconfirmed:?}");
    assert_eq!(inbox.terminal(seq).unwrap(), None);
    assert!(!observed(&fx, &Recipient::Pm, seq));
    assert_eq!(inbox.state().unwrap().open_envelope, None);
    assert_eq!(ts::events(&fx, "delivery_requeued"), 1);
    assert_eq!(ts::events(&fx, "delivery_fenced"), 0);

    // And generation 2 receives it once, in a fresh envelope under a new key:
    let next = bound(&fx, &pm(), "pm-pane", "s-pm-g2");
    assert_eq!(next.identity.generation, 2);
    let fake = FakeHost::start(&fx, &Recipient::Pm, &next);
    deliver(&fx.ctx, &Recipient::Pm, &CorralPiAdapter, None).unwrap();
    deliver(&fx.ctx, &Recipient::Pm, &CorralPiAdapter, None).unwrap();
    assert!(observed(&fx, &Recipient::Pm, seq));
    let fresh = inbox.envelope(2).unwrap();
    assert_eq!(fresh.members, [seq]);
    let submits = fake.submits();
    assert_eq!(submits.len(), 1);
    assert_eq!(
        submits[0]["idempotency_key"],
        json!(wire_key(&fresh.idempotency_key))
    );
    assert_eq!(old.submits().len(), 1);
}

#[test]
fn recovery_observes_a_refused_envelope_its_session_recorded() {
    let fx = ts::fx();
    let _runtime = runtime(&fx, &[&Recipient::Pm]);
    let (host, _old, seq) = refused(&fx);
    let inbox = Inbox::new(&fx.ctx.store, Recipient::Pm).unwrap();
    let envelope = inbox.envelope(1).unwrap();
    let session = host.sessions.join("session.jsonl");
    let text = inbox.payload(&envelope).unwrap();
    append(
        &session,
        &json!({"type": "custom", "customType": ENTRY_TYPE, "id": "e1", "data": {
            "idempotency_key": wire_key(&envelope.idempotency_key),
            "text_sha256": observe::sha256(&text)}}),
    );

    let unconfirmed =
        settle_unconfirmed(&fx.ctx, &Recipient::Pm, &endpoint(&fx, 1), &session).unwrap();

    assert!(unconfirmed[0].observed);
    assert!(observed(&fx, &Recipient::Pm, seq));
    assert_eq!(ts::events(&fx, "delivery_requeued"), 0);
}
